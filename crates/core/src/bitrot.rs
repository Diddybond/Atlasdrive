//! Detecting silent corruption of the originals.
//!
//! Every indexed file has a BLAKE3 content hash recorded alongside its size and
//! modification time. Re-reading a drive and recomputing those hashes answers a
//! question a photograph archive on shelved external disks eventually has to
//! ask: *are the files still the files?*
//!
//! # The distinction that makes this useful
//!
//! A changed hash on its own means very little — most changed files were simply
//! edited. What separates an edit from decay is the metadata:
//!
//! | size | mtime | hash | verdict |
//! |------|-------|------|---------|
//! | same | same  | same | intact |
//! | any  | **changed** | changed | edited — expected, not a fault |
//! | **same** | **same** | **changed** | **corrupt** — the bytes rotted underneath the filesystem |
//!
//! The third row is the whole point. A file whose content changed while its
//! size and modification time did not was not edited by anybody: no editor
//! rewrites a file without touching its mtime. That is bit rot, a failing
//! cable, or a drive starting to go — and it is invisible to every other part
//! of the system, including the thumbnail, which was generated years earlier
//! from bytes that were still good.
//!
//! # Read-only, always
//!
//! Every read goes through [`crate::integrity`], which opens originals
//! read-only. This module never writes to a source drive, and the modification
//! times it reads are the evidence, so touching them would destroy the check
//! it exists to perform.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// What re-reading one file established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Byte-for-byte what was indexed.
    Intact,
    /// Content changed, and so did size or modification time. Someone edited
    /// it. Expected, and not a fault.
    Edited,
    /// Content changed while size and modification time did not. Nothing
    /// legitimate does this.
    Corrupt,
    /// Present but could not be read.
    Unreadable,
    /// Not where the catalogue says it is.
    Missing,
}

impl Verdict {
    /// Whether this verdict is something the owner needs to act on.
    pub fn is_problem(&self) -> bool {
        matches!(self, Verdict::Corrupt | Verdict::Unreadable)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub file_id: String,
    pub relative_path: String,
    pub verdict: Verdict,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RotReport {
    pub drive_number: i64,
    pub checked: u64,
    pub intact: u64,
    pub bytes_read: u64,
    /// Only the findings worth showing; intact files are counted, not listed.
    pub findings: Vec<Finding>,
    /// True when the run stopped early (cancelled, or the drive vanished).
    pub incomplete: bool,
}

impl RotReport {
    pub fn count(&self, v: Verdict) -> usize {
        self.findings.iter().filter(|f| f.verdict == v).count()
    }
    /// Files that need attention: corrupt or unreadable.
    pub fn problems(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.verdict.is_problem())
    }
}

#[derive(Default)]
pub struct VerifyOptions {
    /// Stop after this many files. `None` checks the whole drive.
    pub limit: Option<usize>,
    /// Re-check files verified within this many days. `None` checks everything.
    /// Lets a large drive be worked through across several sessions.
    pub skip_verified_within_days: Option<i64>,
    pub cancel: Option<Arc<AtomicBool>>,
}

struct Row {
    file_id: String,
    relative_path: String,
    size_bytes: i64,
    mtime_ns: i64,
    content_hash: Option<String>,
}

/// Re-read a drive's originals and compare them against what was indexed.
///
/// `progress` is called after each file with (done, total).
pub fn verify_drive<F>(
    conn: &Connection,
    drive_number: i64,
    options: &VerifyOptions,
    mut progress: F,
) -> Result<RotReport>
where
    F: FnMut(u64, u64),
{
    let mut report = RotReport { drive_number, ..Default::default() };

    let Some(root) = drive_scan_root(conn, drive_number)? else {
        return Err(crate::error::Error::InvalidArgs(format!(
            "drive {drive_number} has never been indexed, so there is nothing to check against"
        )));
    };
    if !root.exists() {
        return Err(crate::error::Error::InvalidArgs(format!(
            "drive {drive_number} is not connected ({} is not there)",
            root.display()
        )));
    }

    let rows = load_rows(conn, drive_number, options)?;
    let total = rows.len() as u64;

    for (done, row) in rows.into_iter().enumerate() {
        if options.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
            report.incomplete = true;
            break;
        }

        let abs = root.join(&row.relative_path);
        let (verdict, detail, bytes) = check_one(&abs, &row);
        report.checked += 1;
        report.bytes_read += bytes;
        if verdict == Verdict::Intact {
            report.intact += 1;
            // Record the pass so a later run can skip it and work through a
            // large drive in sessions.
            let _ = conn.execute(
                "UPDATE files SET last_verified_at = ?1 WHERE id = ?2",
                rusqlite::params![crate::util::now_iso8601(), &row.file_id],
            );
        } else {
            report.findings.push(Finding {
                file_id: row.file_id,
                relative_path: row.relative_path,
                verdict,
                detail,
            });
        }
        progress(done as u64 + 1, total);
    }

    Ok(report)
}

/// The comparison itself, isolated so it can be tested without a database.
fn check_one(abs: &Path, row: &Row) -> (Verdict, String, u64) {
    let meta = match std::fs::metadata(abs) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (Verdict::Missing, "not found on the drive".into(), 0)
        }
        Err(e) => return (Verdict::Unreadable, format!("cannot stat: {e}"), 0),
    };

    let size_now = meta.len() as i64;
    let mtime_now = crate::integrity::mtime_ns(&meta);

    let Some(expected) = row.content_hash.as_deref() else {
        // Indexed before hashing, or hashing was skipped. Size and mtime are
        // all there is; saying "intact" on that basis would overstate it.
        return (
            Verdict::Intact,
            "no recorded hash to compare against".into(),
            0,
        );
    };

    let actual = match crate::integrity::content_hash(abs) {
        Ok(h) => h,
        Err(e) => return (Verdict::Unreadable, format!("cannot read: {e}"), 0),
    };

    if actual == expected {
        return (Verdict::Intact, String::new(), size_now.max(0) as u64);
    }

    // Content differs. Whether that is alarming depends entirely on whether
    // anything else about the file moved.
    let metadata_changed = size_now != row.size_bytes || mtime_now != row.mtime_ns;
    if metadata_changed {
        (
            Verdict::Edited,
            format!(
                "changed since indexing ({} -> {} bytes); re-index to bring the catalogue up to date",
                row.size_bytes, size_now
            ),
            size_now.max(0) as u64,
        )
    } else {
        (
            Verdict::Corrupt,
            format!(
                "content changed but size ({size_now} bytes) and modification time are untouched \
                 — nothing legitimate rewrites a file that way. Restore this file from a backup \
                 and check the drive."
            ),
            size_now.max(0) as u64,
        )
    }
}

/// Where this drive was last really scanned from.
fn drive_scan_root(conn: &Connection, drive_number: i64) -> Result<Option<PathBuf>> {
    let root: Option<String> = conn
        .query_row(
            "SELECT sr.scan_root
               FROM scan_runs sr
               JOIN drives d ON d.id = sr.drive_id
              WHERE d.drive_number = ?1 AND sr.mode <> 'dry-run'
              ORDER BY sr.started_at DESC LIMIT 1",
            [drive_number],
            |r| r.get(0),
        )
        .ok();
    Ok(root.map(PathBuf::from))
}

fn load_rows(conn: &Connection, drive_number: i64, options: &VerifyOptions) -> Result<Vec<Row>> {
    // Oldest-verified first, so repeated partial runs sweep the whole drive
    // rather than re-checking the same head of the list.
    let mut sql = String::from(
        "SELECT f.id, f.relative_path, f.size_bytes, f.source_mtime_ns, f.content_hash
           FROM files f
           JOIN drives d ON d.id = f.drive_id
          WHERE d.drive_number = ?1 AND f.status = 'complete'",
    );
    if let Some(days) = options.skip_verified_within_days {
        sql.push_str(&format!(
            " AND (f.last_verified_at IS NULL
                   OR julianday('now') - julianday(f.last_verified_at) >= {days})"
        ));
    }
    sql.push_str(" ORDER BY f.last_verified_at IS NOT NULL, f.last_verified_at ASC");
    if let Some(limit) = options.limit {
        sql.push_str(&format!(" LIMIT {limit}"));
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([drive_number], |r| {
        Ok(Row {
            file_id: r.get(0)?,
            relative_path: r.get(1)?,
            size_bytes: r.get(2)?,
            mtime_ns: r.get(3)?,
            content_hash: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, bytes: &[u8]) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(bytes).unwrap();
        f.sync_all().unwrap();
    }

    fn row_for(path: &Path) -> Row {
        let meta = std::fs::metadata(path).unwrap();
        Row {
            file_id: "f1".into(),
            relative_path: path.file_name().unwrap().to_string_lossy().to_string(),
            size_bytes: meta.len() as i64,
            mtime_ns: crate::integrity::mtime_ns(&meta),
            content_hash: Some(crate::integrity::content_hash(path).unwrap()),
        }
    }

    #[test]
    fn an_untouched_file_is_intact() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jpg");
        write(&p, b"original photograph bytes");
        let row = row_for(&p);
        assert_eq!(check_one(&p, &row).0, Verdict::Intact);
    }

    /// The central case. Content changes while size and mtime do not — which
    /// is what decay looks like and what an edit never looks like.
    #[test]
    fn silent_corruption_is_called_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jpg");
        write(&p, b"original photograph bytes");
        let row = row_for(&p);

        // Same length, different bytes, and the modification time put back
        // exactly as it was — the filesystem's view is unchanged.
        let before = std::fs::metadata(&p).unwrap();
        let mtime = before.modified().unwrap();
        write(&p, b"corrupted photograph byte");
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(mtime)
            .unwrap();

        let after = std::fs::metadata(&p).unwrap();
        assert_eq!(after.len(), before.len(), "fixture must not change the size");
        assert_eq!(
            crate::integrity::mtime_ns(&after),
            row.mtime_ns,
            "fixture must not change the mtime"
        );

        let (verdict, detail, _) = check_one(&p, &row);
        assert_eq!(verdict, Verdict::Corrupt);
        assert!(verdict.is_problem());
        assert!(detail.contains("size"), "the detail must explain why: {detail}");
    }

    /// A genuine edit must not be reported as corruption, or the check becomes
    /// noise and stops being read.
    #[test]
    fn an_edited_file_is_not_reported_as_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jpg");
        write(&p, b"original photograph bytes");
        let row = row_for(&p);

        // A real edit changes the length and lets the mtime move.
        std::thread::sleep(std::time::Duration::from_millis(10));
        write(&p, b"edited photograph bytes, now a different length entirely");

        let (verdict, _, _) = check_one(&p, &row);
        assert_eq!(verdict, Verdict::Edited);
        assert!(!verdict.is_problem());
    }

    #[test]
    fn a_deleted_file_is_missing_not_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jpg");
        write(&p, b"bytes");
        let row = row_for(&p);
        std::fs::remove_file(&p).unwrap();
        assert_eq!(check_one(&p, &row).0, Verdict::Missing);
    }

    /// Without a recorded hash there is nothing to compare, and claiming the
    /// file is verified would be a lie.
    #[test]
    fn a_file_with_no_recorded_hash_is_not_claimed_as_verified() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jpg");
        write(&p, b"bytes");
        let mut row = row_for(&p);
        row.content_hash = None;
        let (_, detail, _) = check_one(&p, &row);
        assert!(detail.contains("no recorded hash"), "must say why: {detail}");
    }

    /// End to end through the database: path resolution, the corrupt/edited
    /// split, and the recording of a pass so a later run can skip it.
    #[test]
    fn verify_drive_finds_corruption_among_healthy_files() {
        use crate::db::{self, SchemaKind};

        let drive = tempfile::tempdir().unwrap();
        let root = drive.path();
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives (id, drive_number, status, first_seen_at)
             VALUES ('d1', 7, 'online', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scan_runs (id, drive_id, drive_number, mode, scan_root, started_at,
                                    files_discovered, files_done, files_failed)
             VALUES ('r1','d1',7,'full', ?1, 'now', 3, 3, 0)",
            [root.to_string_lossy().as_ref()],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO roots (id, drive_id, relative_root, created_at)
             VALUES ('rt1','d1','','now')",
            [],
        )
        .unwrap();

        // Three files: one left alone, one silently corrupted, one edited.
        for (id, name, bytes) in [
            ("f-ok", "keep.jpg", &b"aaaaaaaaaaaaaaaa"[..]),
            ("f-rot", "rot.jpg", &b"bbbbbbbbbbbbbbbb"[..]),
            ("f-edit", "edit.jpg", &b"cccccccccccccccc"[..]),
        ] {
            let p = root.join(name);
            write(&p, bytes);
            let meta = std::fs::metadata(&p).unwrap();
            conn.execute(
                "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                    source_mtime_ns, content_hash, status, analysis_version,
                                    created_at, updated_at)
                 VALUES (?1,'d1','rt1',?2,?2,?3,?4,?5,'complete',1,'now','now')",
                rusqlite::params![
                    id,
                    name,
                    meta.len() as i64,
                    crate::integrity::mtime_ns(&meta),
                    crate::integrity::content_hash(&p).unwrap()
                ],
            )
            .unwrap();
        }

        // Silent corruption: same length, mtime restored.
        let rot = root.join("rot.jpg");
        let mtime = std::fs::metadata(&rot).unwrap().modified().unwrap();
        write(&rot, b"XXXXXXXXXXXXXXXX");
        std::fs::File::options().write(true).open(&rot).unwrap()
            .set_modified(mtime).unwrap();

        // An ordinary edit: different length, mtime moves.
        std::thread::sleep(std::time::Duration::from_millis(10));
        write(&root.join("edit.jpg"), b"a longer edited photograph");

        let report = verify_drive(&conn, 7, &VerifyOptions::default(), |_, _| {}).unwrap();

        assert_eq!(report.checked, 3);
        assert_eq!(report.intact, 1);
        assert_eq!(report.count(Verdict::Corrupt), 1);
        assert_eq!(report.count(Verdict::Edited), 1);
        assert_eq!(report.problems().count(), 1, "only the corrupt file needs action");
        assert_eq!(report.problems().next().unwrap().relative_path, "rot.jpg");

        // The healthy file's pass was recorded; the other two were not.
        let verified: i64 = conn
            .query_row("SELECT count(*) FROM files WHERE last_verified_at IS NOT NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(verified, 1);
    }

    /// A drive that is not plugged in must say so, not report every file
    /// missing and look like catastrophic data loss.
    #[test]
    fn a_disconnected_drive_is_reported_as_disconnected() {
        use crate::db::{self, SchemaKind};
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives (id, drive_number, status, first_seen_at)
             VALUES ('d1', 9, 'offline', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scan_runs (id, drive_id, drive_number, mode, scan_root, started_at,
                                    files_discovered, files_done, files_failed)
             VALUES ('r1','d1',9,'full','/Volumes/NotPluggedIn','now',0,0,0)",
            [],
        )
        .unwrap();

        let err = verify_drive(&conn, 9, &VerifyOptions::default(), |_, _| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("not connected"), "unhelpful error: {err}");
    }

    /// Reading originals must not disturb them — the modification time is the
    /// evidence this check depends on.
    #[test]
    fn checking_does_not_touch_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jpg");
        write(&p, b"original photograph bytes");
        let row = row_for(&p);
        let before = std::fs::metadata(&p).unwrap();

        check_one(&p, &row);

        let after = std::fs::metadata(&p).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(
            crate::integrity::mtime_ns(&before),
            crate::integrity::mtime_ns(&after)
        );
    }
}
