//! Key storage boundary.
//!
//! The application master key is *wrapped* / held by the OS keystore, never
//! written in the clear to source control or logs (see `docs/10`). This is a
//! trait so the real macOS Keychain and the developer fallback are swappable.
//!
//! * macOS: [`KeychainKeyStore`] stores the key as a generic password item in
//!   the login keychain via the `security-framework` crate.
//! * Other platforms: [`FileKeyStore`] persists the key under the app-owned
//!   `keys/` directory with `0600` permissions. This exists purely so the whole
//!   pipeline is testable off a Mac; production on macOS uses the Keychain.

use crate::crypto::MasterKey;
use crate::error::{Error, Result};

// Used by the macOS Keychain backend; unused on the dev fallback platform.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SERVICE: &str = "com.atlasdrive.masterkey";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const ACCOUNT: &str = "master-v1";

/// Anything that can persist and retrieve the wrapped master key.
pub trait KeyStore {
    /// Return the existing master key, or generate+persist a new one.
    fn get_or_create(&self) -> Result<MasterKey>;
    /// Human label for diagnostics (never includes key material).
    fn backend_name(&self) -> &'static str;
}

/// Select the appropriate keystore for this platform + app data root.
pub fn default_keystore(keys_dir: std::path::PathBuf) -> Box<dyn KeyStore> {
    #[cfg(target_os = "macos")]
    {
        let _ = keys_dir; // Keychain does not need the dir.
        Box::new(KeychainKeyStore)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(FileKeyStore { keys_dir })
    }
}

fn decode_key(bytes: &[u8]) -> Result<MasterKey> {
    if bytes.len() != 32 {
        return Err(Error::Encryption("stored key is not 32 bytes".into()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(bytes);
    Ok(MasterKey::from_bytes(arr, 1))
}

// ---------------------------------------------------------------------------
// macOS Keychain
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub struct KeychainKeyStore;

#[cfg(target_os = "macos")]
impl KeyStore for KeychainKeyStore {
    fn get_or_create(&self) -> Result<MasterKey> {
        use security_framework::passwords::{get_generic_password, set_generic_password};
        match get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => decode_key(&bytes),
            Err(_) => {
                let key = MasterKey::generate(1);
                set_generic_password(SERVICE, ACCOUNT, key.as_bytes())
                    .map_err(|e| Error::Encryption(format!("keychain store: {e}")))?;
                Ok(key)
            }
        }
    }
    fn backend_name(&self) -> &'static str {
        "macos-keychain"
    }
}

// ---------------------------------------------------------------------------
// Developer / non-macOS fallback
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "macos"))]
pub struct FileKeyStore {
    pub keys_dir: std::path::PathBuf,
}

#[cfg(not(target_os = "macos"))]
impl KeyStore for FileKeyStore {
    fn get_or_create(&self) -> Result<MasterKey> {
        use std::io::Write;
        std::fs::create_dir_all(&self.keys_dir)?;
        let path = self.keys_dir.join("master.key");
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            return decode_key(&bytes);
        }
        let key = MasterKey::generate(1);
        let mut f = std::fs::File::create(&path)?;
        f.write_all(key.as_bytes())?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path)?.permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(&path, perms)?;
        }
        Ok(key)
    }
    fn backend_name(&self) -> &'static str {
        "file-fallback-dev"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keystore_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let ks = default_keystore(dir.path().join("keys"));
        let k1 = ks.get_or_create().unwrap();
        let k2 = ks.get_or_create().unwrap();
        // Same key returned on second call (persisted).
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }
}
