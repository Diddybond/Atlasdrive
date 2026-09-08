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

    /// Minutes since this was last written, or `None` if the timestamp is
    /// unreadable.
    ///
    /// Unreadable is deliberately not "old": a timestamp that cannot be parsed
    /// says nothing about whether the scan is alive, and treating it as stale
    /// would put a scary label on a working run.
    pub fn age_minutes(&self) -> Option<i64> {
        let then = chrono::DateTime::parse_from_rfc3339(&self.updated_at).ok()?;
        Some((chrono::Utc::now() - then.with_timezone(&chrono::Utc)).num_minutes())
    }

    /// The status to *show*, which is not always the status on disk.
    ///
    /// `progress.json` is written by the run itself, so it can only ever record
    /// what the run knew before it stopped knowing anything. Two states are
    /// therefore never written and have to be worked out by whoever reads the
    /// file:
    ///
    /// * **interrupted** — the file says "running" and the caller can see that
    ///   no run is in flight. A run that was killed rather than cancelled never
    ///   got to write anything else.
    /// * **stalled** — the file says "running", something may well be in
    ///   flight, but nothing has been written for [`STALL_AFTER_MINUTES`].
    ///
    /// `in_flight` is what the caller knows about a run in this process:
    /// `Some(false)` means "certainly nothing running", `None` means "cannot
    /// tell" — which is the honest answer from the command line, where the scan
    /// may belong to another process entirely.
    ///
    /// This lives here rather than in the app because both need it and they
    /// must not disagree (D-049). The desktop app had the rule; the CLI and the
    /// verifier did not, so the one route the owner is told to use for recovery
    /// was also the one that would repeat "running" about a scan that died two
    /// days ago.
    pub fn reconciled_status(&self, in_flight: Option<bool>) -> String {
        // Anything the run itself wrote is the truth and is passed through
        // unchanged — including a status this code has never heard of.
        if self.status != "running" {
            return self.status.clone();
        }
        if in_flight == Some(false) {
            return "interrupted".to_string();
        }
        match self.age_minutes() {
            Some(m) if m >= STALL_AFTER_MINUTES => "stalled".to_string(),
            _ => "running".to_string(),
        }
    }
}

/// How long a "running" scan may go without writing before it is called
/// stalled.
///
/// Progress is written after every photograph — successes and failures alike —
/// so silence is not slowness. The slowest single photograph is bounded by the
/// decode and Vision budgets at ten minutes each, so half an hour is past any
/// honest file and well short of the two days a real scan sat frozen while the
/// screen said it was running.
pub const STALL_AFTER_MINUTES: i64 = 30;

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

    /// Backdate the last write by `minutes`, as a scan that has gone quiet does.
    fn quiet_for(minutes: i64) -> Progress {
        let mut p = Progress::new("run1", 5, "drv", "/Volumes/Drive 5");
        p.updated_at = (chrono::Utc::now() - chrono::Duration::minutes(minutes))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        p
    }

    #[test]
    fn a_scan_writing_regularly_is_running() {
        let p = quiet_for(1);
        assert_eq!(p.reconciled_status(Some(true)), "running");
        assert_eq!(p.reconciled_status(None), "running");
    }

    /// The two days a real scan spent saying "running" with nothing being read.
    #[test]
    fn a_scan_silent_for_half_an_hour_is_stalled() {
        let p = quiet_for(STALL_AFTER_MINUTES + 1);
        assert_eq!(p.reconciled_status(Some(true)), "stalled");
        assert_eq!(
            p.reconciled_status(None),
            "stalled",
            "the command line cannot see a run in flight, and must still say so"
        );
    }

    /// A run that was killed never got to write anything but "running".
    #[test]
    fn a_run_that_is_not_in_flight_was_interrupted_however_recent_it_looks() {
        let p = quiet_for(0);
        assert_eq!(p.reconciled_status(Some(false)), "interrupted");
    }

    /// What the run wrote for itself is the truth, and is never overruled.
    #[test]
    fn a_finished_run_is_left_alone() {
        for status in ["complete", "halted", "interrupted"] {
            let mut p = quiet_for(60 * 24);
            p.status = status.to_string();
            assert_eq!(p.reconciled_status(Some(false)), status);
            assert_eq!(p.reconciled_status(None), status);
        }
    }

    /// An unreadable timestamp says nothing about liveness, so it must not be
    /// read as "old" — that would label a working scan stalled.
    #[test]
    fn an_unreadable_timestamp_is_not_evidence_of_a_stall() {
        let mut p = Progress::new("run1", 5, "drv", "/Volumes/Drive 5");
        p.updated_at = "not a timestamp".to_string();
        assert_eq!(p.age_minutes(), None);
        assert_eq!(p.reconciled_status(None), "running");
    }
}
