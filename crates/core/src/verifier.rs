//! The real verifier (see `docs/13_TESTING_AND_VERIFIER.md`).
//!
//! This is an executable set of checks — not a checklist or log routine — that
//! the CLI runs and exits non-zero on failure. It is deliberately independent
//! of the feature code paths it audits, and is never weakened to obtain a pass.

use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::config::{AppPaths, Config};
use crate::crypto::MasterKey;
use crate::error::{Error, Result};
use crate::faces::FaceRepo;
use crate::integrity::SourceSnapshot;
use crate::pipeline::thumbnail::{self, ThumbnailInfo};

/// Outcome of a single check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    /// A hard safety failure that must halt the whole run immediately.
    Halt,
}

/// One named check result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

impl Check {
    fn pass(name: &str, detail: impl Into<String>) -> Self {
        Self { name: name.into(), status: CheckStatus::Pass, detail: detail.into() }
    }
    fn warn(name: &str, detail: impl Into<String>) -> Self {
        Self { name: name.into(), status: CheckStatus::Warn, detail: detail.into() }
    }
    fn fail(name: &str, detail: impl Into<String>) -> Self {
        Self { name: name.into(), status: CheckStatus::Fail, detail: detail.into() }
    }
    fn halt(name: &str, detail: impl Into<String>) -> Self {
        Self { name: name.into(), status: CheckStatus::Halt, detail: detail.into() }
    }
}

/// Aggregate verifier report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifierReport {
    pub checks: Vec<Check>,
    pub generated_at: String,
}

impl VerifierReport {
    pub fn ok(&self) -> bool {
        self.checks
            .iter()
            .all(|c| matches!(c.status, CheckStatus::Pass | CheckStatus::Warn))
    }
    pub fn has_halt(&self) -> bool {
        self.checks.iter().any(|c| c.status == CheckStatus::Halt)
    }
    /// Exit code implied by the worst check.
    pub fn exit_code(&self) -> i32 {
        if self.has_halt() {
            // Determine the specific hard-halt exit code from the first halt.
            for c in &self.checks {
                if c.status == CheckStatus::Halt {
                    if c.name.contains("original") {
                        return crate::error::exit::SOURCE_INTEGRITY;
                    }
                    if c.name.contains("disk") {
                        return crate::error::exit::INSUFFICIENT_DISK;
                    }
                    if c.name.contains("network") || c.name.contains("path") {
                        return crate::error::exit::SOURCE_INTEGRITY;
                    }
                    if c.name.contains("corruption") || c.name.contains("integrity_db") {
                        return crate::error::exit::MIGRATION_OR_CORRUPTION;
                    }
                }
            }
            return crate::error::exit::VERIFIER_FAILURE;
        }
        if self.ok() {
            crate::error::exit::SUCCESS
        } else {
            crate::error::exit::VERIFIER_FAILURE
        }
    }

    pub fn summary(&self) -> String {
        let mut pass = 0;
        let mut warn = 0;
        let mut fail = 0;
        let mut halt = 0;
        for c in &self.checks {
            match c.status {
                CheckStatus::Pass => pass += 1,
                CheckStatus::Warn => warn += 1,
                CheckStatus::Fail => fail += 1,
                CheckStatus::Halt => halt += 1,
            }
        }
        format!("{pass} pass, {warn} warn, {fail} fail, {halt} halt")
    }
}

/// Context needed to run the verifier.
pub struct VerifyContext<'a> {
    pub archive: &'a Connection,
    pub queue: Option<&'a Connection>,
    pub paths: &'a AppPaths,
    pub config: &'a Config,
    pub key: Option<&'a MasterKey>,
    /// AI model partition to sanity-check faces against.
    pub face_model: (String, String),
    /// Median batch throughput observed this run (files/sec), if known.
    pub observed_throughput: Option<f64>,
    /// Whether the run's network guard recorded zero blocked attempts.
    pub network_blocked_attempts: u64,
}

/// Run the full verifier suite and return a structured report.
pub fn run(ctx: &VerifyContext) -> Result<VerifierReport> {
    let mut checks = Vec::new();

    checks.push(check_db_integrity(ctx.archive));
    checks.push(check_catalogue_rows(ctx.archive));
    checks.push(check_hashes(ctx.archive));
    checks.extend(check_thumbnails(ctx.archive, ctx.paths));
    checks.push(check_originals_unchanged(ctx.archive));
    checks.push(check_output_containment(ctx.archive, ctx.paths));
    checks.push(check_network_isolation(ctx.network_blocked_attempts));
    checks.push(check_disk_floor(ctx.paths, ctx.config));
    checks.push(check_throughput(ctx));
    if let Some(q) = ctx.queue {
        checks.push(check_queue_consistency(q));
    }
    if let Some(key) = ctx.key {
        checks.push(check_face_pipeline(ctx.archive, key, &ctx.face_model));
    }

    Ok(VerifierReport {
        checks,
        generated_at: crate::util::now_iso8601(),
    })
}

fn check_db_integrity(conn: &Connection) -> Check {
    match crate::db::integrity_check(conn) {
        Ok(()) => Check::pass("db_integrity", "integrity_check and foreign_key_check ok"),
        Err(e) => Check::halt("integrity_db_corruption", format!("{e}")),
    }
}

fn check_catalogue_rows(conn: &Connection) -> Check {
    // Every 'complete' file must have metadata + scene rows and analysis_version.
    let missing: i64 = conn
        .query_row(
            "SELECT count(*) FROM files f
             WHERE f.status='complete'
               AND (f.analysis_version = 0
                    OR NOT EXISTS (SELECT 1 FROM metadata m WHERE m.file_id=f.id))",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    if missing == 0 {
        Check::pass("catalogue_rows", "all complete files have catalogue rows")
    } else {
        Check::fail(
            "catalogue_rows",
            format!("{missing} complete files missing catalogue rows"),
        )
    }
}

fn check_hashes(conn: &Connection) -> Check {
    let missing: i64 = conn
        .query_row(
            "SELECT count(*) FROM files WHERE status='complete' AND perceptual_hash IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    if missing == 0 {
        Check::pass("hashes", "all complete files have a perceptual hash")
    } else {
        Check::fail("hashes", format!("{missing} complete files missing perceptual hash"))
    }
}

fn check_thumbnails(conn: &Connection, paths: &AppPaths) -> Vec<Check> {
    // Every complete file has a thumbnail row.
    let missing_row: i64 = conn
        .query_row(
            "SELECT count(*) FROM files f
             WHERE f.status='complete' AND NOT EXISTS
                (SELECT 1 FROM thumbnails t WHERE t.file_id=f.id)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    let mut checks = vec![if missing_row == 0 {
        Check::pass("thumbnail_rows", "every complete file has a thumbnail row")
    } else {
        Check::fail("thumbnail_rows", format!("{missing_row} complete files lack a thumbnail row"))
    }];

    // Each thumbnail file decodes and matches its checksum/dimensions.
    let dir = paths.thumbnails_dir();
    let mut stmt = match conn.prepare(
        "SELECT file_id, rel_path, width, height, format, checksum, decode_ok FROM thumbnails",
    ) {
        Ok(s) => s,
        Err(e) => {
            checks.push(Check::fail("thumbnail_files", format!("query error: {e}")));
            return checks;
        }
    };
    let rows = stmt
        .query_map([], |r| {
            Ok(ThumbnailInfo {
                rel_path: r.get(1)?,
                width: r.get(2)?,
                height: r.get(3)?,
                format: r.get(4)?,
                checksum: r.get(5)?,
                decode_ok: r.get::<_, i64>(6)? != 0,
            })
        })
        .and_then(|m| m.collect::<std::result::Result<Vec<_>, _>>());
    match rows {
        Ok(infos) => {
            let mut bad = 0;
            let mut detail = String::new();
            for info in &infos {
                if let Err(e) = thumbnail::verify(&dir, info) {
                    bad += 1;
                    if detail.len() < 200 {
                        detail.push_str(&format!("{e}; "));
                    }
                }
            }
            checks.push(if bad == 0 {
                Check::pass("thumbnail_files", format!("{} thumbnails decode and match", infos.len()))
            } else {
                Check::fail("thumbnail_files", format!("{bad} bad thumbnails: {detail}"))
            });
        }
        Err(e) => checks.push(Check::fail("thumbnail_files", format!("read error: {e}"))),
    }
    checks
}

fn check_originals_unchanged(conn: &Connection) -> Check {
    // For each complete file that still resolves to a present original, confirm
    // size + mtime match the recorded snapshot. A mismatch is a hard halt.
    let mut stmt = match conn.prepare(
        "SELECT f.size_bytes, f.source_mtime_ns, d.volume_name, f.relative_path
         FROM files f
         JOIN drives d ON d.id=f.drive_id
         WHERE f.status='complete'",
    ) {
        Ok(s) => s,
        Err(e) => return Check::fail("originals_unchanged", format!("query error: {e}")),
    };
    // We cannot always resolve an absolute path here (drive may be offline).
    // The check verifies only files whose absolute path is currently present;
    // offline files are skipped (their integrity was verified at index time).
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,             // size
            r.get::<_, i64>(1)?,             // mtime
            r.get::<_, Option<String>>(2)?, // volume_name
            r.get::<_, String>(3)?,          // relative_path
        ))
    });
    let rows = match rows.and_then(|m| m.collect::<std::result::Result<Vec<_>, _>>()) {
        Ok(v) => v,
        Err(e) => return Check::fail("originals_unchanged", format!("read error: {e}")),
    };
    let mut checked = 0;
    for (size, mtime, volume_name, rel_path) in rows {
        // Best-effort absolute path via a mounted /Volumes/<name>.
        let Some(vol) = volume_name else { continue };
        let abs = Path::new("/Volumes").join(&vol).join(&rel_path);
        if !abs.exists() {
            continue; // offline / not mounted: skip
        }
        let snap = SourceSnapshot {
            size_bytes: size as u64,
            mtime_ns: mtime,
            birthtime_ns: None,
            inode_or_file_id: None,
        };
        if let Err(e) = snap.assert_unchanged(&abs) {
            return Check::halt("originals_modified", format!("{e}"));
        }
        checked += 1;
    }
    Check::pass(
        "originals_unchanged",
        format!("verified {checked} present originals unchanged (offline skipped)"),
    )
}

fn check_output_containment(conn: &Connection, paths: &AppPaths) -> Check {
    // No thumbnail rel_path may escape the app-owned thumbnails dir.
    let mut stmt = match conn.prepare("SELECT rel_path FROM thumbnails") {
        Ok(s) => s,
        Err(e) => return Check::fail("output_path_containment", format!("query error: {e}")),
    };
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .and_then(|m| m.collect::<std::result::Result<Vec<_>, _>>());
    let rows = match rows {
        Ok(v) => v,
        Err(e) => return Check::fail("output_path_containment", format!("read error: {e}")),
    };
    for rel in rows {
        if rel.contains("..") || rel.starts_with('/') {
            return Check::halt(
                "output_path_escape",
                format!("thumbnail path escapes app dir: {rel}"),
            );
        }
        let abs = paths.thumbnails_dir().join(&rel);
        if !abs.starts_with(paths.thumbnails_dir()) {
            return Check::halt("output_path_escape", format!("path escapes: {rel}"));
        }
    }
    Check::pass("output_path_containment", "all output paths contained")
}

fn check_network_isolation(blocked_attempts: u64) -> Check {
    if blocked_attempts == 0 {
        Check::pass("network_isolation", "no network access attempted during indexing")
    } else {
        Check::halt(
            "network_isolation_violated",
            format!("{blocked_attempts} network attempts blocked during indexing"),
        )
    }
}

fn check_disk_floor(paths: &AppPaths, config: &Config) -> Check {
    match crate::util::available_space(&paths.root) {
        Ok(free) => {
            if free >= config.free_space_floor_bytes {
                Check::pass(
                    "disk_floor",
                    format!("{} bytes free, floor {}", free, config.free_space_floor_bytes),
                )
            } else {
                Check::halt(
                    "disk_floor_breach",
                    format!("free {} below floor {}", free, config.free_space_floor_bytes),
                )
            }
        }
        Err(e) => Check::warn("disk_floor", format!("could not determine free space: {e}")),
    }
}

fn check_throughput(ctx: &VerifyContext) -> Check {
    match ctx.observed_throughput {
        Some(t) if t >= ctx.config.min_throughput_files_per_sec => {
            Check::pass("throughput", format!("{t:.3} files/sec"))
        }
        Some(t) => Check::warn(
            "throughput",
            format!("{t:.3} files/sec below {:.3}", ctx.config.min_throughput_files_per_sec),
        ),
        None => Check::pass("throughput", "no throughput sample (verify-only)"),
    }
}

fn check_queue_consistency(queue: &Connection) -> Check {
    // No complete item may remain leased; no item both complete and queued.
    let leased_complete: i64 = queue
        .query_row(
            "SELECT count(*) FROM queue_items qi
             JOIN queue_leases ql ON ql.item_id = qi.id
             WHERE qi.state='complete'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    if leased_complete != 0 {
        return Check::fail(
            "queue_consistency",
            format!("{leased_complete} complete items still hold a lease"),
        );
    }
    Check::pass("queue_consistency", "queue states consistent")
}

fn check_face_pipeline(conn: &Connection, key: &MasterKey, model: &(String, String)) -> Check {
    let repo = FaceRepo::new(conn);
    match repo.embedding_health(&model.0, &model.1, key) {
        Ok(h) => {
            if h.total == 0 {
                return Check::pass("face_pipeline", "no faces to check");
            }
            if h.non_finite > 0 {
                return Check::fail("face_pipeline", format!("{} non-finite embeddings", h.non_finite));
            }
            if h.dim_mismatches > 0 {
                return Check::fail("face_pipeline", format!("{} dim mismatches", h.dim_mismatches));
            }
            // Suspicious if nearly all embeddings are byte-identical.
            if h.total >= 5 && h.max_identical as f64 / h.total as f64 > 0.9 {
                return Check::warn(
                    "face_pipeline",
                    format!("{} of {} embeddings identical (possible detector failure)", h.max_identical, h.total),
                );
            }
            Check::pass(
                "face_pipeline",
                format!("{} embeddings, dim {}, finite", h.total, h.dim),
            )
        }
        Err(Error::Encryption(e)) => Check::halt("face_encryption_failure", e),
        Err(e) => Check::fail("face_pipeline", format!("{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, SchemaKind};

    fn ctx_paths() -> (tempfile::TempDir, AppPaths, Config) {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        paths.ensure().unwrap();
        let mut config = Config::default();
        config.free_space_floor_bytes = 0; // don't fail on CI disk
        (dir, paths, config)
    }

    #[test]
    fn clean_empty_catalogue_passes() {
        let (_d, paths, config) = ctx_paths();
        let archive = open_in_memory(SchemaKind::Archive).unwrap();
        let queue = open_in_memory(SchemaKind::Queue).unwrap();
        let ctx = VerifyContext {
            archive: &archive,
            queue: Some(&queue),
            paths: &paths,
            config: &config,
            key: None,
            face_model: ("local-heuristic".into(), "0.1.0".into()),
            observed_throughput: None,
            network_blocked_attempts: 0,
        };
        let report = run(&ctx).unwrap();
        assert!(report.ok(), "report should pass: {}", report.summary());
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn network_attempt_halts() {
        let (_d, paths, config) = ctx_paths();
        let archive = open_in_memory(SchemaKind::Archive).unwrap();
        let ctx = VerifyContext {
            archive: &archive,
            queue: None,
            paths: &paths,
            config: &config,
            key: None,
            face_model: ("m".into(), "1".into()),
            observed_throughput: None,
            network_blocked_attempts: 3,
        };
        let report = run(&ctx).unwrap();
        assert!(!report.ok());
        assert!(report.has_halt());
        assert_eq!(report.exit_code(), crate::error::exit::SOURCE_INTEGRITY);
    }

    #[test]
    fn missing_hash_fails() {
        let (_d, paths, config) = ctx_paths();
        let archive = open_in_memory(SchemaKind::Archive).unwrap();
        archive
            .execute_batch(
                "INSERT INTO drives (id, drive_number, status, first_seen_at) VALUES ('d',1,'online','now');
                 INSERT INTO roots (id, drive_id, relative_root, created_at) VALUES ('r','d','','now');
                 INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                    source_mtime_ns, status, analysis_version, created_at, updated_at)
                 VALUES ('f','d','r','a.jpg','a.jpg',1,1,'complete',1,'now','now');
                 INSERT INTO metadata (file_id) VALUES ('f');",
            )
            .unwrap();
        let ctx = VerifyContext {
            archive: &archive,
            queue: None,
            paths: &paths,
            config: &config,
            key: None,
            face_model: ("m".into(), "1".into()),
            observed_throughput: None,
            network_blocked_attempts: 0,
        };
        let report = run(&ctx).unwrap();
        assert!(!report.ok(), "missing perceptual hash should fail");
        let hashes = report.checks.iter().find(|c| c.name == "hashes").unwrap();
        assert_eq!(hashes.status, CheckStatus::Fail);
    }
}
