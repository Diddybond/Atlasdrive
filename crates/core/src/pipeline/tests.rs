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
    let err = p.run(&opts).expect_err("scanning a vanished volume must fail");
    assert!(
        matches!(err, crate::error::Error::InvalidArgs(_)),
        "expected a clear 'not a directory' error, got {err:?}"
    );

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

/// A real HEIC photograph indexes end to end on macOS: thumbnail, hash and
/// embedding all produced, and the original left untouched.
#[cfg(target_os = "macos")]
#[test]
fn indexes_a_real_heic_photograph_on_macos() {
    let (h, opts) = setup(no_disk_floor());

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
