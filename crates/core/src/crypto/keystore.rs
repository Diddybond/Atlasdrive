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
    /// The stored key. `Ok(None)` only when the store answers, definitely, that
    /// there is no key; any other failure (a locked or missing keychain, a
    /// refused prompt) is an error, never a reason to make a new key.
    fn get(&self) -> Result<Option<MasterKey>>;
    /// Generate and persist a new key.
    fn create(&self) -> Result<MasterKey>;
    /// Return the existing master key, or generate+persist a new one.
    ///
    /// Only safe when nothing has been encrypted yet; the application goes
    /// through [`master_key`], which checks.
    fn get_or_create(&self) -> Result<MasterKey> {
        match self.get()? {
            Some(k) => Ok(k),
            None => self.create(),
        }
    }
    /// Overwrite the stored key with `key`.
    ///
    /// Only restore uses this. Face embeddings and face crops are encrypted
    /// with the master key, so a catalogue restored onto different hardware is
    /// unreadable unless the key that encrypted it is put back first.
    fn put(&self, key: &MasterKey) -> Result<()>;
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

/// What a missing key means for a catalogue that already holds face data.
pub const MISSING_KEY: &str = "The key that protects face data is not in this Mac's Keychain. \
AtlasDrive will not make a new one, because that would lock the existing faces away for good. \
Restore the key from a backup in Settings.";

/// The master key for this catalogue.
///
/// A new key is made only for a catalogue with nothing encrypted in it. If
/// face data already exists and the key cannot be found, making a fresh key
/// would silently orphan every face crop and embedding, so it is an error.
pub fn master_key(keys_dir: std::path::PathBuf, conn: &rusqlite::Connection) -> Result<MasterKey> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = cached(&cache, &keys_dir) {
        return Ok(k);
    }
    let store = default_keystore(keys_dir.clone());
    let key = match store.get()? {
        Some(k) => k,
        None if holds_encrypted_data(conn)? => {
            return Err(Error::Encryption(MISSING_KEY.into()));
        }
        None => store.create()?,
    };
    *cache = Some((keys_dir, *key.as_bytes(), key.version));
    Ok(key)
}

/// The stored key if there is one; never creates.
pub fn existing_key(keys_dir: std::path::PathBuf) -> Result<Option<MasterKey>> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = cached(&cache, &keys_dir) {
        return Ok(Some(k));
    }
    let key = default_keystore(keys_dir.clone()).get()?;
    if let Some(k) = &key {
        *cache = Some((keys_dir, *k.as_bytes(), k.version));
    }
    Ok(key)
}

/// Put a key back (restore) and make it the one handed out from now on.
pub fn replace_key(keys_dir: std::path::PathBuf, key: &MasterKey) -> Result<()> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    default_keystore(keys_dir.clone()).put(key)?;
    *cache = Some((keys_dir, *key.as_bytes(), key.version));
    Ok(())
}

/// Forget the key held in memory, so the next request reads the store again.
pub fn forget_cached_key() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The key, read once per run and then held in memory.
///
/// The People screen asks for one face picture per tile, dozens at once. Each
/// asking the Keychain separately raced macOS's access prompt; the losers got
/// an error. One read, under one lock, means at most one prompt.
type Cached = Option<(std::path::PathBuf, [u8; 32], i64)>;
static CACHE: std::sync::Mutex<Cached> = std::sync::Mutex::new(None);

fn cached(cache: &Cached, keys_dir: &std::path::Path) -> Option<MasterKey> {
    match cache {
        Some((dir, bytes, version)) if dir == keys_dir => Some(MasterKey::from_bytes(*bytes, *version)),
        _ => None,
    }
}

fn holds_encrypted_data(conn: &rusqlite::Connection) -> Result<bool> {
    let any = |table: &str| -> Result<bool> {
        Ok(conn
            .query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table})"), [], |r| r.get::<_, bool>(0))?)
    };
    Ok(any("face_embeddings")? || any("face_thumbnails")?)
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
    fn get(&self) -> Result<Option<MasterKey>> {
        use security_framework::passwords::get_generic_password;
        // errSecItemNotFound: the keychain answered and has no such item.
        const ITEM_NOT_FOUND: i32 = -25300;
        match get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => decode_key(&bytes).map(Some),
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(Error::Encryption(format!("keychain read: {e}"))),
        }
    }
    fn create(&self) -> Result<MasterKey> {
        let key = MasterKey::generate(1);
        self.put(&key)?;
        Ok(key)
    }
    fn put(&self, key: &MasterKey) -> Result<()> {
        use security_framework::passwords::set_generic_password;
        set_generic_password(SERVICE, ACCOUNT, key.as_bytes())
            .map_err(|e| Error::Encryption(format!("keychain store: {e}")))
    }
    fn backend_name(&self) -> &'static str {
        "macos-keychain"
    }
}

// ---------------------------------------------------------------------------
// Developer / non-macOS fallback
// ---------------------------------------------------------------------------

/// File-backed key storage.
///
/// The real store on platforms without a Keychain, and the store the tests use
/// everywhere. Compiled on macOS too, deliberately: `default_keystore` ignores
/// the directory it is handed there and returns the Keychain, so a test that
/// passes a temporary directory looks isolated while actually reading and
/// writing the developer's own Keychain — and blocking the whole suite on an
/// authorisation dialog if macOS decides to ask.
pub struct FileKeyStore {
    pub keys_dir: std::path::PathBuf,
}

impl KeyStore for FileKeyStore {
    fn get(&self) -> Result<Option<MasterKey>> {
        let path = self.keys_dir.join("master.key");
        match std::fs::read(&path) {
            Ok(bytes) => decode_key(&bytes).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn create(&self) -> Result<MasterKey> {
        let key = MasterKey::generate(1);
        self.put(&key)?;
        Ok(key)
    }
    fn put(&self, key: &MasterKey) -> Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.keys_dir)?;
        let path = self.keys_dir.join("master.key");
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
        Ok(())
    }
    fn backend_name(&self) -> &'static str {
        "file-fallback-dev"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property that matters: a store returns the same key twice.
    ///
    /// Exercised against `FileKeyStore` rather than `default_keystore`. On
    /// macOS the default is the Keychain, which is the developer's own and
    /// which can block on an authorisation dialog — this test hung the entire
    /// suite for half an hour that way.
    #[test]
    fn keystore_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let ks = FileKeyStore { keys_dir: dir.path().join("keys") };
        let k1 = ks.get_or_create().unwrap();
        let k2 = ks.get_or_create().unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    /// Restore has to be able to put a key back, or a catalogue restored onto
    /// new hardware cannot decrypt its own face data.
    #[test]
    fn a_key_can_be_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let ks = FileKeyStore { keys_dir: dir.path().join("keys") };
        let original = ks.get_or_create().unwrap();

        let replacement = MasterKey::generate(1);
        assert_ne!(original.as_bytes(), replacement.as_bytes());
        ks.put(&replacement).unwrap();

        assert_eq!(ks.get_or_create().unwrap().as_bytes(), replacement.as_bytes());
    }

    /// A catalogue that already holds face data never gets a fresh key: that
    /// would make every existing crop and embedding unreadable.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_missing_key_is_not_replaced_once_faces_exist() {
        let dir = tempfile::tempdir().unwrap();
        let keys = dir.path().join("keys");
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE face_embeddings (x); CREATE TABLE face_thumbnails (x);",
        )
        .unwrap();

        // Empty catalogue: a key is made, and the same one comes back.
        let first = master_key(keys.clone(), &conn).unwrap();
        assert_eq!(master_key(keys.clone(), &conn).unwrap().as_bytes(), first.as_bytes());

        // Face data exists and the key goes missing: refuse, and make nothing.
        conn.execute("INSERT INTO face_embeddings VALUES (1)", []).unwrap();
        std::fs::remove_file(keys.join("master.key")).unwrap();
        forget_cached_key();
        let err = match master_key(keys.clone(), &conn) {
            Ok(_) => panic!("made a new key over existing face data"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("will not make a new one"), "{err}");
        assert!(!keys.join("master.key").exists());
    }

    /// An unreadable key is an error, not "no key".
    #[cfg(unix)]
    #[test]
    fn an_unreadable_key_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let keys = dir.path().join("keys");
        std::fs::create_dir_all(keys.join("master.key")).unwrap(); // a directory: read fails
        let ks = FileKeyStore { keys_dir: keys };
        assert!(ks.get().is_err());
        assert!(ks.get_or_create().is_err());
    }

    /// The key file must not be world-readable.
    #[cfg(unix)]
    #[test]
    fn the_key_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let keys = dir.path().join("keys");
        let ks = FileKeyStore { keys_dir: keys.clone() };
        ks.get_or_create().unwrap();
        let mode = std::fs::metadata(keys.join("master.key")).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "key file is readable by others: {mode:o}");
    }
}
