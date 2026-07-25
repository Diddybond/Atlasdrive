//! `progress.json` — the human-readable recovery summary written atomically
//! after every batch (see `docs/06_INDEXING_PIPELINE.md`).

use serde::{Deserialize, Serialize};

use crate::config::AppPaths;
use crate::error::Result;
use crate::util::{atomic_write, now_iso8601};

/// Matches the required `progress.json` schema in `docs/06`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "runId")]
    pub run_id: String,
    #[serde(rename = "driveNumber")]
    pub drive_number: i64,
    #[serde(rename = "driveId")]
    pub drive_id: String,
    #[serde(rename = "scanRoot")]
    pub scan_root: String,
    #[serde(rename = "startedAt")]
    pub started_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "filesDiscovered")]
    pub files_discovered: u64,
    #[serde(rename = "filesDone")]
    pub files_done: u64,
    #[serde(rename = "filesFailed")]
    pub files_failed: u64,
    #[serde(rename = "filesQueued")]
    pub files_queued: u64,
    #[serde(rename = "currentBatch")]
    pub current_batch: u64,
    #[serde(rename = "lastCompletedFile")]
    pub last_completed_file: Option<String>,
    #[serde(rename = "consecutiveVerifierFailures")]
    pub consecutive_verifier_failures: u32,
    pub status: String,
}

impl Progress {
    pub fn new(run_id: &str, drive_number: i64, drive_id: &str, scan_root: &str) -> Self {
        let now = now_iso8601();
        Self {
            schema_version: 1,
            run_id: run_id.to_string(),
            drive_number,
            drive_id: drive_id.to_string(),
            scan_root: scan_root.to_string(),
            started_at: now.clone(),
            updated_at: now,
            files_discovered: 0,
            files_done: 0,
            files_failed: 0,
            files_queued: 0,
            current_batch: 0,
            last_completed_file: None,
            consecutive_verifier_failures: 0,
            status: "running".into(),
        }
    }

    /// Atomically persist to the app-data `progress.json`.
    pub fn write(&self, paths: &AppPaths) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self)?;
        atomic_write(&paths.progress_json(), &bytes)
    }

    /// Load an existing `progress.json` if present.
    pub fn load(paths: &AppPaths) -> Result<Option<Progress>> {
        let path = paths.progress_json();
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    pub fn touch(&mut self) {
        self.updated_at = now_iso8601();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        paths.ensure().unwrap();
        let mut p = Progress::new("run1", 14, "drv-uuid", "/Volumes/Example");
        p.files_discovered = 100;
        p.files_done = 20;
        p.current_batch = 2;
        p.write(&paths).unwrap();
        let loaded = Progress::load(&paths).unwrap().unwrap();
        assert_eq!(loaded.files_discovered, 100);
        assert_eq!(loaded.drive_number, 14);
        assert_eq!(loaded.run_id, "run1");
        // Field names use the documented camelCase.
        let text = std::fs::read_to_string(paths.progress_json()).unwrap();
        assert!(text.contains("\"filesDiscovered\""));
        assert!(text.contains("\"driveNumber\""));
    }
}
