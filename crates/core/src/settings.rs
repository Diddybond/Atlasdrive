//! Small persisted preferences.
//!
//! Deliberately tiny and deliberately not a database: these are a handful of
//! choices that must survive a restart and that a human might reasonably want
//! to read or fix in a text editor. The catalogue is for catalogue data.
//!
//! Unknown fields are preserved on load and written back, so an older build
//! opening a newer settings file does not silently discard the newer keys.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::AppPaths;
use crate::error::Result;

const FILE: &str = "settings.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    /// Folder backups are written to. Expected to be inside a folder a cloud
    /// client synchronises, but nothing here requires that.
    pub backup_destination: Option<String>,
    /// Whether the master key travels in the bundle. See [`crate::backup`].
    pub backup_include_key: bool,
    /// Snapshots to retain at the destination.
    pub backup_keep: Option<usize>,
    /// Back up automatically once indexing a drive finishes.
    pub backup_after_indexing: bool,
    /// When the last successful backup ran, RFC3339.
    pub last_backup_at: Option<String>,

    /// Anything a newer build wrote that this one does not know about.
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

/// Defaults chosen so that turning backups on requires one decision (where),
/// not five.
pub fn defaults() -> Settings {
    Settings {
        backup_destination: None,
        backup_include_key: true,
        backup_keep: Some(7),
        backup_after_indexing: true,
        last_backup_at: None,
        other: Default::default(),
    }
}

pub fn path(paths: &AppPaths) -> std::path::PathBuf {
    paths.root.join(FILE)
}

/// Load settings, falling back to defaults when absent or unreadable.
///
/// A corrupt settings file must not stop the app opening — the catalogue is
/// what matters, and every one of these has a safe default.
pub fn load(paths: &AppPaths) -> Settings {
    match std::fs::read(path(paths)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| defaults()),
        Err(_) => defaults(),
    }
}

/// Write settings atomically, so a crash mid-write cannot truncate the file.
pub fn save(paths: &AppPaths, settings: &Settings) -> Result<()> {
    std::fs::create_dir_all(&paths.root)?;
    let target = path(paths);
    let tmp = target.with_extension("json.partial");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(&tmp, &target)?;
    Ok(())
}

/// True when `dir` looks like a folder a cloud client synchronises.
///
/// Advisory only — it never blocks a choice. The point is to be able to tell
/// the user "this will leave your Mac" or "this stays on this Mac", because
/// those are very different backups and the difference is not visible from the
/// path alone unless you know what to look for.
pub fn is_cloud_synced(dir: &Path) -> Option<&'static str> {
    let s = dir.to_string_lossy();
    // Google Drive for Desktop mounts under ~/Library/CloudStorage on modern
    // macOS, and used to use ~/Google Drive.
    if s.contains("CloudStorage/GoogleDrive") || s.contains("Google Drive") {
        Some("Google Drive")
    } else if s.contains("CloudStorage/Dropbox") || s.contains("/Dropbox") {
        Some("Dropbox")
    } else if s.contains("Library/Mobile Documents") || s.contains("iCloud Drive") {
        Some("iCloud Drive")
    } else if s.contains("CloudStorage/OneDrive") || s.contains("OneDrive") {
        Some("OneDrive")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        let mut s = defaults();
        s.backup_destination = Some("/somewhere".into());
        s.backup_keep = Some(3);
        save(&paths, &s).unwrap();
        assert_eq!(load(&paths), s);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        assert_eq!(load(&paths), defaults());
        assert!(load(&paths).backup_include_key);
    }

    /// A settings file that has been damaged must not stop the app opening.
    #[test]
    fn corrupt_file_falls_back_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        std::fs::create_dir_all(&paths.root).unwrap();
        std::fs::write(path(&paths), b"{not json at all").unwrap();
        assert_eq!(load(&paths), defaults());
    }

    /// An older build must not silently drop keys a newer one wrote.
    #[test]
    fn unknown_keys_survive_a_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        std::fs::create_dir_all(&paths.root).unwrap();
        std::fs::write(
            path(&paths),
            br#"{"backup_keep":4,"something_from_the_future":{"a":1}}"#,
        )
        .unwrap();

        let loaded = load(&paths);
        assert_eq!(loaded.backup_keep, Some(4));
        save(&paths, &loaded).unwrap();

        let raw = std::fs::read_to_string(path(&paths)).unwrap();
        assert!(
            raw.contains("something_from_the_future"),
            "unknown keys were dropped: {raw}"
        );
    }

    #[test]
    fn recognises_cloud_folders() {
        assert_eq!(
            is_cloud_synced(Path::new(
                "/Users/x/Library/CloudStorage/GoogleDrive-a@b.co.uk/My Drive/AtlasDrive"
            )),
            Some("Google Drive")
        );
        assert_eq!(is_cloud_synced(Path::new("/Users/x/Dropbox/AtlasDrive")), Some("Dropbox"));
        assert_eq!(is_cloud_synced(Path::new("/Volumes/Backup Disk")), None);
    }
}
