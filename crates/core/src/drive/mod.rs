//! Drive registration, recognition and conflict handling (see `docs/05`).
//!
//! Dual identity: an internal UUID the app owns, and a user-assigned physical
//! drive number printed on the drive. Recognition combines several signals and
//! never resolves an identity conflict silently.

pub mod manifest;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::util::{new_uuid, now_iso8601};

pub use manifest::DriveManifest;

/// A catalogued drive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drive {
    pub id: String,
    pub drive_number: i64,
    pub friendly_name: Option<String>,
    pub volume_uuid: Option<String>,
    pub volume_name: Option<String>,
    pub capacity_bytes: Option<i64>,
    pub filesystem_type: Option<String>,
    pub physical_location: Option<String>,
    pub categories: Vec<String>,
    pub status: String,
    pub first_seen_at: String,
    pub last_seen_at: Option<String>,
    pub last_scan_at: Option<String>,
}

/// Parameters for registering a new drive.
#[derive(Debug, Clone, Default)]
pub struct RegisterParams {
    pub drive_number: i64,
    pub friendly_name: Option<String>,
    pub volume_uuid: Option<String>,
    pub volume_name: Option<String>,
    pub capacity_bytes: Option<i64>,
    pub filesystem_type: Option<String>,
    pub physical_location: Option<String>,
    pub categories: Vec<String>,
    /// The folder chosen to index. Kept because otherwise the only record of
    /// what to scan is `scan_runs.scan_root`, which does not exist until a scan
    /// has run — leaving a freshly registered drive with nothing to scan.
    pub registered_root: Option<String>,
}

/// Recognition outcome when a drive is inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recognition {
    /// Matched an existing drive with confidence in [0,1] scaled to 0..100.
    Known { drive_id: String, drive_number: i64, score: u32 },
    /// Manifest UUID seen on a *different* physical volume: conflict.
    Conflict { drive_id: String, reason: String },
    /// No confident match; secondary signals may suggest candidates.
    Unknown { candidates: Vec<String> },
}

/// Repository over `archive.db` for drive operations.
pub struct DriveRepo<'a> {
    conn: &'a Connection,
}

impl<'a> DriveRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Register a new drive with a unique physical number and fresh UUID.
    pub fn register(&self, p: &RegisterParams) -> Result<Drive> {
        if p.drive_number <= 0 {
            return Err(Error::InvalidArgs(
                "drive number must be a positive integer".into(),
            ));
        }
        // Enforce uniqueness with a clear error (not a raw SQLite failure).
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT drive_number FROM drives WHERE drive_number = ?1",
                [p.drive_number],
                |r| r.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Err(Error::InvalidArgs(format!(
                "drive number {} is already in use",
                p.drive_number
            )));
        }

        let id = new_uuid();
        let now = now_iso8601();
        let categories = serde_json::to_string(&p.categories)?;
        self.conn.execute(
            "INSERT INTO drives
             (id, drive_number, friendly_name, volume_uuid, volume_name, capacity_bytes,
              filesystem_type, physical_location, categories, status, first_seen_at,
              registered_root)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'offline',?10,?11)",
            params![
                id,
                p.drive_number,
                p.friendly_name,
                p.volume_uuid,
                p.volume_name,
                p.capacity_bytes,
                p.filesystem_type,
                p.physical_location,
                categories,
                now,
                p.registered_root,
            ],
        )?;
        self.audit(&id, "registered", None)?;

        // Store initial recognition fingerprints.
        if let Some(vu) = &p.volume_uuid {
            self.add_fingerprint(&id, "volume_uuid", vu)?;
        }
        if let Some(cap) = p.capacity_bytes {
            self.add_fingerprint(&id, "capacity", &cap.to_string())?;
        }

        self.get(&id).map(|d| d.expect("just inserted"))
    }

    /// Recognize a drive from a manifest and secondary signals.
    ///
    /// `connected_volume_uuid` is the *current* volume UUID of the physically
    /// connected drive, used to detect a manifest copied onto another volume.
    pub fn recognize(
        &self,
        manifest: Option<&DriveManifest>,
        connected_volume_uuid: Option<&str>,
        capacity_bytes: Option<i64>,
    ) -> Result<Recognition> {
        if let Some(m) = manifest {
            if let Some(drive) = self.get(&m.drive_id)? {
                // Manifest UUID matches a known drive. Check the volume UUID to
                // detect a clone/copy onto a different physical volume.
                if let (Some(stored), Some(current)) = (&drive.volume_uuid, connected_volume_uuid) {
                    if stored != current {
                        return Ok(Recognition::Conflict {
                            drive_id: drive.id.clone(),
                            reason: format!(
                                "manifest UUID {} found on volume {} but was registered on {}",
                                m.drive_id, current, stored
                            ),
                        });
                    }
                }
                let mut score = 60u32; // manifest UUID match
                if drive.capacity_bytes == capacity_bytes && capacity_bytes.is_some() {
                    score += 20;
                }
                if drive.volume_uuid.as_deref() == connected_volume_uuid
                    && connected_volume_uuid.is_some()
                {
                    score += 20;
                }
                return Ok(Recognition::Known {
                    drive_id: drive.id,
                    drive_number: drive.drive_number,
                    score,
                });
            }
        }

        // No manifest match: try secondary signals (volume UUID, capacity).
        let mut candidates = Vec::new();
        if let Some(vu) = connected_volume_uuid {
            let mut stmt = self.conn.prepare(
                "SELECT drive_id FROM drive_fingerprints WHERE kind='volume_uuid' AND value=?1",
            )?;
            let rows = stmt
                .query_map([vu], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            candidates.extend(rows);
        }
        Ok(Recognition::Unknown { candidates })
    }

    /// Mark a drive's online/offline/changed/conflict/retired status.
    pub fn set_status(&self, drive_id: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE drives SET status=?2, last_seen_at=?3 WHERE id=?1",
            params![drive_id, status, now_iso8601()],
        )?;
        Ok(())
    }

    /// Update where a drive physically lives and how it is categorised.
    ///
    /// These are the user's own notes about a piece of hardware — "Drawer 2",
    /// "scanned prints" — and they are the only way to find the disk in the real
    /// world once the app says which drive number holds a photograph. Each field
    /// is independently optional so updating one never blanks the other, and the
    /// change is audited like any other drive edit.
    pub fn update_details(
        &self,
        drive_id: &str,
        physical_location: Option<&str>,
        categories: Option<&[String]>,
    ) -> Result<()> {
        if let Some(loc) = physical_location {
            let trimmed = loc.trim();
            self.conn.execute(
                "UPDATE drives SET physical_location=?2 WHERE id=?1",
                params![drive_id, (!trimmed.is_empty()).then_some(trimmed)],
            )?;
        }
        if let Some(cats) = categories {
            // Normalise: trimmed, non-empty, de-duplicated, order preserved.
            let mut seen = std::collections::HashSet::new();
            let cleaned: Vec<String> = cats
                .iter()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty() && seen.insert(c.to_lowercase()))
                .collect();
            self.conn.execute(
                "UPDATE drives SET categories=?2 WHERE id=?1",
                params![drive_id, serde_json::to_string(&cleaned)?],
            )?;
        }
        self.audit(drive_id, "details_updated", None)?;
        Ok(())
    }

    /// Rename a drive, keeping its number and identity.
    ///
    /// The friendly name is a label for the user's benefit; the drive number and
    /// internal id are what the catalogue keys on (D-004), so renaming is always
    /// safe and never invalidates indexed photographs. An empty name clears it.
    pub fn rename(&self, drive_id: &str, friendly_name: &str) -> Result<()> {
        let trimmed = friendly_name.trim();
        self.conn.execute(
            "UPDATE drives SET friendly_name=?2 WHERE id=?1",
            params![drive_id, (!trimmed.is_empty()).then_some(trimmed)],
        )?;
        self.audit(drive_id, "renamed", None)?;
        Ok(())
    }

    /// Renumber a drive, preserving history in the audit table.
    pub fn renumber(&self, drive_id: &str, new_number: i64) -> Result<()> {
        if new_number <= 0 {
            return Err(Error::InvalidArgs("drive number must be positive".into()));
        }
        let taken: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM drives WHERE drive_number=?1 AND id<>?2",
                params![new_number, drive_id],
                |r| r.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(Error::InvalidArgs(format!(
                "drive number {new_number} already in use"
            )));
        }
        let old: i64 = self.conn.query_row(
            "SELECT drive_number FROM drives WHERE id=?1",
            [drive_id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "UPDATE drives SET drive_number=?2 WHERE id=?1",
            params![drive_id, new_number],
        )?;
        self.audit(
            drive_id,
            "renumbered",
            Some(serde_json::json!({"from": old, "to": new_number})),
        )?;
        Ok(())
    }

    /// Declare a drive to be a backup/clone/replacement of another.
    pub fn set_backup_relationship(
        &self,
        drive_id: &str,
        of_drive_id: &str,
        relationship: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE drives SET backup_of_drive_id=?2, backup_relationship=?3 WHERE id=?1",
            params![drive_id, of_drive_id, relationship],
        )?;
        self.audit(
            drive_id,
            "backup_relationship",
            Some(serde_json::json!({"of": of_drive_id, "relationship": relationship})),
        )?;
        Ok(())
    }

    pub fn add_fingerprint(&self, drive_id: &str, kind: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO drive_fingerprints (id, drive_id, kind, value, captured_at)
             VALUES (?1,?2,?3,?4,?5)",
            params![new_uuid(), drive_id, kind, value, now_iso8601()],
        )?;
        Ok(())
    }

    pub fn audit(&self, drive_id: &str, event: &str, detail: Option<serde_json::Value>) -> Result<()> {
        let detail_str = detail.map(|d| d.to_string());
        self.conn.execute(
            "INSERT INTO drive_audit (id, drive_id, event, detail, at) VALUES (?1,?2,?3,?4,?5)",
            params![new_uuid(), drive_id, event, detail_str, now_iso8601()],
        )?;
        Ok(())
    }

    /// Ensure a root row exists for a drive; returns its id.
    pub fn ensure_root(&self, drive_id: &str, relative_root: &str) -> Result<String> {
        if let Some(id) = self
            .conn
            .query_row(
                "SELECT id FROM roots WHERE drive_id=?1 AND relative_root=?2",
                params![drive_id, relative_root],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(id);
        }
        let id = new_uuid();
        self.conn.execute(
            "INSERT INTO roots (id, drive_id, relative_root, created_at) VALUES (?1,?2,?3,?4)",
            params![id, drive_id, relative_root, now_iso8601()],
        )?;
        Ok(id)
    }

    pub fn get(&self, id: &str) -> Result<Option<Drive>> {
        self.query_one("SELECT * FROM drives WHERE id=?1", params![id])
    }

    pub fn get_by_number(&self, number: i64) -> Result<Option<Drive>> {
        self.query_one("SELECT * FROM drives WHERE drive_number=?1", params![number])
    }

    /// Every registered drive, with `status` reflecting what is plugged in now.
    ///
    /// The stored `status` records what was true during the last scan, which is
    /// a different question from whether the drive is connected at this moment:
    /// a drive registered and never scanned stays "offline" while plugged in,
    /// and one unplugged after a scan stays "online". Resolving it here rather
    /// than in each caller is deliberate — the same rule implemented twice is
    /// how the interface came to show a mounted drive as DISCONNECTED while the
    /// command line showed something else again.
    ///
    /// `retired` and `conflict` are left alone: those are judgements about a
    /// drive, not observations about a cable.
    pub fn list(&self) -> Result<Vec<Drive>> {
        let mut drives = self.list_as_recorded()?;
        let mounted = crate::volumes::mounted_drive_numbers(self.conn);
        for d in &mut drives {
            if d.status == "online" || d.status == "offline" {
                d.status = if mounted.contains(&d.drive_number) {
                    "online".to_string()
                } else {
                    "offline".to_string()
                };
            }
        }
        Ok(drives)
    }

    /// The folder chosen when this drive was registered, if it is still known.
    pub fn registered_root(&self, drive_number: i64) -> Option<String> {
        self.conn
            .query_row(
                "SELECT registered_root FROM drives WHERE drive_number = ?1",
                [drive_number],
                |r| r.get(0),
            )
            .ok()
            .flatten()
    }

    /// Drives exactly as stored, without consulting what is mounted.
    ///
    /// For callers that need the recorded state rather than the live one —
    /// and for [`crate::volumes`], which must not recurse back into `list`.
    pub fn list_as_recorded(&self) -> Result<Vec<Drive>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM drives ORDER BY drive_number")?;
        let rows = stmt.query_map([], Self::row_to_drive)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Remove a drive catalogue (leaves the physical drive untouched).
    pub fn remove_catalogue(&self, drive_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM drives WHERE id=?1", [drive_id])?;
        Ok(())
    }

    fn query_one(&self, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Option<Drive>> {
        let mut stmt = self.conn.prepare(sql)?;
        let d = stmt.query_row(p, Self::row_to_drive).optional()?;
        Ok(d)
    }

    fn row_to_drive(r: &rusqlite::Row) -> rusqlite::Result<Drive> {
        let categories: Option<String> = r.get("categories")?;
        let categories = categories
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Ok(Drive {
            id: r.get("id")?,
            drive_number: r.get("drive_number")?,
            friendly_name: r.get("friendly_name")?,
            volume_uuid: r.get("volume_uuid")?,
            volume_name: r.get("volume_name")?,
            capacity_bytes: r.get("capacity_bytes")?,
            filesystem_type: r.get("filesystem_type")?,
            physical_location: r.get("physical_location")?,
            categories,
            status: r.get("status")?,
            first_seen_at: r.get("first_seen_at")?,
            last_seen_at: r.get("last_seen_at")?,
            last_scan_at: r.get("last_scan_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, SchemaKind};

    fn repo_conn() -> Connection {
        open_in_memory(SchemaKind::Archive).unwrap()
    }

    #[test]
    fn physical_location_and_categories_round_trip_and_can_be_edited() {
        let conn = repo_conn();
        let repo = DriveRepo::new(&conn);
        let d = repo
            .register(&RegisterParams {
                drive_number: 14,
                friendly_name: Some("Family Archive A".into()),
                physical_location: Some("Studio shelf B".into()),
                categories: vec!["family".into(), "scans".into()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(d.physical_location.as_deref(), Some("Studio shelf B"));
        assert_eq!(d.categories, vec!["family", "scans"]);

        // Reading it back preserves both.
        let read = repo.get_by_number(14).unwrap().unwrap();
        assert_eq!(read.physical_location.as_deref(), Some("Studio shelf B"));
        assert_eq!(read.categories, vec!["family", "scans"]);

        // Moving the drive updates only the location.
        repo.update_details(&d.id, Some("Drawer 2"), None).unwrap();
        let moved = repo.get_by_number(14).unwrap().unwrap();
        assert_eq!(moved.physical_location.as_deref(), Some("Drawer 2"));
        assert_eq!(moved.categories, vec!["family", "scans"], "categories untouched");

        // Recategorising updates only the categories, and normalises them.
        repo.update_details(
            &d.id,
            None,
            Some(&[
                "  Holidays ".into(),
                "holidays".into(), // duplicate, different case
                "".into(),         // blank
                "negatives".into(),
            ]),
        )
        .unwrap();
        let recategorised = repo.get_by_number(14).unwrap().unwrap();
        assert_eq!(recategorised.categories, vec!["Holidays", "negatives"]);
        assert_eq!(
            recategorised.physical_location.as_deref(),
            Some("Drawer 2"),
            "location untouched"
        );

        // Clearing the location is possible, and is distinct from leaving it be.
        repo.update_details(&d.id, Some("  "), None).unwrap();
        assert!(repo.get_by_number(14).unwrap().unwrap().physical_location.is_none());

        // Every edit is audited.
        let audits: i64 = conn
            .query_row(
                "SELECT count(*) FROM drive_audit WHERE event='details_updated'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(audits, 3);
    }

    #[test]
    fn register_and_unique_number() {
        let conn = repo_conn();
        let repo = DriveRepo::new(&conn);
        let d = repo
            .register(&RegisterParams {
                drive_number: 14,
                friendly_name: Some("Family A".into()),
                volume_uuid: Some("VOL-1".into()),
                capacity_bytes: Some(1000),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(d.drive_number, 14);
        assert!(!d.id.is_empty());
        // Duplicate number rejected.
        let dup = repo.register(&RegisterParams {
            drive_number: 14,
            ..Default::default()
        });
        assert!(dup.is_err());
        // Non-positive rejected.
        assert!(repo
            .register(&RegisterParams { drive_number: 0, ..Default::default() })
            .is_err());
    }

    #[test]
    fn recognize_known_and_conflict() {
        let conn = repo_conn();
        let repo = DriveRepo::new(&conn);
        let d = repo
            .register(&RegisterParams {
                drive_number: 7,
                volume_uuid: Some("VOL-7".into()),
                capacity_bytes: Some(500),
                ..Default::default()
            })
            .unwrap();
        let m = DriveManifest::new(&d.id, 7, None);

        // Same volume → known.
        let rec = repo.recognize(Some(&m), Some("VOL-7"), Some(500)).unwrap();
        match rec {
            Recognition::Known { drive_number, score, .. } => {
                assert_eq!(drive_number, 7);
                assert!(score >= 60);
            }
            _ => panic!("expected known"),
        }

        // Manifest on a different volume → conflict.
        let rec = repo.recognize(Some(&m), Some("VOL-OTHER"), Some(500)).unwrap();
        assert!(matches!(rec, Recognition::Conflict { .. }));
    }

    #[test]
    fn renumber_preserves_and_rejects_conflict() {
        let conn = repo_conn();
        let repo = DriveRepo::new(&conn);
        let a = repo.register(&RegisterParams { drive_number: 1, ..Default::default() }).unwrap();
        let _b = repo.register(&RegisterParams { drive_number: 2, ..Default::default() }).unwrap();
        assert!(repo.renumber(&a.id, 2).is_err()); // taken
        repo.renumber(&a.id, 5).unwrap();
        assert_eq!(repo.get(&a.id).unwrap().unwrap().drive_number, 5);
    }
}
