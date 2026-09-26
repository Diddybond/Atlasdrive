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
    Config { free_space_floor_bytes: 0, batch_size: 2, ..Default::default() }
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

/// Natural-language search over a really-indexed catalogue: a text query is
/// embedded locally and must rank the visually matching photograph above the
/// others, using only `archive.db` (drives may be disconnected).
#[test]
fn natural_language_search_ranks_by_visual_similarity() {
    use crate::ai::Capability;
    use crate::search::{SearchFilters, SearchRepo, VisualQuery};

    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    let registry = EngineRegistry::local_default();
    let engine = registry.engine_for(Capability::TextEmbedding);
    let cancel = CancelToken::new();
    let repo = SearchRepo::new(&h.archive);
    let filters = SearchFilters { include_offline: true, limit: 10, ..Default::default() };

    let rank_of = |query: &str, filename: &str| -> Option<usize> {
        let q = engine.text_embedding(query, &cancel).unwrap();
        let results = repo
            .natural_language_search(
                query,
                Some(VisualQuery {
                    vector: &q.value.vector,
                    model_id: engine.model_id(),
                    model_version: engine.model_version(),
                    coverage: q.meta.confidence,
                }),
                &filters,
            )
            .unwrap();
        results.iter().position(|r| r.filename == filename)
    };

    // The fixture holds a blue photo (beach.png) and a red one (xmas_1987.png).
    // A "blue" query must put the blue photo ahead of the red one, and "red"
    // must reverse that — the ranking has to follow the query, not a fixed order.
    let blue_beach = rank_of("blue", "beach.png").expect("beach.png ranked for 'blue'");
    let blue_xmas = rank_of("blue", "xmas_1987.png").expect("xmas ranked for 'blue'");
    assert!(blue_beach < blue_xmas, "blue: beach {blue_beach} should precede xmas {blue_xmas}");

    let red_xmas = rank_of("red", "xmas_1987.png").expect("xmas ranked for 'red'");
    let red_beach = rank_of("red", "beach.png").expect("beach ranked for 'red'");
    assert!(red_xmas < red_beach, "red: xmas {red_xmas} should precede beach {red_beach}");
}

/// A query the encoder does not understand must not reorder anything: the
/// visual leg is dropped and the result is exactly the text search.
#[test]
fn unintelligible_query_falls_back_to_text_search() {
    use crate::ai::Capability;
    use crate::search::{SearchFilters, SearchRepo, VisualQuery};

    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    let registry = EngineRegistry::local_default();
    let engine = registry.engine_for(Capability::TextEmbedding);
    let cancel = CancelToken::new();
    let repo = SearchRepo::new(&h.archive);
    let filters = SearchFilters { include_offline: true, limit: 10, ..Default::default() };

    // "beach" appears in a filename, so text search finds it; the nonsense word
    // carries no visual meaning.
    let query = "beach zzzzqqqq";
    let q = engine.text_embedding("zzzzqqqq", &cancel).unwrap();
    assert_eq!(q.meta.confidence, 0.0, "nonsense must report zero coverage");

    let text_only = repo.text_search(query, &filters).unwrap();
    let fused = repo
        .natural_language_search(
            query,
            Some(VisualQuery {
                vector: &q.value.vector,
                model_id: engine.model_id(),
                model_version: engine.model_version(),
                coverage: q.meta.confidence,
            }),
            &filters,
        )
        .unwrap();

    let text_ids: Vec<&str> = text_only.iter().map(|r| r.file_id.as_str()).collect();
    let fused_ids: Vec<&str> = fused.iter().map(|r| r.file_id.as_str()).collect();
    assert_eq!(text_ids, fused_ids);
    assert!(!fused.is_empty(), "expected the filename match to survive");
    assert!(fused.iter().all(|r| !r.matched.iter().any(|m| m == "visual")));
}

/// Offline drives stay searchable by natural language: the visual leg reads
/// only `archive.db`, never the original volume.
#[test]
fn natural_language_search_works_with_the_drive_disconnected() {
    use crate::ai::Capability;
    use crate::search::{SearchFilters, SearchRepo, VisualQuery};

    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    // Disconnect the drive: mark it offline and remove the volume entirely.
    h.archive
        .execute("UPDATE drives SET status='offline'", [])
        .unwrap();
    std::fs::remove_dir_all(&h.drive_dir).unwrap();

    let registry = EngineRegistry::local_default();
    let engine = registry.engine_for(Capability::TextEmbedding);
    let q = engine.text_embedding("blue", &CancelToken::new()).unwrap();
    let repo = SearchRepo::new(&h.archive);
    let results = repo
        .natural_language_search(
            "blue",
            Some(VisualQuery {
                vector: &q.value.vector,
                model_id: engine.model_id(),
                model_version: engine.model_version(),
                coverage: q.meta.confidence,
            }),
            &SearchFilters { include_offline: true, limit: 10, ..Default::default() },
        )
        .unwrap();

    assert!(!results.is_empty(), "offline catalogue must still be searchable");
    assert!(results.iter().all(|r| !r.online), "all results should report offline");
}

/// Critical gate: three consecutive verifier failures halt the run and write a
/// report. A *failing* (not halting) check must not stop the first batch — the
/// pipeline tolerates two, then stops rather than grinding on indefinitely.
#[test]
fn three_consecutive_verifier_failures_halt_and_report() {
    use crate::error::Error;

    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    // Introduce a catalogue defect that makes the verifier *fail* every batch
    // from now on: a complete file with no perceptual hash.
    let corrupted = h
        .archive
        .execute(
            "UPDATE files SET perceptual_hash = NULL
             WHERE id = (SELECT id FROM files WHERE status='complete' LIMIT 1)",
            [],
        )
        .unwrap();
    assert_eq!(corrupted, 1);

    // Add enough new work that the run would otherwise continue well past the
    // failure threshold, and force one file per batch.
    for i in 0..6 {
        write_photo(
            &h.drive_dir.join(format!("later/new_{i}.png")),
            [10 * i as u8, 90, 140],
            40,
            30,
        );
    }
    let mut opts2 = opts.clone();
    opts2.config = Config { batch_size: 1, ..no_disk_floor() };
    assert_eq!(opts2.config.max_consecutive_verifier_failures, 3);

    let err = p.run(&opts2).expect_err("repeated verifier failure must stop the run");
    match &err {
        Error::RepeatedVerifierFailure(summary) => {
            assert!(summary.contains("fail"), "summary should name the failure: {summary}");
        }
        other => panic!("expected RepeatedVerifierFailure, got {other:?}"),
    }

    // A report must exist for the halted run, and progress must say halted.
    let reports: Vec<_> = std::fs::read_dir(h.paths.reports_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("verifier-"))
        .collect();
    assert!(!reports.is_empty(), "a verifier report must be written on halt");

    let progress = crate::progress::Progress::load(&h.paths).unwrap().unwrap();
    assert_eq!(progress.status, "halted");
    assert_eq!(progress.consecutive_verifier_failures, 3);

    // The halt must stop early, not after draining all six new files.
    let done: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE status='complete'", [], |r| r.get(0))
        .unwrap();
    assert!(done < 3 + 6, "run should have halted before finishing the queue, got {done}");
}

/// A failure is published when it happens, not at the end of the batch that
/// contains it.
///
/// Progress was written after every *success* only. That left two marks. The
/// count of failures on screen lagged behind the truth, and — worse — a stretch
/// of files that each take minutes to fail wrote nothing at all, so a run that
/// was working exactly as designed went silent for long enough to be called
/// stalled. A wedged decoder is given ten minutes (D-081), so three such files
/// in a row is half an hour of silence.
///
/// A run that ends inside a batch is where the difference is visible: the
/// end-of-batch write never happens, so whatever was published per photograph
/// is all that survives. Here the run halts on repeated verifier failures with
/// a failed photograph in the final batch.
#[test]
fn a_failure_reaches_progress_before_the_batch_ends() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    // A catalogue defect that makes every later batch fail verification.
    h.archive
        .execute(
            "UPDATE files SET perceptual_hash = NULL
             WHERE id = (SELECT id FROM files WHERE status='complete' LIMIT 1)",
            [],
        )
        .unwrap();

    // Three more items, one per batch, with the unreadable one last: batches one
    // and two fail verification, and batch three both fails a photograph and
    // trips the third consecutive verifier failure, halting mid-batch.
    write_photo(&h.drive_dir.join("later/new_a.png"), [10, 90, 140], 40, 30);
    write_photo(&h.drive_dir.join("later/new_b.png"), [20, 90, 140], 40, 30);
    std::fs::write(h.drive_dir.join("later/s_broken.png"), b"not a png at all").unwrap();

    let mut opts2 = opts.clone();
    opts2.config = Config { batch_size: 1, ..no_disk_floor() };
    let err = p.run(&opts2).expect_err("three verifier failures must halt the run");
    assert_eq!(err.exit_code(), crate::error::exit::REPEATED_VERIFIER_FAILURE);

    let progress = crate::progress::Progress::load(&h.paths).unwrap().unwrap();
    assert_eq!(progress.status, "halted");
    assert_eq!(
        progress.files_failed, 1,
        "the photograph that failed in the halting batch must be counted; \
         publishing only on success reports 0 here"
    );
    assert_eq!(progress.files_done, 2, "both readable photographs were catalogued");
}

/// `docs/06` stage 3: a batch is on record from the moment it is claimed, and
/// the run beats while it works.
///
/// Both are how anything other than the running process can tell whether a scan
/// is alive. Before this, a batch row appeared only once the batch had
/// finished, so the batch a run died inside left no trace, and `outcome` stayed
/// 'running' for ever with nothing to weigh it against.
#[test]
fn a_batch_is_recorded_when_it_starts_and_the_run_beats_while_it_works() {
    let (h, opts) = setup(no_disk_floor());
    let summary = pipeline(&h).run(&opts).unwrap();

    // Every batch that ran is closed, because this run finished.
    let (batches, open): (i64, i64) = h
        .archive
        .query_row(
            "SELECT count(*), sum(ended_at IS NULL) FROM scan_batches WHERE run_id=?1",
            [&summary.run_id],
            |r| Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0))),
        )
        .unwrap();
    assert!(batches >= 1, "the run's batches must be recorded");
    assert_eq!(open, 0, "a finished run leaves no batch open");

    let (heartbeat, outcome): (Option<String>, String) = h
        .archive
        .query_row(
            "SELECT heartbeat_at, outcome FROM scan_runs WHERE id=?1",
            [&summary.run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(heartbeat.is_some(), "a run that indexed photographs must have beaten");
    assert_eq!(outcome, "success");
    assert!(
        crate::inventory::running_scans(&h.archive).unwrap().is_empty(),
        "a finished run is not a running one"
    );
}

/// The batch a run dies inside stays open, which is what says where it was.
#[test]
fn a_halting_run_leaves_the_batch_it_stopped_inside_open() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();
    h.archive
        .execute(
            "UPDATE files SET perceptual_hash = NULL
             WHERE id = (SELECT id FROM files WHERE status='complete' LIMIT 1)",
            [],
        )
        .unwrap();
    for i in 0..3 {
        write_photo(&h.drive_dir.join(format!("later/n{i}.png")), [10 * i as u8, 90, 140], 40, 30);
    }
    let mut opts2 = opts.clone();
    opts2.config = Config { batch_size: 1, ..no_disk_floor() };
    let err = p.run(&opts2).expect_err("three verifier failures must halt the run");
    assert_eq!(err.exit_code(), crate::error::exit::REPEATED_VERIFIER_FAILURE);

    let open: i64 = h
        .archive
        .query_row("SELECT count(*) FROM scan_batches WHERE ended_at IS NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(open, 1, "the batch the run halted inside is still open");
}

/// Incremental rescan: a file edited since indexing is re-analysed, and a file
/// that has gone away is marked missing rather than silently left as complete.
#[test]
fn incremental_rescan_reanalyses_changed_and_marks_missing() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    let first = p.run(&opts).unwrap();
    assert_eq!(first.files_done, 3);
    assert_eq!(first.files_changed, 0, "nothing can be changed on a first scan");
    assert_eq!(first.files_missing, 0);

    // Compare the stored visual embedding rather than the perceptual hash: the
    // fixtures are flat colour blocks, whose phash is all zeros whatever the
    // colour, while the embedding is exactly what colour drives.
    let embedding = |h: &Harness| -> Vec<u8> {
        h.archive
            .query_row(
                "SELECT ve.vector FROM visual_embeddings ve
                   JOIN files f ON f.id = ve.file_id WHERE f.filename='beach.png'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let embedding_before = embedding(&h);

    // Edit one original (a different colour, so its analysis must differ) and
    // delete another entirely.
    write_photo(&h.drive_dir.join("holiday/beach.png"), [220, 30, 30], 80, 60);
    std::fs::remove_file(h.drive_dir.join("family/xmas_1987.png")).unwrap();

    let second = p.run(&opts).unwrap();
    assert_eq!(second.files_changed, 1, "the edited file should be re-queued");
    assert_eq!(second.files_missing, 1, "the deleted file should be marked missing");
    assert_eq!(second.files_done, 1, "only the changed file needs re-analysis");

    // The changed file is complete again, with freshly computed analysis.
    let status: String = h
        .archive
        .query_row("SELECT status FROM files WHERE filename='beach.png'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "complete");
    assert_ne!(
        embedding(&h),
        embedding_before,
        "re-analysis must recompute the visual embedding"
    );

    // The deleted file is marked missing, not deleted from the catalogue —
    // the user still needs to know it was once on Drive 14.
    let missing: String = h
        .archive
        .query_row(
            "SELECT status FROM files WHERE filename='xmas_1987.png'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(missing, "missing");

    // No rows were lost.
    let total: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 3);
}

/// A rescan that finds nothing new must do nothing at all — no re-analysis, no
/// status churn. This is what keeps repeated scans cheap.
#[test]
fn rescan_with_no_changes_is_a_no_op() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    let second = p.run(&opts).unwrap();
    assert_eq!(second.files_changed, 0);
    assert_eq!(second.files_missing, 0);
    assert_eq!(second.files_done, 0, "nothing should be re-processed");

    let complete: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE status='complete'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(complete, 3);
}

/// A file that returns after being marked missing is re-analysed and restored,
/// rather than being stranded in the missing state forever.
#[test]
fn a_returning_file_is_restored_by_the_next_rescan() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    let path = h.drive_dir.join("family/xmas_1987.png");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(p.run(&opts).unwrap().files_missing, 1);

    // The drive is reconnected / the file is restored.
    std::fs::write(&path, &bytes).unwrap();
    let third = p.run(&opts).unwrap();
    assert_eq!(third.files_changed, 1, "a returning file must be re-analysed");

    let status: String = h
        .archive
        .query_row("SELECT status FROM files WHERE filename='xmas_1987.png'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "complete");
}

/// The drive is pulled out mid-run. Nothing may be lost or corrupted: files
/// already committed stay complete, the unreadable ones are isolated rather
/// than fatal, and the queue stays consistent so a later run finishes the job.
#[test]
fn drive_disconnected_mid_batch_is_survivable_and_resumable() {
    // One file per batch, so the disconnection lands between batches the way a
    // real unplug would.
    let (h, opts) = setup(Config { batch_size: 1, ..no_disk_floor() });
    let p = pipeline(&h);

    // Index the first file only, by cancelling after one batch.
    let cancel_after_one = Pipeline {
        archive: &h.archive,
        queue: &h.queue,
        paths: &h.paths,
        engines: Arc::new(EngineRegistry::local_default()),
        key: &h.key,
        logger: Logger::new(h.paths.index_log()),
        cancel: CancelToken::new(),
    };
    cancel_after_one.cancel.cancel();
    let stopped = cancel_after_one.run(&opts).unwrap();
    assert_eq!(stopped.files_done, 0, "cancelled before any batch ran");

    // Now the volume disappears entirely — the drive was unplugged.
    let backup = h.drive_dir.with_extension("unplugged");
    std::fs::rename(&h.drive_dir, &backup).unwrap();

    // A run against a vanished drive must fail cleanly, not panic or corrupt.
    //
    // Reported as a disconnection rather than the older "scan path is not a
    // directory": both are clean refusals, but only one tells the owner that
    // the drive is unplugged and that reconnecting it will do. The same cause
    // now reads the same way whether it is noticed at preflight or mid-scan.
    let err = p.run(&opts).expect_err("scanning a vanished volume must fail");
    assert!(
        matches!(err, crate::error::Error::DriveDisconnected(_)),
        "expected a clear 'drive disconnected' error, got {err:?}"
    );
    assert!(!err.is_hard_halt(), "an unplugged drive is not a safety halt");

    // The catalogue is intact and no file was falsely marked complete.
    assert!(crate::db::integrity_check(&h.archive).is_ok());
    let complete: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE status='complete'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(complete, 0);

    // Reconnect the drive: the run picks up and finishes everything.
    std::fs::rename(&backup, &h.drive_dir).unwrap();
    let finished = p.run(&opts).unwrap();
    assert_eq!(finished.files_done, 3, "all work completes after reconnection");
    assert!(!finished.halted);

    let complete: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE status='complete'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(complete, 3);

    // And the queue agrees with the catalogue.
    let mut vo = opts.clone();
    vo.mode = IndexMode::VerifyOnly;
    p.run(&vo).unwrap();
}

/// An individual original vanishing *between* being queued and being read is a
/// per-file failure, not a run-ending one: the rest of the batch still indexes.
#[test]
fn a_file_vanishing_mid_run_is_isolated_not_fatal() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);

    // Remove one original after it was written but before the run reads it.
    std::fs::remove_file(h.drive_dir.join("scan.png")).unwrap();

    let summary = p.run(&opts).unwrap();
    assert_eq!(summary.files_done, 2, "the two readable files still index");
    assert!(!summary.halted, "one unreadable file must not halt the run");

    let complete: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE status='complete'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(complete, 2);
}

/// The scan root vanishing mid-run is a disconnected drive, not a safety event.
///
/// This is the exact condition that ended a two-day scan of Drive 10 with
/// "Stopped for safety": the volume was unplugged without ejecting, so
/// `canonicalize` returned "No such file or directory", and *every* failure to
/// canonicalize was reported as `UnsafePath` — a hard safety halt. Pulling an
/// external drive is ordinary, and the pipeline is built to survive it.
///
/// Tested at the function that makes the decision rather than through a whole
/// run: a real mid-run unplug lands between preflight and a file being read,
/// and racing the filesystem to hit that window would only buy a flaky test.
#[test]
fn a_vanished_scan_root_is_a_disconnected_drive_not_an_unsafe_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("NOV 22 A");
    std::fs::create_dir_all(&root).unwrap();
    let photo = root.join("shoot/_DSC9418-Edit-Edit.tif");
    std::fs::create_dir_all(photo.parent().unwrap()).unwrap();
    std::fs::write(&photo, b"x").unwrap();

    // While the drive is present the path resolves normally.
    crate::scan::ensure_contained(&root, &photo).unwrap();

    // The drive is unplugged mid-scan.
    std::fs::remove_dir_all(&root).unwrap();

    let err = crate::scan::ensure_contained(&root, &photo).unwrap_err();
    assert!(
        matches!(err, crate::error::Error::DriveDisconnected(_)),
        "an unplugged drive must not read as an unsafe path, got: {err}"
    );
    assert!(
        !err.is_hard_halt(),
        "unplugging a drive must not stop the scan for safety"
    );
    assert_eq!(err.exit_code(), crate::error::exit::DRIVE_DISCONNECTED);
    // The message tells the owner what to do, not what a syscall returned.
    assert!(
        format!("{err}").contains("reconnect the drive"),
        "unhelpful message: {err}"
    );
}

/// Handing work back on a disconnection must not spend the item's attempts.
#[test]
fn releasing_work_returns_it_unspent() {
    let (h, opts) = setup(no_disk_floor());
    pipeline(&h).run(&opts).unwrap();

    let drive_id: String = h
        .archive
        .query_row("SELECT id FROM drives WHERE drive_number = 14", [], |r| r.get(0))
        .unwrap();
    let q = crate::queue::Queue::new(&h.queue);

    // Re-queue one item, claim it, then release it as a disconnection would.
    let id: String = h
        .queue
        .query_row("SELECT id FROM queue_items LIMIT 1", [], |r| r.get(0))
        .unwrap();
    h.queue
        .execute("UPDATE queue_items SET state='queued', attempts=0 WHERE id=?1", [&id])
        .unwrap();

    let claimed = q.claim_batch(&drive_id, 1, 300, "w").unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].attempts, 1, "claiming counts as an attempt");

    q.release(&claimed[0].id).unwrap();

    let (state, attempts): (String, i64) = h
        .queue
        .query_row("SELECT state, attempts FROM queue_items WHERE id=?1", [&id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(state, "queued", "released work waits, it does not fail");
    assert_eq!(attempts, 0, "a drive leaving must not count against the photograph");
}

/// The safety half of the same distinction must not be weakened: a path that
/// genuinely escapes the approved root is still a hard halt.
#[test]
fn a_path_escaping_the_root_is_still_a_hard_halt() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let secret = outside.join("elsewhere.jpg");
    std::fs::write(&secret, b"x").unwrap();

    let err = crate::scan::ensure_contained(&root, &secret).unwrap_err();
    assert!(
        matches!(err, crate::error::Error::UnsafePath(_)),
        "escaping the root must stay an unsafe path, got: {err}"
    );
    assert!(err.is_hard_halt(), "an escape must still stop the run");

    // A '..' component is refused before the filesystem is touched at all.
    let traversal = crate::scan::ensure_contained(&root, std::path::Path::new("../etc/passwd"));
    assert!(matches!(
        traversal.unwrap_err(),
        crate::error::Error::UnsafePath(_)
    ));
}

/// A single original disappearing is that photograph's problem, not the run's.
#[test]
fn one_missing_original_is_not_an_unsafe_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).unwrap();

    let err = crate::scan::ensure_contained(&root, &root.join("gone.jpg")).unwrap_err();
    assert!(
        matches!(err, crate::error::Error::NotFound(_)),
        "an absent file is not a dangerous path, got: {err}"
    );
    assert!(!err.is_hard_halt(), "a missing photograph must not stop the run");
}

/// A real HEIC photograph indexes end to end on macOS: thumbnail, hash and
/// embedding all produced, and the original left untouched.
#[cfg(target_os = "macos")]
#[test]
fn indexes_a_real_heic_photograph_on_macos() {
    let (h, mut opts) = setup(no_disk_floor());
    // HEIC is not a default type (D-029) — a scan opts into it.
    opts.extra_extensions = vec!["heic".into()];

    // Build a genuine HEIC with the system tool, then index it alongside the
    // ordinary fixtures.
    let png = h.drive_dir.join("holiday/tmp_source.png");
    write_photo(&png, [40, 90, 200], 64, 48);
    let heic = h.drive_dir.join("holiday/IMG_2001.heic");
    let made = std::process::Command::new("/usr/bin/sips")
        .args(["-s", "format", "heic"])
        .arg(&png)
        .arg("--out")
        .arg(&heic)
        .output()
        .expect("sips must exist on macOS");
    assert!(made.status.success(), "could not create a HEIC fixture");
    std::fs::remove_file(&png).unwrap();

    let before = std::fs::metadata(&heic).unwrap();
    let summary = pipeline(&h).run(&opts).unwrap();
    assert_eq!(summary.files_failed, 0, "the HEIC must not fail to decode");
    assert_eq!(summary.files_done, 4);

    // It is catalogued like any other photograph.
    let (status, phash): (String, Option<String>) = h
        .archive
        .query_row(
            "SELECT status, perceptual_hash FROM files WHERE filename='IMG_2001.heic'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "complete");
    assert!(phash.is_some(), "HEIC must get a perceptual hash");

    let thumbs: i64 = h
        .archive
        .query_row(
            "SELECT count(*) FROM thumbnails t JOIN files f ON f.id=t.file_id
              WHERE f.filename='IMG_2001.heic'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(thumbs, 1);

    // The original HEIC is byte-for-byte untouched.
    let after = std::fs::metadata(&heic).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
}

/// A date the user corrected is the highest authority: re-analysing the
/// photograph must not quietly replace it with the estimator's guess.
#[test]
fn a_user_date_override_survives_reanalysis() {
    use crate::dates::DateRepo;

    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    let file_id: String = h
        .archive
        .query_row("SELECT id FROM files WHERE filename='beach.png'", [], |r| r.get(0))
        .unwrap();

    let repo = DateRepo::new(&h.archive);
    let estimated = repo.get(&file_id).unwrap().expect("an estimate is stored");
    assert!(!estimated.is_user_confirmed);

    // The user knows this one: it was their honeymoon, August 1998.
    let confirmed = repo
        .set_user_override(&file_id, "1998-08-12", "1998-08-12")
        .unwrap();
    assert!(confirmed.is_user_confirmed);
    assert_eq!(crate::dates::describe(&confirmed), "Taken on 1998-08-12");

    // Force a full re-analysis of that file by changing it on disk.
    write_photo(&h.drive_dir.join("holiday/beach.png"), [10, 200, 90], 80, 60);
    let second = p.run(&opts).unwrap();
    assert_eq!(second.files_changed, 1);

    let after = repo.get(&file_id).unwrap().unwrap();
    assert!(after.is_user_confirmed, "the correction must survive re-analysis");
    assert_eq!(after.earliest_date, "1998-08-12");
    assert_eq!(after.latest_date, "1998-08-12");

    // Clearing it hands authority back to the estimator on the next run.
    repo.clear_user_override(&file_id).unwrap();
    assert!(repo.get(&file_id).unwrap().is_none());
}

/// With Apple Vision registered, the catalogue records what a photograph
/// actually shows and the words visible inside it — and both become searchable.
///
/// This is the difference between the heuristic engine and real understanding,
/// so it is asserted end to end rather than at the engine boundary.
#[cfg(target_os = "macos")]
#[test]
fn vision_records_real_labels_and_readable_text() {
    use crate::search::{SearchFilters, SearchRepo};

    // Skip when the Swift worker has not been built in this checkout.
    if crate::ai::vision::VisionEngine::detect().is_none() {
        return;
    }

    let (h, opts) = setup(no_disk_floor());

    // An image with unmistakable content: rendered text on a page. Vision should
    // classify it as a document and read the words back.
    let doc = h.drive_dir.join("papers/letter.png");
    std::fs::create_dir_all(doc.parent().unwrap()).unwrap();
    render_text_image(&doc, "MARGARET");

    let p = Pipeline {
        archive: &h.archive,
        queue: &h.queue,
        paths: &h.paths,
        engines: Arc::new(EngineRegistry::local_with_vision()),
        key: &h.key,
        logger: Logger::new(h.paths.index_log()),
        cancel: CancelToken::new(),
    };
    let summary = p.run(&opts).unwrap();
    assert_eq!(summary.files_failed, 0);

    // The analysis is attributed to Vision, not the heuristic engine.
    let (model, description): (String, String) = h
        .archive
        .query_row(
            "SELECT s.model_id, s.description FROM scene_analysis s
               JOIN files f ON f.id = s.file_id WHERE f.filename='letter.png'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(model, crate::ai::vision::MODEL_ID);
    assert!(!description.is_empty());

    // Embeddings land in Vision's own partition at its own dimension, never
    // mixed with the heuristic engine's.
    let (dim, count): (i64, i64) = h
        .archive
        .query_row(
            "SELECT dim, count(*) FROM visual_embeddings WHERE model_id=?1",
            [crate::ai::vision::MODEL_ID],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(dim, 768);
    assert!(count > 0);

    // The word is only present as pixels — not in the filename, not in the path.
    let ocr: String = h
        .archive
        .query_row(
            "SELECT COALESCE(s.ocr_text,'') FROM scene_analysis s
               JOIN files f ON f.id = s.file_id WHERE f.filename='letter.png'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        ocr.to_uppercase().contains("MARGARET"),
        "Vision should have read the text; got {ocr:?}"
    );

    // And that makes it findable by searching for what the photograph says.
    let repo = SearchRepo::new(&h.archive);
    let hits = repo
        .text_search(
            "MARGARET",
            &SearchFilters { include_offline: true, limit: 10, ..Default::default() },
        )
        .unwrap();
    assert!(
        hits.iter().any(|r| r.filename == "letter.png"),
        "recognised text must be searchable"
    );
}

/// Draw large block letters onto a white page, so Vision has real text to read
/// without needing a font renderer or a checked-in photograph.
#[cfg(target_os = "macos")]
fn render_text_image(path: &Path, word: &str) {
    // A coarse 5x7 block font, enough for Vision's text recogniser.
    const GLYPHS: &[(char, [&str; 7])] = &[
        ('A', ["00100", "01010", "01010", "10001", "11111", "10001", "10001"]),
        ('E', ["11111", "10000", "10000", "11110", "10000", "10000", "11111"]),
        ('G', ["01110", "10001", "10000", "10111", "10001", "10001", "01110"]),
        ('M', ["10001", "11011", "10101", "10001", "10001", "10001", "10001"]),
        ('R', ["11110", "10001", "10001", "11110", "10010", "10010", "10001"]),
        ('T', ["11111", "00100", "00100", "00100", "00100", "00100", "00100"]),
    ];
    let scale = 26u32;
    let pad = 60u32;
    let letters: Vec<&[&str; 7]> = word
        .chars()
        .filter_map(|c| GLYPHS.iter().find(|(g, _)| *g == c).map(|(_, rows)| rows))
        .collect();
    let w = pad * 2 + letters.len() as u32 * 7 * scale;
    let h = pad * 2 + 7 * scale;
    let mut img = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
    for (i, rows) in letters.iter().enumerate() {
        let ox = pad + i as u32 * 7 * scale;
        for (ry, row) in rows.iter().enumerate() {
            for (cx, ch) in row.chars().enumerate() {
                if ch != '1' {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let x = ox + cx as u32 * scale + dx;
                        let y = pad + ry as u32 * scale + dy;
                        if x < w && y < h {
                            img.put_pixel(x, y, Rgb([15, 15, 15]));
                        }
                    }
                }
            }
        }
    }
    img.save(path).unwrap();
}

/// The catalogue must answer "what is on this drive?" and "which drive do I
/// need?" with the drive physically gone. This is the product's core promise, so
/// the test removes the volume entirely rather than just marking it offline.
#[test]
fn the_catalogue_describes_a_drive_that_is_no_longer_connected() {
    use crate::inventory::{drive_contents, drives_matching, locate_matches, where_to_look};
    use crate::search::{SearchFilters, SearchRepo};

    let (h, opts) = setup(no_disk_floor());
    pipeline(&h).run(&opts).unwrap();

    // Record where the drive is kept, then unplug it: mark it offline and delete
    // the volume from disk so nothing can secretly read from it.
    {
        let repo = DriveRepo::new(&h.archive);
        let d = repo.get_by_number(14).unwrap().unwrap();
        repo.update_details(&d.id, Some("Drawer 2"), Some(&["holidays".into()]))
            .unwrap();
        repo.set_status(&d.id, "offline").unwrap();
    }
    std::fs::remove_dir_all(&h.drive_dir).unwrap();

    // 1. What is on it?
    let contents = drive_contents(&h.archive, Some(14)).unwrap();
    assert_eq!(contents.len(), 1);
    let c = &contents[0];
    assert_eq!(c.photo_count, 3, "the catalogue still knows what it holds");
    assert!(!c.online);
    assert_eq!(c.physical_location.as_deref(), Some("Drawer 2"));
    let summary = c.summary();
    assert!(summary.contains("Drive 14"), "got {summary}");
    assert!(summary.contains("3 photographs"), "got {summary}");
    assert!(summary.contains("Disconnected."), "got {summary}");
    assert!(summary.contains("Kept in Drawer 2."), "got {summary}");

    // 2. Which drive do I need?
    let repo = SearchRepo::new(&h.archive);
    let results = repo
        .text_search(
            "beach",
            &SearchFilters { include_offline: true, limit: 50, ..Default::default() },
        )
        .unwrap();
    assert!(!results.is_empty(), "offline search must still find photographs");

    let mut grouped = drives_matching(&results);
    locate_matches(&h.archive, &mut grouped).unwrap();
    assert_eq!(grouped[0].drive_number, 14);
    assert!(!grouped[0].online);

    let line = where_to_look(&grouped);
    assert!(line.contains("Found on Drive 14"), "got {line}");
    assert!(
        line.contains("Connect Drive 14 (Drawer 2)"),
        "the user must be told which physical disk to fetch; got {line}"
    );

    // Every drive can be inventoried at once, too.
    assert_eq!(drive_contents(&h.archive, None).unwrap().len(), 1);
}

/// Gathering a person's photographs copies them out and leaves every original
/// exactly as it was — the whole point of the feature being safe to use.
#[test]
fn copying_photographs_out_never_touches_the_originals() {
    use crate::export;

    let (h, opts) = setup(no_disk_floor());
    pipeline(&h).run(&opts).unwrap();

    let ids: Vec<String> = {
        let mut stmt = h
            .archive
            .prepare("SELECT id FROM files WHERE status='complete' ORDER BY filename")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(ids.len(), 3);

    // Fingerprint every original before the copy.
    let before: Vec<_> = std::fs::read_dir(h.drive_dir.join("holiday"))
        .unwrap()
        .chain(std::fs::read_dir(h.drive_dir.join("family")).unwrap())
        .filter_map(|e| e.ok())
        .map(|e| {
            let m = e.metadata().unwrap();
            (e.path(), m.len(), m.modified().unwrap())
        })
        .collect();

    let dest = h.paths.root.join("exported");
    let summary = export::copy_photos(&h.archive, &ids, &dest).unwrap();
    assert_eq!(summary.copied, 3);
    assert_eq!(summary.missing, 0);

    // The copies exist, prefixed with the drive number so two drives holding the
    // same filename cannot overwrite each other.
    let copied: Vec<String> = std::fs::read_dir(&dest)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(copied.len(), 3);
    assert!(copied.iter().all(|n| n.starts_with("drive14_")), "got {copied:?}");

    // Every original is byte-identical and untouched.
    for (path, len, modified) in before {
        let m = std::fs::metadata(&path).unwrap();
        assert_eq!(m.len(), len, "{} changed size", path.display());
        assert_eq!(m.modified().unwrap(), modified, "{} was rewritten", path.display());
    }

    // Running it again copies nothing new rather than duplicating.
    let again = export::copy_photos(&h.archive, &ids, &dest).unwrap();
    assert_eq!(again.copied, 0);
    assert_eq!(again.skipped_existing, 3);
}

/// A face crop is stored for each detected face, so the gallery works with the
/// drive unplugged.
#[test]
fn face_crops_are_stored_locally_and_survive_disconnection() {
    let (h, opts) = setup(no_disk_floor());
    // A skin-tone rectangle the heuristic detector will find.
    let mut img = RgbImage::from_pixel(400, 300, Rgb([20, 20, 30]));
    for y in 80..220 {
        for x in 120..280 {
            img.put_pixel(x, y, Rgb([205, 160, 130]));
        }
    }
    std::fs::create_dir_all(h.drive_dir.join("people")).unwrap();
    img.save(h.drive_dir.join("people/portrait.png")).unwrap();

    pipeline(&h).run(&opts).unwrap();

    let repo = crate::faces::FaceRepo::new(&h.archive);
    let gallery = repo.gallery(50).unwrap();
    assert!(!gallery.is_empty(), "a face crop should have been stored");

    // Unplug the drive entirely.
    std::fs::remove_dir_all(&h.drive_dir).unwrap();

    // The crop is still readable, and is a real decodable image.
    let (bytes, format) = repo
        .thumbnail(&gallery[0].face_id, &h.key)
        .unwrap()
        .expect("crop is stored locally");
    assert_eq!(format, "jpeg");
    let decoded = image::load_from_memory(&bytes).expect("a valid image");
    assert!(decoded.width() <= crate::faces::FACE_THUMBNAIL_EDGE);
    assert!(decoded.height() <= crate::faces::FACE_THUMBNAIL_EDGE);

    // And it is encrypted at rest, not sitting in the clear.
    let raw: Vec<u8> = h
        .archive
        .query_row(
            "SELECT ciphertext FROM face_thumbnails LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(raw, bytes, "the stored bytes must not be the plain image");
    // Not a bare JPEG either — SOI marker would be the giveaway.
    assert_ne!(&raw[..2.min(raw.len())], b"\xff\xd8");
}

/// A scan indexes the delivery formats and leaves RAW alone unless asked.
///
/// This is a deliberate default, not a limitation: a working drive holds far
/// more RAW than anything else, and cataloguing negatives nobody searches for
/// would triple the scan for no benefit.
#[test]
fn raw_files_are_skipped_unless_explicitly_requested() {
    let (h, opts) = setup(no_disk_floor());

    // Delivery formats alongside their RAW negatives, as a real shoot looks.
    write_photo(&h.drive_dir.join("shoot/_DSC0001.png"), [90, 140, 200], 60, 40);
    std::fs::write(h.drive_dir.join("shoot/_DSC0001.arw"), b"raw bytes").unwrap();
    std::fs::write(h.drive_dir.join("shoot/_DSC0002.cr2"), b"raw bytes").unwrap();
    std::fs::write(h.drive_dir.join("shoot/_DSC0001.xmp"), b"<x:xmpmeta/>").unwrap();

    let summary = pipeline(&h).run(&opts).unwrap();
    // The three fixture photos plus the new PNG — no RAW, no sidecar.
    assert_eq!(summary.files_discovered, 4);

    let indexed: Vec<String> = {
        let mut stmt = h.archive.prepare("SELECT filename FROM files").unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    assert!(indexed.iter().any(|f| f == "_DSC0001.png"));
    assert!(!indexed.iter().any(|f| f.ends_with(".arw")), "RAW must be skipped");
    assert!(!indexed.iter().any(|f| f.ends_with(".cr2")), "RAW must be skipped");
    assert!(!indexed.iter().any(|f| f.ends_with(".xmp")), "sidecars are not photographs");
}

#[test]
fn the_default_file_types_are_the_delivery_formats() {
    use crate::scan::{is_supported_extension, ScanOptions};

    for wanted in ["jpg", "jpeg", "png", "tif", "tiff", "psd"] {
        assert!(is_supported_extension(wanted), "{wanted} should be indexed by default");
    }
    for skipped in ["arw", "cr2", "cr3", "nef", "dng", "rw2", "xmp"] {
        assert!(!is_supported_extension(skipped), "{skipped} must not be indexed by default");
    }

    // Opting in is per scan, and case-insensitive.
    let opts = ScanOptions { extra_extensions: vec!["arw".into()], ..Default::default() };
    assert!(opts.accepts("ARW"));
    assert!(opts.accepts("jpg"));
    assert!(!opts.accepts("cr2"), "opting into one type must not open the floodgates");
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

/// A file that moved is the same photograph, and must be adopted rather than
/// re-analysed. The real case: a drive first scanned from one wedding folder,
/// then rescanned from the drive root — 758 photographs reappeared under
/// longer paths, went missing under their old ones, and everything the owner
/// had confirmed about them (names above all) hung off the old rows.
#[test]
fn a_moved_file_is_adopted_with_everything_it_carries() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    let old_id: String = h
        .archive
        .query_row("SELECT id FROM files WHERE filename='xmas_1987.png'", [], |r| r.get(0))
        .unwrap();
    // Something user-confirmed hangs off the old row, standing in for names,
    // tags and dates. Adoption must carry it; re-analysis would orphan it.
    h.archive
        .execute(
            "UPDATE date_estimates
                SET earliest_date='1987-12-25', latest_date='1987-12-25',
                    is_user_confirmed=1
              WHERE file_id=?1",
            [&old_id],
        )
        .unwrap();

    // The photograph moves to a new folder on the same drive.
    let from = h.drive_dir.join("family/xmas_1987.png");
    let to_dir = h.drive_dir.join("sorted/1987");
    std::fs::create_dir_all(&to_dir).unwrap();
    std::fs::rename(&from, to_dir.join("xmas_1987.png")).unwrap();

    // One rescan marks it missing; the next finds it at the new path.
    p.run(&opts).unwrap();
    p.run(&opts).unwrap();

    // Same row, new address — not a stranger with a fresh id.
    let (id, rel, status): (String, String, String) = h
        .archive
        .query_row(
            "SELECT id, relative_path, status FROM files WHERE filename='xmas_1987.png'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(id, old_id, "adoption must keep the row, or names and tags are orphaned");
    assert_eq!(rel, "sorted/1987/xmas_1987.png");
    assert_eq!(status, "complete");

    // No duplicate row survives, and the confirmed date rode along.
    let rows: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE filename='xmas_1987.png'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    let confirmed: i64 = h
        .archive
        .query_row(
            "SELECT count(*) FROM date_estimates WHERE file_id=?1 AND is_user_confirmed=1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(confirmed, 1, "what the owner confirmed must survive the move");
}

/// Adoption must not fire for a file that merely looks similar — only an
/// identical file (same content hash) on the same drive is the same photograph.
#[test]
fn a_genuinely_new_file_is_not_mistaken_for_a_moved_one() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    // Remove one file, and add a *different* photograph elsewhere.
    std::fs::remove_file(h.drive_dir.join("family/xmas_1987.png")).unwrap();
    let new_dir = h.drive_dir.join("new");
    std::fs::create_dir_all(&new_dir).unwrap();
    write_photo(&new_dir.join("different.png"), [7, 200, 40], 64, 48);

    p.run(&opts).unwrap();

    let missing: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE status='missing'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(missing, 1, "the vanished file stays missing");
    let complete: i64 = h
        .archive
        .query_row("SELECT count(*) FROM files WHERE status='complete'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(complete, 3, "two originals plus the genuinely new photograph");
}

/// A stop asked for while a run is under way is obeyed, leaves the queue
/// intact, and is consumed so it cannot haunt the next run.
#[test]
fn a_stop_request_interrupts_without_losing_work() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);

    // A request belongs to the run that is going when it is made. The runs here
    // are far too quick to press Stop during, so the request is dated a minute
    // ahead: to any run starting now it reads as "asked for after you began",
    // which is exactly the case being tested.
    crate::stop::request(&h.paths).unwrap();
    let ahead = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    filetime::set_file_mtime(
        h.paths.stop_flag(),
        filetime::FileTime::from_system_time(ahead),
    )
    .unwrap();

    let summary = p.run(&opts).unwrap();
    assert_eq!(summary.files_done, 0, "nothing should be processed after a stop");
    assert!(!summary.halted, "a stop is an interruption, not a failure");
    assert!(
        !crate::stop::requested(&h.paths),
        "the run that obeys a stop must also consume it"
    );

    // The queue survived, so the same run picks up where it left off.
    let resumed = p.run(&opts).unwrap();
    assert_eq!(resumed.files_done, 3, "the queue must have survived the stop");
}

/// A stop request left over from an earlier scan must not stop a new one.
///
/// This is the bug that made a stopped drive impossible to rescan: the request
/// is a file, it outlived the scan it was meant for, and every later run found
/// it at the first batch boundary and quit. The desktop app hid it by deleting
/// the file in its own start command; a scan started from the command line had
/// no such protection and stopped dead every time, for ever.
#[test]
fn a_stop_left_over_from_a_previous_scan_does_not_block_the_next_one() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);

    // Yesterday's Stop, still sitting on disk.
    crate::stop::request(&h.paths).unwrap();
    let yesterday = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 60 * 60);
    filetime::set_file_mtime(
        h.paths.stop_flag(),
        filetime::FileTime::from_system_time(yesterday),
    )
    .unwrap();

    let summary = p.run(&opts).unwrap();
    assert_eq!(
        summary.files_done, 3,
        "a stop from before this run began must not stop it"
    );
    assert!(!summary.halted);
}

/// A photograph with real structure and a size of its own, so that a result
/// written to the wrong row cannot pass unnoticed (the same reasoning as D-044).
fn write_distinct_photo(path: &Path, i: u32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let (w, h) = (50 + i * 7, 30 + i * 3);
    let img = RgbImage::from_fn(w, h, |x, y| {
        Rgb([
            ((x * (i + 3)) % 256) as u8,
            ((y * (i + 5)) % 256) as u8,
            (((x + y) * 7 + i * 31) % 256) as u8,
        ])
    });
    img.save(path).unwrap();
}

/// Everything the catalogue says about each photograph, keyed by path.
fn catalogue_by_path(conn: &rusqlite::Connection) -> std::collections::BTreeMap<String, String> {
    let mut stmt = conn
        .prepare(
            "SELECT f.relative_path, f.perceptual_hash, f.content_hash, m.width, m.height,
                    t.width, t.height, t.checksum, s.description,
                    (SELECT group_concat(name, ',') FROM
                        (SELECT tg.name FROM file_tags ft JOIN tags tg ON tg.id = ft.tag_id
                          WHERE ft.file_id = f.id ORDER BY tg.name)),
                    (SELECT count(*) FROM faces fc WHERE fc.file_id = f.id)
               FROM files f
               JOIN metadata m ON m.file_id = f.id
               JOIN thumbnails t ON t.file_id = f.id
               JOIN scene_analysis s ON s.file_id = f.id",
        )
        .unwrap();
    stmt.query_map([], |r| {
        let mut row = Vec::new();
        for i in 1..11 {
            row.push(format!("{:?}", r.get::<_, rusqlite::types::Value>(i)?));
        }
        Ok((r.get::<_, String>(0)?, row.join("|")))
    })
    .unwrap()
    .map(|r| r.unwrap())
    .collect()
}

/// D-087: photographs analysed at the same time are each written to their own
/// row. Every result is checked against its own original, read independently.
#[test]
fn parallel_analysis_attributes_every_result_to_its_own_photograph() {
    let (h, mut opts) = setup(no_disk_floor());
    for i in 0..24 {
        write_distinct_photo(&h.drive_dir.join(format!("many/p{i:02}.png")), i);
    }
    opts.config.analysis_workers = 4;
    opts.config.batch_size = 8;
    let summary = pipeline(&h).run(&opts).unwrap();
    assert_eq!(summary.files_done, 27);
    assert_eq!(summary.files_failed, 0);

    let mut stmt = h
        .archive
        .prepare(
            "SELECT f.relative_path, f.perceptual_hash, m.width, m.height, t.width, t.height
               FROM files f
               JOIN metadata m ON m.file_id = f.id
               JOIN thumbnails t ON t.file_id = f.id",
        )
        .unwrap();
    let rows: Vec<(String, String, u32, u32, u32, u32)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(rows.len(), 27);
    for (rel, phash, mw, mh, tw, th) in rows {
        let original = image::open(h.drive_dir.join(&rel)).unwrap().to_rgb8();
        let (w, h_) = original.dimensions();
        assert_eq!((mw, mh), (w, h_), "{rel}: metadata describes another photograph");
        assert_eq!((tw, th), (w, h_), "{rel}: thumbnail belongs to another photograph");
        assert_eq!(phash, crate::pipeline::phash::dhash(&original), "{rel}: hash of another photograph");
    }
}

/// D-087: indexing four at a time builds exactly the catalogue that indexing
/// one at a time does — the same hashes, thumbnails, descriptions, tags and
/// faces for every path.
#[test]
fn one_worker_and_four_build_the_same_catalogue() {
    let build = |workers: usize| {
        let (h, mut opts) = setup(no_disk_floor());
        for i in 0..16 {
            write_distinct_photo(&h.drive_dir.join(format!("many/p{i:02}.png")), i);
        }
        opts.config.analysis_workers = workers;
        opts.config.batch_size = 5;
        pipeline(&h).run(&opts).unwrap();
        catalogue_by_path(&h.archive)
    };
    let serial = build(1);
    let parallel = build(4);
    assert_eq!(serial.len(), 19);
    assert_eq!(serial, parallel);
}

/// D-087: a batch verifies the photographs it wrote, not the whole archive
/// again; the drive as a whole is read back once, when the scan ends.
#[test]
fn batches_verify_their_own_photographs_and_the_drive_is_checked_at_the_end() {
    use crate::verifier::{self, Scope, VerifyContext};

    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    p.run(&opts).unwrap();

    // Damage one thumbnail from that first run.
    let (damaged, rel): (String, String) = h
        .archive
        .query_row("SELECT file_id, rel_path FROM thumbnails LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    std::fs::write(h.paths.thumbnails_dir().join(&rel), b"not a jpeg").unwrap();

    let drive_id: String = h
        .archive
        .query_row("SELECT id FROM drives WHERE drive_number = 14", [], |r| r.get(0))
        .unwrap();
    let others: Vec<String> = {
        let mut stmt = h.archive.prepare("SELECT id FROM files WHERE id <> ?1").unwrap();
        stmt.query_map([&damaged], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
    };
    let config = no_disk_floor();
    let ctx = VerifyContext {
        archive: &h.archive,
        queue: Some(&h.queue),
        paths: &h.paths,
        config: &config,
        key: Some(&h.key),
        observed_throughput: None,
        network_blocked_attempts: 0,
    };
    // A batch that did not write it does not read it again…
    assert!(verifier::run_scoped(&ctx, Scope::Files(&others)).unwrap().ok());
    // …but the batch that did, the drive's check and the full verifier all do.
    assert!(!verifier::run_scoped(&ctx, Scope::Files(&[damaged])).unwrap().ok());
    assert!(!verifier::run_scoped(&ctx, Scope::Drive(&drive_id)).unwrap().ok());
    assert!(!verifier::run(&ctx).unwrap().ok());

    // End to end: new photographs index in batches that pass, and the check at
    // the end of the run still finds the damage and writes it down.
    write_photo(&h.drive_dir.join("later/new_a.png"), [10, 90, 140], 40, 30);
    write_photo(&h.drive_dir.join("later/new_b.png"), [20, 90, 140], 40, 30);
    let summary = p.run(&opts).unwrap();
    assert_eq!(summary.files_done, 2);
    assert!(
        h.paths.reports_dir().join(format!("verifier-{}.json", summary.run_id)).exists(),
        "a failed end-of-run check must leave a report"
    );
}

/// Every item is handed out once and its result delivered once, whatever
/// order the threads finish in.
#[test]
fn in_parallel_delivers_every_result_exactly_once() {
    let items: Vec<u32> = (0..50).collect();
    let mut seen = vec![0u32; items.len()];
    let (unstarted, stopped) = super::in_parallel(
        &items,
        4,
        &|| false,
        &|n: &u32| {
            // Uneven work, so results arrive out of order.
            std::thread::sleep(std::time::Duration::from_millis(u64::from(n % 5)));
            n * 2
        },
        |i, r| {
            assert_eq!(r, items[i] * 2, "result delivered against the wrong item");
            seen[i] += 1;
            true
        },
    );
    assert_eq!(unstarted, items.len());
    assert!(!stopped);
    assert!(seen.iter().all(|&c| c == 1), "{seen:?}");
}

/// A stop starts nothing new, keeps what was already being worked on, and
/// says exactly which items were never touched — the ones handed back to the
/// queue unspent.
#[test]
fn in_parallel_stops_starting_work_but_keeps_work_in_flight() {
    let items: Vec<u32> = (0..40).collect();
    let started = std::sync::Mutex::new(Vec::new());
    let delivered = std::sync::atomic::AtomicUsize::new(0);
    let mut results = Vec::new();
    let (unstarted, stopped) = super::in_parallel(
        &items,
        3,
        &|| delivered.load(std::sync::atomic::Ordering::SeqCst) >= 5,
        &|n: &u32| {
            started.lock().unwrap().push(*n);
            std::thread::sleep(std::time::Duration::from_millis(20));
            *n
        },
        |i, _| {
            results.push(i);
            delivered.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            true
        },
    );
    assert!(stopped, "the stop must be reported");
    let mut started = started.into_inner().unwrap();
    started.sort();
    results.sort();
    assert_eq!(started, results.iter().map(|&i| i as u32).collect::<Vec<_>>(),
        "everything started is delivered, nothing else is");
    assert!(unstarted < items.len(), "work stopped early");
    assert_eq!(started, (0..unstarted as u32).collect::<Vec<_>>(),
        "exactly the items before `unstarted` were touched");
}

/// Returning `false` from the result handler — a hard halt — also stops new
/// work from starting.
#[test]
fn in_parallel_stops_when_the_handler_says_so() {
    let items: Vec<u32> = (0..40).collect();
    let mut handled = 0;
    let (unstarted, stopped) = super::in_parallel(
        &items,
        2,
        &|| false,
        &|n: &u32| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            *n
        },
        |_, _| {
            handled += 1;
            false
        },
    );
    assert!(!stopped, "a halt is not a stop request");
    assert!(unstarted <= 4, "at most one more per thread after the halt, got {unstarted}");
    assert_eq!(handled, unstarted);
}

/// D-089: a scan groups its drive's faces when it finishes, so there is
/// nothing left for grouping by hand to do afterwards.
#[test]
fn a_finished_scan_has_already_grouped_its_faces() {
    let (h, mut opts) = setup(no_disk_floor());
    for i in 0..12 {
        write_distinct_photo(&h.drive_dir.join(format!("many/p{i:02}.png")), i);
    }
    opts.config.batch_size = 4;
    pipeline(&h).run(&opts).unwrap();

    let faces: i64 = h.archive.query_row("SELECT count(*) FROM faces", [], |r| r.get(0)).unwrap();
    assert!(faces > 0, "the fixture must produce faces for this test to mean anything");
    let log = std::fs::read_to_string(h.paths.index_log()).unwrap();
    assert!(log.contains("\"faces_grouped\""), "the scan must report its grouping");

    let drive_id: String = h
        .archive
        .query_row("SELECT id FROM drives WHERE drive_number = 14", [], |r| r.get(0))
        .unwrap();
    let again = crate::faces::FaceRepo::new(&h.archive)
        .group_ungrouped(Some(&drive_id), &h.key)
        .unwrap();
    assert_eq!(again.groups_created, 0, "the scan should have left nothing to group");
    let grouped: i64 = h
        .archive
        .query_row("SELECT count(*) FROM faces WHERE cluster_id IS NOT NULL", [], |r| r.get(0))
        .unwrap();
    assert!(grouped > 0, "the fixture's look-alike faces were grouped by the scan");
}

/// D-090: a second scan of a drive is refused while the first is really
/// alive — and only then.
#[test]
fn a_second_scan_of_the_same_drive_is_refused_only_while_the_first_is_alive() {
    let (h, opts) = setup(no_disk_floor());
    let p = pipeline(&h);
    let drive_id: String = h
        .archive
        .query_row("SELECT id FROM drives WHERE drive_number = 14", [], |r| r.get(0))
        .unwrap();
    let other_run = |pid: u32, minutes_ago: i64| {
        let beat = (chrono::Utc::now() - chrono::Duration::minutes(minutes_ago))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        h.archive.execute("DELETE FROM scan_runs WHERE id = 'other'", []).unwrap();
        h.archive
            .execute(
                "INSERT INTO scan_runs (id, drive_id, drive_number, scan_root, mode, started_at, outcome, heartbeat_at, pid)
                 VALUES ('other', ?1, 14, '/Volumes/X', 'initial', ?2, 'running', ?2, ?3)",
                rusqlite::params![drive_id, beat, i64::from(pid)],
            )
            .unwrap();
    };

    // Another process, alive and beating: refused, with a reason.
    let mut alive = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    other_run(alive.id(), 1);
    let err = p.run(&opts).expect_err("a live scan of the same drive must block a second");
    assert!(format!("{err}").contains("already being scanned"), "{err}");
    assert!(!err.is_hard_halt(), "being busy is not a safety event");

    // The same process, silent for longer than a stall: not blocking.
    other_run(alive.id(), crate::progress::STALL_AFTER_MINUTES + 1);
    let summary = p.run(&opts).expect("a stalled scan must not block a new one");
    assert_eq!(summary.files_done, 3);
    let _ = alive.kill();
    let _ = alive.wait();

    // A process that has gone — killed a minute ago, heartbeat still fresh:
    // not blocking either. This is the case that must never lock the owner out.
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let gone_pid = gone.id();
    gone.wait().unwrap();
    other_run(gone_pid, 1);
    p.run(&opts).expect("a dead scan must not block a new one");
}
