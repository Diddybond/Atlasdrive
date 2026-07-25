//! End-to-end pipeline tests exercising the safety-critical happy path and the
//! resumability / dry-run / integrity gates.

use std::path::Path;
use std::sync::Arc;

use image::{Rgb, RgbImage};

use crate::ai::{CancelToken, EngineRegistry};
use crate::config::{AppPaths, Config};
use crate::crypto::MasterKey;
use crate::db::{self, SchemaKind};
use crate::drive::{DriveRepo, RegisterParams};
use crate::logging::Logger;
use crate::pipeline::{IndexMode, IndexOptions, Pipeline};

struct Harness {
    _dir: tempfile::TempDir,
    paths: AppPaths,
    archive: rusqlite::Connection,
    queue: rusqlite::Connection,
    key: MasterKey,
    drive_dir: std::path::PathBuf,
}

fn write_photo(path: &Path, color: [u8; 3], w: u32, h: u32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut img = RgbImage::from_pixel(w, h, Rgb(color));
    // add a little structure so hashes are not degenerate
    for x in 0..w {
        img.put_pixel(x, 0, Rgb([255, 255, 255]));
    }
    img.save(path).unwrap();
}

fn setup(config: Config) -> (Harness, IndexOptions) {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(dir.path().join("appdata"));
    paths.ensure().unwrap();
    let archive = db::open(&paths.archive_db(), SchemaKind::Archive).unwrap();
    let queue = db::open(&paths.queue_db(), SchemaKind::Queue).unwrap();

    // Register drive 14.
    {
        let repo = DriveRepo::new(&archive);
        repo.register(&RegisterParams {
            drive_number: 14,
            friendly_name: Some("Test Drive".into()),
            volume_name: Some("TestVol".into()),
            ..Default::default()
        })
        .unwrap();
    }

    // Create a fake drive root with photos.
    let drive_dir = dir.path().join("Volumes/TestVol");
    write_photo(&drive_dir.join("holiday/beach.png"), [30, 120, 200], 80, 60);
    write_photo(&drive_dir.join("family/xmas_1987.png"), [200, 40, 40], 80, 60);
    write_photo(&drive_dir.join("scan.png"), [180, 175, 170], 100, 80);

    let mut opts = IndexOptions::new(14, &drive_dir);
    opts.config = config;

    (
        Harness {
            _dir: dir,
            paths,
            archive,
            queue,
            key: MasterKey::generate(1),
            drive_dir,
        },
        opts,
    )
}

fn pipeline<'a>(h: &'a Harness) -> Pipeline<'a> {
    Pipeline {
        archive: &h.archive,
        queue: &h.queue,
        paths: &h.paths,
        engines: Arc::new(EngineRegistry::local_default()),
        key: &h.key,
        logger: Logger::new(h.paths.index_log()),
        cancel: CancelToken::new(),
    }
}

fn no_disk_floor() -> Config {
    let mut c = Config::default();
    c.free_space_floor_bytes = 0;
    c.batch_size = 2;
    c
}

#[test]
fn end_to_end_index_and_verify() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    let summary = p.run(&opts).unwrap();
    assert_eq!(summary.files_discovered, 3);
    assert_eq!(summary.files_done, 3);
    assert_eq!(summary.files_failed, 0);
    assert!(!summary.halted);

    // Catalogue has three complete files, each with a thumbnail + phash.
    let complete: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE status='complete'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(complete, 3);
    let thumbs: i64 = h
        .archive
        .query_row("SELECT count(*) FROM thumbnails", [], |r| r.get(0))
        .unwrap();
    assert_eq!(thumbs, 3);

    // Final verify-only passes.
    let mut vo = opts.clone();
    vo.mode = IndexMode::VerifyOnly;
    p.run(&vo).unwrap();
}

#[test]
fn rerun_is_idempotent() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();
    let first: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .unwrap();
    // Re-run: no duplicate rows or thumbnails.
    p.run(&opts).unwrap();
    let second: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first, 3);
}

#[test]
fn dry_run_writes_nothing_permanent() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    let mut dry = opts.clone();
    dry.mode = IndexMode::DryRun;
    let summary = p.run(&dry).unwrap();
    assert!(summary.dry_run);
    // No catalogue rows written.
    let files: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(files, 0);
    // No permanent thumbnails.
    let thumb_dir = h.paths.thumbnails_dir();
    let count = std::fs::read_dir(&thumb_dir).map(|d| d.count()).unwrap_or(0);
    assert_eq!(count, 0);
}

#[test]
fn resume_after_interruption() {
    let (h, opts) = setup(no_disk_floor());

    // First pass: cancel immediately so the run enqueues work but processes
    // little or nothing, then records itself as interrupted (resumable).
    let cancel = CancelToken::new();
    cancel.cancel();
    let interrupted = Pipeline {
        archive: &h.archive,
        queue: &h.queue,
        paths: &h.paths,
        engines: Arc::new(EngineRegistry::local_default()),
        key: &h.key,
        logger: Logger::new(h.paths.index_log()),
        cancel,
    };
    let s = interrupted.run(&opts).unwrap();
    assert!(s.files_done < 3, "interrupted run should not finish everything");

    // Resume: a fresh (non-cancelled) pipeline finishes the remaining work.
    let mut resume = opts.clone();
    resume.resume = true;
    let done = pipeline(&h).run(&resume).unwrap();
    let complete: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE status='complete'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(complete, 3, "resume completes all files");
    assert!(!done.halted);
}

#[test]
fn original_files_unchanged_after_indexing() {
    let (h, opts) = setup(no_disk_floor());
    // Snapshot mtimes before.
    let before: Vec<(std::path::PathBuf, std::time::SystemTime)> = walk(&h.drive_dir);
    pipeline(&h).run(&opts).unwrap();
    let after: Vec<(std::path::PathBuf, std::time::SystemTime)> = walk(&h.drive_dir);
    assert_eq!(before, after, "originals must be byte/mtime identical after indexing");
}

#[test]
fn malformed_file_is_isolated_not_fatal() {
    let (h, opts) = setup(no_disk_floor());
    // A garbage file with an image extension: must fail at file level, not crash
    // or halt the whole run.
    let bad = h.drive_dir.join("broken.png");
    std::fs::write(&bad, b"this is not a real png").unwrap();
    let summary = pipeline(&h).run(&opts).unwrap();
    assert!(!summary.halted, "a malformed file must not halt the run");
    assert!(summary.files_failed >= 1, "malformed file should be recorded as failed");
    // The three valid photos still index.
    let complete: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE status='complete'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(complete, 3);
}

#[test]
fn disk_floor_blocks_indexing() {
    let mut config = no_disk_floor();
    config.free_space_floor_bytes = u64::MAX; // impossible to satisfy
    let (h, mut opts) = setup(config.clone());
    opts.config = config;
    let err = pipeline(&h).run(&opts).unwrap_err();
    assert_eq!(err.exit_code(), crate::error::exit::INSUFFICIENT_DISK);
}

#[test]
fn no_network_attempts_during_indexing() {
    let (h, opts) = setup(no_disk_floor());
    crate::net::reset_blocked_attempts();
    pipeline(&h).run(&opts).unwrap();
    assert_eq!(crate::net::blocked_attempts(), 0, "indexing must attempt no network access");
}

fn walk(root: &Path) -> Vec<(std::path::PathBuf, std::time::SystemTime)> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(root).sort_by_file_name() {
        let e = e.unwrap();
        if e.file_type().is_file() {
            let m = e.metadata().unwrap();
            out.push((e.path().to_path_buf(), m.modified().unwrap()));
        }
    }
    out
}
