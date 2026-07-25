//! The app-owned drive identity manifest (`.atlasdrive/drive.json`).
//!
//! Written atomically via a temp file + rename, and only ever inside the
//! app-owned hidden folder — never elsewhere on the drive (see `docs/05`).
//! The manifest aids recognition but is never the sole source of truth.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::scan::APP_MANIFEST_DIR;
use crate::util::{atomic_write, now_iso8601};

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const APP_ID: &str = "atlasdrive";

/// Contents of `.atlasdrive/drive.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DriveManifest {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "driveId")]
    pub drive_id: String,
    #[serde(rename = "driveNumber")]
    pub drive_number: i64,
    #[serde(rename = "friendlyName")]
    pub friendly_name: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "lastSuccessfulScanAt")]
    pub last_successful_scan_at: Option<String>,
    #[serde(rename = "appId")]
    pub app_id: String,
}

impl DriveManifest {
    pub fn new(drive_id: &str, drive_number: i64, friendly_name: Option<String>) -> Self {
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            drive_id: drive_id.to_string(),
            drive_number,
            friendly_name,
            created_at: now_iso8601(),
            last_successful_scan_at: None,
            app_id: APP_ID.to_string(),
        }
    }

    /// Path to the manifest folder for a given mounted volume root.
    pub fn manifest_dir(volume_root: &Path) -> PathBuf {
        volume_root.join(APP_MANIFEST_DIR)
    }

    pub fn manifest_path(volume_root: &Path) -> PathBuf {
        Self::manifest_dir(volume_root).join("drive.json")
    }

    /// Write the manifest to the volume's app-owned folder, atomically.
    ///
    /// This is the *only* write the application makes to a drive, and it is
    /// confined to the hidden `.atlasdrive` folder.
    pub fn write_to_volume(&self, volume_root: &Path) -> Result<PathBuf> {
        let dir = Self::manifest_dir(volume_root);
        std::fs::create_dir_all(&dir)?;
        let path = Self::manifest_path(volume_root);
        let bytes = serde_json::to_vec_pretty(self)?;
        atomic_write(&path, &bytes)?;
        Ok(path)
    }

    /// Read a manifest from a volume if present.
    pub fn read_from_volume(volume_root: &Path) -> Result<Option<DriveManifest>> {
        let path = Self::manifest_path(volume_root);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)?;
        let m: DriveManifest = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Other(format!("invalid drive manifest: {e}")))?;
        if m.app_id != APP_ID {
            return Err(Error::Other("manifest app_id mismatch".into()));
        }
        Ok(Some(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let vol = dir.path();
        let m = DriveManifest::new("uuid-1", 14, Some("Family A".into()));
        let path = m.write_to_volume(vol).unwrap();
        assert!(path.ends_with(".atlasdrive/drive.json"));
        let read = DriveManifest::read_from_volume(vol).unwrap().unwrap();
        assert_eq!(read, m);
        assert_eq!(read.drive_number, 14);
    }

    #[test]
    fn missing_manifest_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(DriveManifest::read_from_volume(dir.path()).unwrap().is_none());
    }

    #[test]
    fn manifest_only_touches_hidden_folder() {
        let dir = tempfile::tempdir().unwrap();
        let vol = dir.path();
        // Pre-existing user content must remain untouched.
        std::fs::write(vol.join("family.jpg"), b"original").unwrap();
        let m = DriveManifest::new("uuid-1", 3, None);
        m.write_to_volume(vol).unwrap();
        assert_eq!(std::fs::read(vol.join("family.jpg")).unwrap(), b"original");
        // Only .atlasdrive was added besides the original.
        let entries: Vec<_> = std::fs::read_dir(vol)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(entries.contains(&".atlasdrive".to_string()));
        assert!(entries.contains(&"family.jpg".to_string()));
        assert_eq!(entries.len(), 2);
    }
}
