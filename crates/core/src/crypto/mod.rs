//! Authenticated encryption for face embeddings and the key-management boundary.
//!
//! Requirements (see `docs/08_FACE_RECOGNITION_AND_REVIEW.md` and
//! `docs/10_SECURITY_AND_PRIVACY.md`):
//!   * Face embeddings are encrypted at rest with authenticated encryption.
//!   * A random application master key is generated and *wrapped* by the OS
//!     keystore (macOS Keychain). On non-macOS dev machines a file-based
//!     fallback keystore is used so the pipeline is testable off a Mac.
//!   * Keys are never hard-coded and never stored in source control.
//!   * Key versions are supported for rotation.
//!   * Vectors are never logged and decrypted buffers are zeroized.

pub mod keystore;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use zeroize::Zeroize;

use crate::error::{Error, Result};

/// Current on-disk encryption scheme version. Bump only with a migration path.
pub const ENC_VERSION: i64 = 1;

/// An encrypted payload: ciphertext + the nonce and versions needed to decrypt.
#[derive(Debug, Clone)]
pub struct Sealed {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub enc_version: i64,
    pub key_version: i64,
}

/// A 256-bit key that zeroizes itself on drop.
pub struct MasterKey {
    bytes: [u8; 32],
    pub version: i64,
}

impl MasterKey {
    pub fn from_bytes(bytes: [u8; 32], version: i64) -> Self {
        Self { bytes, version }
    }

    pub fn generate(version: i64) -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self { bytes, version }
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// Encrypt a float vector (little-endian f32) with AES-256-GCM.
pub fn seal_vector(key: &MasterKey, vector: &[f32]) -> Result<Sealed> {
    let mut plaintext = Vec::with_capacity(vector.len() * 4);
    for v in vector {
        plaintext.extend_from_slice(&v.to_le_bytes());
    }
    let sealed = seal(key, &plaintext);
    plaintext.zeroize();
    sealed
}

/// Encrypt arbitrary bytes with AES-256-GCM.
pub fn seal(key: &MasterKey, plaintext: &[u8]) -> Result<Sealed> {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| Error::Encryption(format!("key init: {e}")))?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| Error::Encryption(format!("encrypt: {e}")))?;
    Ok(Sealed {
        ciphertext,
        nonce: nonce_bytes.to_vec(),
        enc_version: ENC_VERSION,
        key_version: key.version,
    })
}

/// Decrypt to raw bytes. Caller is responsible for zeroizing sensitive output.
pub fn open(key: &MasterKey, sealed: &Sealed) -> Result<Vec<u8>> {
    if sealed.enc_version != ENC_VERSION {
        return Err(Error::Encryption(format!(
            "unsupported enc_version {}",
            sealed.enc_version
        )));
    }
    if sealed.key_version != key.version {
        return Err(Error::Encryption(format!(
            "key version mismatch: payload {} key {}",
            sealed.key_version, key.version
        )));
    }
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| Error::Encryption(format!("key init: {e}")))?;
    if sealed.nonce.len() != 12 {
        return Err(Error::Encryption("bad nonce length".into()));
    }
    let nonce = Nonce::from_slice(&sealed.nonce);
    cipher
        .decrypt(nonce, sealed.ciphertext.as_ref())
        .map_err(|_| Error::Encryption("authentication failed (tampered or wrong key)".into()))
}

/// Decrypt back to an f32 vector.
pub fn open_vector(key: &MasterKey, sealed: &Sealed) -> Result<Vec<f32>> {
    let mut bytes = open(key, sealed)?;
    if bytes.len() % 4 != 0 {
        bytes.zeroize();
        return Err(Error::Encryption("decrypted length not a multiple of 4".into()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    bytes.zeroize();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_vector() {
        let key = MasterKey::generate(1);
        let v = vec![0.1f32, -0.5, 3.14, 42.0, -0.0001];
        let sealed = seal_vector(&key, &v).unwrap();
        assert_ne!(sealed.ciphertext, {
            let mut raw = Vec::new();
            for x in &v {
                raw.extend_from_slice(&x.to_le_bytes());
            }
            raw
        });
        let back = open_vector(&key, &sealed).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn tamper_is_detected() {
        let key = MasterKey::generate(1);
        let mut sealed = seal(&key, b"secret vector bytes").unwrap();
        sealed.ciphertext[0] ^= 0xFF;
        assert!(open(&key, &sealed).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let k1 = MasterKey::generate(1);
        let k2 = MasterKey::generate(1);
        let sealed = seal(&k1, b"data").unwrap();
        assert!(open(&k2, &sealed).is_err());
    }
}
