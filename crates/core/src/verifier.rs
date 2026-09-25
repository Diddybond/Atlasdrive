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

/// Which photographs the per-photograph checks read.
///
/// The verifier's checks come in two kinds, and they cost very different
/// amounts. Some are one SQL query over the catalogue — every complete file has
/// a perceptual hash, every file has a thumbnail row, the queue agrees with
/// itself — and stay catalogue-wide in every scope, because a query over
/// 200,000 rows is milliseconds. The others touch each photograph in turn:
/// they read and decode its thumbnail, `stat` its original on an external
/// drive, decrypt its face embeddings. Those are what the scope narrows.
///
/// Running the per-photograph checks over the whole catalogue after every
/// 64-photograph batch made indexing quadratic: at 100,000 photographs each
/// batch re-decoded 100,000 thumbnails it had already verified, and across a
/// twenty-drive archive that alone was measured in days (D-087).
#[derive(Debug, Clone, Copy)]
pub enum Scope<'s> {
    /// Every photograph in the catalogue: `atlasdrive verify` and verify-only.
    Catalogue,
    /// Every photograph on one drive, by drive id: the end of a scan.
    Drive(&'s str),
    /// Exactly these file ids: the batch a scan has just written.
    Files(&'s [String]),
}

impl Scope<'_> {
    /// A condition restricting the file-id column `col` to this scope, and the
    /// one parameter (`?1`) it needs, if any.
    fn filter(&self, col: &str) -> (String, Option<String>) {
        match self {
            Scope::Catalogue => ("1".into(), None),
            Scope::Drive(drive_id) => (
                format!("{col} IN (SELECT id FROM files WHERE drive_id = ?1)"),
                Some(drive_id.to_string()),
            ),
            Scope::Files(ids) => (
                format!("{col} IN (SELECT value FROM json_each(?1))"),
                Some(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into())),
            ),
        }
    }
}

/// Context needed to run the verifier.
pub struct VerifyContext<'a> {
    pub archive: &'a Connection,
    pub queue: Option<&'a Connection>,
    pub paths: &'a AppPaths,
    pub config: &'a Config,
    pub key: Option<&'a MasterKey>,
    /// Median batch throughput observed this run (files/sec), if known.
    pub observed_throughput: Option<f64>,
    /// Whether the run's network guard recorded zero blocked attempts.
    pub network_blocked_attempts: u64,
}

/// Run the full verifier suite over the whole catalogue.
pub fn run(ctx: &VerifyContext) -> Result<VerifierReport> {
    run_scoped(ctx, Scope::Catalogue)
}

/// Run the verifier suite with its per-photograph checks limited to `scope`.
///
/// Every check still runs in every scope; see [`Scope`] for which ones the
/// scope narrows.
pub fn run_scoped(ctx: &VerifyContext, scope: Scope) -> Result<VerifierReport> {
    let mut checks = Vec::new();

    checks.push(check_db_integrity(ctx.archive));
    checks.push(check_catalogue_rows(ctx.archive));
    checks.push(check_hashes(ctx.archive));
    checks.extend(check_thumbnails(ctx.archive, ctx.paths, scope));
    checks.push(check_originals_unchanged(ctx.archive, scope));
    checks.push(check_output_containment(ctx.archive, ctx.paths));
    checks.push(check_network_isolation(ctx.network_blocked_attempts));
    checks.push(check_disk_floor(ctx.paths, ctx.config));
    checks.push(check_throughput(ctx));
    checks.push(check_heartbeat(ctx.paths, ctx.archive));
    if let Some(q) = ctx.queue {
        checks.push(check_queue_consistency(q));
    }
    if let Some(key) = ctx.key {
        checks.push(check_face_pipeline(ctx.archive, key, scope));
    }

    Ok(VerifierReport {
        checks,
        generated_at: crate::util::now_iso8601(),
    })
}

/// How many times to re-attempt an integrity check that could not acquire a lock.
const INTEGRITY_LOCK_RETRIES: u32 = 3;

fn check_db_integrity(conn: &Connection) -> Check {
    // A lock is not corruption, and the difference matters enormously: one is a
    // reason to stop everything, the other is a reason to look again in a
    // moment.
    //
    // `PRAGMA integrity_check` needs to read the whole file, including the FTS5
    // index. If another connection holds a write lock it answers "database is
    // locked" — the check did not fail, it did not run. Treating that as
    // corruption halted a real 102,000-photograph scan after five batches,
    // because the owner had opened AtlasDrive to watch it. The catalogue was
    // fine; two things were simply reading it at once.
    let mut last = String::new();
    for attempt in 0..=INTEGRITY_LOCK_RETRIES {
        match crate::db::integrity_check(conn) {
            Ok(()) => {
                return Check::pass("db_integrity", "integrity_check and foreign_key_check ok")
            }
            Err(e) => {
                last = e.to_string();
                if !is_lock_contention(&last) {
                    return Check::halt("integrity_db_corruption", last);
                }
                if attempt < INTEGRITY_LOCK_RETRIES {
                    std::thread::sleep(std::time::Duration::from_millis(
                        250 * (1 << attempt) as u64,
                    ));
                }
            }
        }
    }
    // Still locked. Say so as a warning: indexing continues, and the check runs
    // again after the next batch.
    Check::warn(
        "db_integrity",
        format!(
            "could not verify just now — the catalogue was busy ({last}).              Not a sign of damage; it will be checked again after the next batch."
        ),
    )
}

/// True when a database error means "busy", not "broken".
fn is_lock_contention(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("database is locked")
        || m.contains("database table is locked")
        || m.contains("database schema is locked")
        || m.contains("busy")
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

fn check_thumbnails(conn: &Connection, paths: &AppPaths, scope: Scope) -> Vec<Check> {
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
    let (in_scope, param) = scope.filter("file_id");
    let mut stmt = match conn.prepare(&format!(
        "SELECT file_id, rel_path, width, height, format, checksum, decode_ok FROM thumbnails
          WHERE {in_scope}"
    )) {
        Ok(s) => s,
        Err(e) => {
            checks.push(Check::fail("thumbnail_files", format!("query error: {e}")));
            return checks;
        }
    };
    let rows = stmt
        .query_map(rusqlite::params_from_iter(param.iter()), |r| {
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
            // Each thumbnail is read, checksummed and decoded independently, so
            // the whole-archive check spreads them across the cores. One at a
            // time, 218,000 of them took minutes (D-087).
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
            let chunk = infos.len().div_ceil(threads).max(1);
            let failures: Vec<String> = std::thread::scope(|s| {
                let handles: Vec<_> = infos
                    .chunks(chunk)
                    .map(|part| {
                        let dir = &dir;
                        s.spawn(move || {
                            part.iter()
                                .filter_map(|info| thumbnail::verify(dir, info).err())
                                .map(|e| e.to_string())
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
            });
            let bad = failures.len();
            let mut detail = String::new();
            for e in &failures {
                if detail.len() >= 200 {
                    break;
                }
                detail.push_str(&format!("{e}; "));
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

fn check_originals_unchanged(conn: &Connection, scope: Scope) -> Check {
    // For each complete file that still resolves to a present original, confirm
    // size + mtime match the recorded snapshot. A mismatch is a hard halt.
    let (in_scope, param) = scope.filter("f.id");
    let mut stmt = match conn.prepare(&format!(
        "SELECT f.size_bytes, f.source_mtime_ns, d.volume_name, f.relative_path,
                (SELECT sr.scan_root FROM scan_runs sr
                  WHERE sr.drive_id = f.drive_id AND sr.mode <> 'dry-run'
                  ORDER BY sr.started_at DESC LIMIT 1)
         FROM files f
         JOIN drives d ON d.id=f.drive_id
         WHERE f.status='complete' AND {in_scope}"
    )) {
        Ok(s) => s,
        Err(e) => return Check::fail("originals_unchanged", format!("query error: {e}")),
    };
    // An absolute path is not always resolvable (the drive may be disconnected).
    // Files whose original is genuinely absent are skipped and counted — their
    // integrity was verified at index time — but the count is reported so a
    // wholly-skipped run can never be mistaken for a verified one.
    let rows = stmt.query_map(rusqlite::params_from_iter(param.iter()), |r| {
        Ok((
            r.get::<_, i64>(0)?,            // size
            r.get::<_, i64>(1)?,            // mtime
            r.get::<_, Option<String>>(2)?, // volume_name
            r.get::<_, String>(3)?,         // relative_path
            r.get::<_, Option<String>>(4)?, // scan_root of the latest real run
        ))
    });
    let rows = match rows.and_then(|m| m.collect::<std::result::Result<Vec<_>, _>>()) {
        Ok(v) => v,
        Err(e) => return Check::fail("originals_unchanged", format!("read error: {e}")),
    };
    let mut checked = 0;
    let mut skipped = 0;
    for (size, mtime, volume_name, rel_path, scan_root) in rows {
        // Prefer the root the file was actually indexed from; fall back to a
        // drive mounted at the conventional /Volumes/<name>.
        let candidates = [
            scan_root.map(|root| Path::new(&root).join(&rel_path)),
            volume_name.map(|vol| Path::new("/Volumes").join(&vol).join(&rel_path)),
        ];
        let Some(abs) = candidates.into_iter().flatten().find(|p| p.exists()) else {
            skipped += 1; // offline / not mounted
            continue;
        };
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
        format!("verified {checked} present originals unchanged; {skipped} skipped (offline)"),
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

/// `docs/13` requires the verifier to confirm the worker's heartbeat is current.
///
/// `progress.json` is that heartbeat: it is rewritten after every photograph,
/// success or failure. A run that says it is still going and has written
/// nothing for half an hour is not going. This is a warning rather than a
/// failure because the catalogue is not wrong — the scan is stuck, which is a
/// thing to be told, not a corruption to halt over.
fn check_heartbeat(paths: &AppPaths, archive: &Connection) -> Check {
    // The catalogue knows which drive each run belongs to, so it is asked
    // first: `progress.json` describes only whichever run wrote it last.
    match crate::inventory::running_scans(archive) {
        Ok(runs) => {
            let quiet: Vec<&crate::inventory::RunningScan> =
                runs.iter().filter(|r| r.stale).collect();
            if let Some(worst) = quiet.iter().max_by_key(|r| r.silent_for_minutes.unwrap_or(0)) {
                return Check::warn(
                    "heartbeat",
                    format!(
                        "{} scan(s) recorded as running have gone quiet; the longest is drive {} \
                         at {} minutes. Either the run was killed or it is stuck.",
                        quiet.len(),
                        worst.drive_number,
                        worst.silent_for_minutes.unwrap_or(-1)
                    ),
                );
            }
            if !runs.is_empty() {
                return Check::pass("heartbeat", format!("{} scan(s) running and current", runs.len()));
            }
        }
        Err(e) => return Check::warn("heartbeat", format!("could not read scan runs: {e}")),
    }

    let progress = match crate::progress::Progress::load(paths) {
        Ok(Some(p)) => p,
        // No scan has ever run here, or the file is unreadable. Neither is
        // evidence of a stall.
        Ok(None) => return Check::pass("heartbeat", "no scan in progress"),
        Err(e) => return Check::warn("heartbeat", format!("progress.json unreadable: {e}")),
    };
    // The verifier is a separate process from any running scan, so it cannot
    // see whether one is in flight — `None` says exactly that.
    match progress.reconciled_status(None).as_str() {
        "stalled" => Check::warn(
            "heartbeat",
            format!(
                "drive {} reports a running scan but has written nothing for {} minutes",
                progress.drive_number,
                progress.age_minutes().unwrap_or(-1)
            ),
        ),
        other => Check::pass("heartbeat", format!("last scan {other}")),
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

    // `docs/13`: failed items include reason and retry count. This is not
    // bookkeeping. A photograph given up on can be put back in the queue by its
    // failure code (D-060) — 232 large TIFFs on a real drive came back that way
    // once the decoder could read them. An item marked failed with no recorded
    // reason can never be revived by code, and never appears in the list of what
    // went wrong, so it is simply absent from the archive with nothing to say so.
    let unexplained: i64 = queue
        .query_row(
            "SELECT count(*) FROM queue_items qi
              WHERE qi.state='failed'
                AND NOT EXISTS (SELECT 1 FROM queue_failures f WHERE f.item_id = qi.id)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    if unexplained != 0 {
        return Check::fail(
            "queue_consistency",
            format!("{unexplained} failed items have no recorded reason, so nothing can revive them"),
        );
    }

    let untried: i64 = queue
        .query_row(
            "SELECT count(*) FROM queue_items WHERE state='failed' AND attempts < 1",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    if untried != 0 {
        return Check::fail(
            "queue_consistency",
            format!("{untried} failed items were never attempted"),
        );
    }

    Check::pass("queue_consistency", "queue states consistent")
}

/// Face embeddings in scope are the right shape and finite, per model.
///
/// Checked for every model partition present, not one named in advance. The
/// batch verifier used to be told to look at the heuristic engine's partition
/// only, so on a Mac — where every face comes from Apple Vision — the check
/// found nothing to look at and passed without having read a single embedding.
fn check_face_pipeline(conn: &Connection, key: &MasterKey, scope: Scope) -> Check {
    let repo = FaceRepo::new(conn);
    let (in_scope, param) = scope.filter("f.file_id");
    match repo.embedding_health_where(&in_scope, param.as_deref(), key) {
        Ok(partitions) => {
            if partitions.is_empty() {
                return Check::pass("face_pipeline", "no faces to check");
            }
            let mut summary = Vec::new();
            for ((model_id, model_version), h) in &partitions {
                let model = format!("{model_id} {model_version}");
                if h.non_finite > 0 {
                    return Check::fail(
                        "face_pipeline",
                        format!("{model}: {} non-finite embeddings", h.non_finite),
                    );
                }
                if h.dim_mismatches > 0 {
                    return Check::fail(
                        "face_pipeline",
                        format!("{model}: {} dim mismatches", h.dim_mismatches),
                    );
                }
                // Suspicious if nearly all embeddings are byte-identical.
                if h.total >= 5 && h.max_identical as f64 / h.total as f64 > 0.9 {
                    return Check::warn(
                        "face_pipeline",
                        format!(
                            "{model}: {} of {} embeddings identical (possible detector failure)",
                            h.max_identical, h.total
                        ),
                    );
                }
                summary.push(format!("{model}: {} embeddings, dim {}, finite", h.total, h.dim));
            }
            Check::pass("face_pipeline", summary.join("; "))
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
        // don't fail on CI disk
        let config = Config { free_space_floor_bytes: 0, ..Default::default() };
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
            observed_throughput: None,
            network_blocked_attempts: 0,
        };
        let report = run(&ctx).unwrap();
        assert!(report.ok(), "report should pass: {}", report.summary());
        assert_eq!(report.exit_code(), 0);
    }

    /// `docs/13`: the worker heartbeat must be checked, and the check has to be
    /// available where recovery actually happens — the command line — not only
    /// in the desktop app.
    #[test]
    fn a_scan_that_claims_to_be_running_but_has_gone_quiet_is_reported() {
        let (_d, paths, _config) = ctx_paths();
        let archive = open_in_memory(SchemaKind::Archive).unwrap();
        let mut p = crate::progress::Progress::new("run1", 5, "drv", "/Volumes/Drive 5");
        p.updated_at =
            (chrono::Utc::now() - chrono::Duration::minutes(crate::progress::STALL_AFTER_MINUTES + 5))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
        p.write(&paths).unwrap();

        let check = check_heartbeat(&paths, &archive);
        assert!(matches!(check.status, CheckStatus::Warn), "{check:?}");
        assert!(check.detail.contains("Drive 5") || check.detail.contains("drive 5"), "{check:?}");

        // A scan writing normally passes, and so does a machine that has never
        // scanned at all.
        p.touch();
        p.write(&paths).unwrap();
        assert!(matches!(check_heartbeat(&paths, &archive).status, CheckStatus::Pass));
        let (_d2, empty, _c) = ctx_paths();
        assert!(matches!(check_heartbeat(&empty, &archive).status, CheckStatus::Pass));
    }

    /// `docs/13`: failed items include reason and retry count — because a
    /// failure that recorded neither cannot be revived and cannot be listed.
    #[test]
    fn a_failed_item_with_no_recorded_reason_fails_the_queue_check() {
        let queue = open_in_memory(SchemaKind::Queue).unwrap();
        let insert = |id: &str, state: &str, attempts: i64| {
            queue
                .execute(
                    "INSERT INTO queue_items
                       (id, run_id, drive_id, drive_number, root_id, relative_path, abs_path,
                        size_bytes, source_mtime_ns, state, attempts, enqueued_at, queue_key)
                     VALUES (?1,'run','drv',5,'root',?1,?1,1,1,?2,?3,'now',?1)",
                    rusqlite::params![id, state, attempts],
                )
                .unwrap();
        };

        // A healthy queue: one done, one properly failed with its reason.
        insert("ok", "complete", 1);
        insert("bad", "failed", 3);
        queue
            .execute(
                "INSERT INTO queue_failures (id, item_id, code, message, retryable, created_at)
                 VALUES ('f1','bad','DECODE','memory limit exceeded',0,'now')",
                [],
            )
            .unwrap();
        assert!(matches!(check_queue_consistency(&queue).status, CheckStatus::Pass));

        // A failure nothing explains: invisible in the failure list, and
        // `retry_failed --code` can never bring it back.
        insert("orphan", "failed", 2);
        let check = check_queue_consistency(&queue);
        assert!(matches!(check.status, CheckStatus::Fail), "{check:?}");
        assert!(check.detail.contains("no recorded reason"), "{check:?}");
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
            observed_throughput: None,
            network_blocked_attempts: 3,
        };
        let report = run(&ctx).unwrap();
        assert!(!report.ok());
        assert!(report.has_halt());
        assert_eq!(report.exit_code(), crate::error::exit::SOURCE_INTEGRITY);
    }

    /// Faces from every model are checked, not only the heuristic engine's.
    ///
    /// The verifier was told which partition to read, and every caller named
    /// the heuristic one — so on a Mac, where faces come from Apple Vision,
    /// the check read nothing and passed.
    #[test]
    fn face_embeddings_from_every_model_are_checked() {
        let archive = open_in_memory(SchemaKind::Archive).unwrap();
        archive
            .execute_batch(
                "INSERT INTO drives (id, drive_number, status, first_seen_at) VALUES ('d',1,'online','now');
                 INSERT INTO roots (id, drive_id, relative_root, created_at) VALUES ('r','d','','now');
                 INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                    source_mtime_ns, status, analysis_version, created_at, updated_at)
                 VALUES ('f','d','r','a.jpg','a.jpg',1,1,'queued',1,'now','now');",
            )
            .unwrap();
        let key = MasterKey::generate(1);
        let repo = FaceRepo::new(&archive);
        repo.insert_face("f", (0.1, 0.1, 0.2, 0.2), 0.9, "local-heuristic", "0.2.0", &[0.1, 0.2, 0.3], &key)
            .unwrap();
        repo.insert_face("f", (0.5, 0.5, 0.2, 0.2), 0.9, "apple-vision", "1.0.0", &[0.4, f32::NAN, 0.1, 0.2], &key)
            .unwrap();

        let check = check_face_pipeline(&archive, &key, Scope::Catalogue);
        assert_eq!(check.status, CheckStatus::Fail, "{check:?}");
        assert!(check.detail.contains("apple-vision"), "{check:?}");

        // Scoped to a file that holds it, the same face is still found; scoped
        // to a batch that does not, it is not this batch's business.
        let f = vec!["f".to_string()];
        assert_eq!(check_face_pipeline(&archive, &key, Scope::Files(&f)).status, CheckStatus::Fail);
        let other = vec!["g".to_string()];
        assert_eq!(check_face_pipeline(&archive, &key, Scope::Files(&other)).status, CheckStatus::Pass);
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
            observed_throughput: None,
            network_blocked_attempts: 0,
        };
        let report = run(&ctx).unwrap();
        assert!(!report.ok(), "missing perceptual hash should fail");
        let hashes = report.checks.iter().find(|c| c.name == "hashes").unwrap();
        assert_eq!(hashes.status, CheckStatus::Fail);
    }
}

#[cfg(test)]
mod lock_vs_corruption_tests {
    use super::*;

    /// The exact message SQLite produced on the owner's machine when the app
    /// was open while a scan ran. It stopped a 102,000-photograph scan after
    /// five batches by reporting a healthy catalogue as corrupt.
    #[test]
    fn the_message_that_halted_a_real_scan_is_recognised_as_a_lock() {
        let real = "integrity_check failed: unable to validate the inverted index for \
                    FTS5 table main.files_fts: database is locked";
        assert!(is_lock_contention(real), "must be treated as busy, not broken");
    }

    #[test]
    fn other_ways_sqlite_says_busy_are_recognised() {
        for m in [
            "database table is locked",
            "database is locked",
            "SQLITE_BUSY: database is busy",
            "Database Is Locked",
        ] {
            assert!(is_lock_contention(m), "{m} should count as busy");
        }
    }

    /// The half that must not regress: real damage still has to halt. A
    /// verifier that shrugs at corruption is worse than none.
    #[test]
    fn real_corruption_is_not_mistaken_for_a_lock() {
        for m in [
            "integrity_check failed: *** in database main *** page 42 is never used",
            "database disk image is malformed",
            "integrity_check failed: row missing from index idx_files_drive",
            "foreign_key_check failed: files references a missing drive",
        ] {
            assert!(!is_lock_contention(m), "{m} must still halt");
        }
    }

    /// A healthy catalogue passes, and the check that runs against it is the
    /// real one — not a stub that would pass whatever it was given.
    #[test]
    fn a_healthy_catalogue_passes() {
        let conn = crate::db::open_in_memory(crate::db::SchemaKind::Archive).unwrap();
        let check = check_db_integrity(&conn);
        assert_eq!(check.status, CheckStatus::Pass, "{:?}", check.detail);
    }
}
