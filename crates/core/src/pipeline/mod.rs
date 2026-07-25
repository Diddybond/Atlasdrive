//! The indexing pipeline and its resumable batch loop
//! (see `docs/06_INDEXING_PIPELINE.md`).
//!
//! Responsibilities:
//!   * Preflight safety checks (migrations current, disk floor, models present,
//!     network isolation engaged).
//!   * Durable queue construction (idempotent).
//!   * Batch lease → per-file analysis → atomic commit → verify → progress.
//!   * Resume, dry-run, verify-only and rebuild-faces modes.
//!
//! The loop is idempotent: re-running never creates duplicate catalogue rows or
//! unnecessary duplicate thumbnails.

pub mod decode;
pub mod metadata;
pub mod phash;
pub mod thumbnail;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, Connection};

use crate::ai::{Capability, CancelToken, EngineRegistry};
use crate::config::{AppPaths, Config};
use crate::crypto::MasterKey;
use crate::dates::{self, DateInputs};
use crate::drive::DriveRepo;
use crate::error::{Error, Result};
use crate::faces::FaceRepo;
use crate::integrity::{self, SourceSnapshot};
use crate::logging::{Level, Logger};
use crate::net::{self, OfflineGuard};
use crate::progress::Progress;
use crate::queue::{Queue, QueueItem};
use crate::scan::{self, ScanOptions};
use crate::search::encode_vector;
use crate::util::{new_uuid, now_iso8601};

/// Indexing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexMode {
    /// Full run (or resume of one).
    Normal,
    /// Process at most 20 files, write nothing permanent.
    DryRun,
    /// Only run the verifier against the existing catalogue.
    VerifyOnly,
    /// Rebuild face clusters without reopening originals.
    RebuildFaces,
}

/// Options for an index run.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub drive_number: i64,
    pub path: PathBuf,
    pub mode: IndexMode,
    pub resume: bool,
    pub exclusions: Vec<String>,
    pub config: Config,
}

impl IndexOptions {
    pub fn new(drive_number: i64, path: impl Into<PathBuf>) -> Self {
        Self {
            drive_number,
            path: path.into(),
            mode: IndexMode::Normal,
            resume: false,
            exclusions: Vec::new(),
            config: Config::default(),
        }
    }
}

/// Summary returned from a run.
#[derive(Debug, Clone)]
pub struct IndexSummary {
    pub run_id: String,
    pub files_discovered: u64,
    pub files_done: u64,
    pub files_failed: u64,
    pub batches: u64,
    pub dry_run: bool,
    pub halted: bool,
    pub halt_reason: Option<String>,
    /// Previously-indexed files whose original changed on disk and were
    /// re-queued for analysis.
    pub files_changed: u64,
    /// Previously-indexed files no longer present under the scan root, marked
    /// `missing` in the catalogue.
    pub files_missing: u64,
}

/// Crop a detected face out of the decoded original and encode a small JPEG.
///
/// Returns `None` when the crop would be too small to recognise anyone from,
/// which is the honest outcome for a face in the far background of a crowd.
///
/// The margin matches the one used for the identity embedding, so the picture
/// the user judges is the same region the matching was based on.
fn crop_face_image(
    rgb: &image::RgbImage,
    face: &crate::ai::FaceDetection,
) -> Option<(Vec<u8>, u32, u32)> {
    const MARGIN: f32 = 0.45;
    const MIN_EDGE: u32 = 24;

    let (iw, ih) = (rgb.width() as f32, rgb.height() as f32);
    let cx = (face.x + face.w / 2.0) * iw;
    let cy = (face.y + face.h / 2.0) * ih;
    let half_w = face.w * iw * (1.0 + MARGIN) / 2.0;
    let half_h = face.h * ih * (1.0 + MARGIN) / 2.0;

    let x0 = (cx - half_w).max(0.0) as u32;
    let y0 = (cy - half_h).max(0.0) as u32;
    let x1 = (cx + half_w).min(iw) as u32;
    let y1 = (cy + half_h).min(ih) as u32;
    let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if w < MIN_EDGE || h < MIN_EDGE {
        return None;
    }

    let crop = image::imageops::crop_imm(rgb, x0, y0, w, h).to_image();
    let (tw, th) = thumbnail::fit_within(w, h, crate::faces::FACE_THUMBNAIL_EDGE);
    let small = image::imageops::resize(&crop, tw, th, image::imageops::FilterType::Lanczos3);

    // JPEG, not PNG: these are photographs of faces viewed at thumbnail size, so
    // lossless costs roughly 10x the disk for no visible benefit. At 2,000 faces
    // per wedding that is the difference between ~13MB and ~135MB.
    let mut jpeg = Vec::new();
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::Cursor::new(&mut jpeg), 82);
    encoder.encode_image(&small).ok()?;
    Some((jpeg, tw, th))
}

/// What an incremental rescan found when comparing disk against the catalogue.
#[derive(Debug, Default)]
struct Rescan {
    /// Files whose original changed since indexing, to be re-analysed.
    changed: Vec<(scan::DiscoveredFile, i64)>,
    /// Count of files marked `missing`.
    missing: u64,
}

impl Rescan {
    fn changed_count(&self) -> u64 {
        self.changed.len() as u64
    }
}

/// Everything the pipeline needs to run.
pub struct Pipeline<'a> {
    pub archive: &'a Connection,
    pub queue: &'a Connection,
    pub paths: &'a AppPaths,
    pub engines: Arc<EngineRegistry>,
    pub key: &'a MasterKey,
    pub logger: Logger,
    pub cancel: CancelToken,
}

impl<'a> Pipeline<'a> {
    /// Run an index operation according to `opts`.
    pub fn run(&self, opts: &IndexOptions) -> Result<IndexSummary> {
        match opts.mode {
            IndexMode::VerifyOnly => self.run_verify_only(opts),
            IndexMode::RebuildFaces => self.run_rebuild_faces(opts),
            IndexMode::DryRun => self.run_index(opts, true),
            IndexMode::Normal => self.run_index(opts, false),
        }
    }

    fn preflight(&self, opts: &IndexOptions) -> Result<()> {
        // Migrations current (opening already migrates; assert version > 0).
        if crate::db::schema_version(self.archive)? < 1 {
            return Err(Error::MigrationOrCorruption("archive schema not migrated".into()));
        }
        // Free-space floor.
        let free = crate::util::available_space(&self.paths.root)?;
        if free < opts.config.free_space_floor_bytes {
            return Err(Error::InsufficientDisk(format!(
                "free {} below floor {}",
                free, opts.config.free_space_floor_bytes
            )));
        }
        // Path exists and is a directory.
        if !opts.path.is_dir() {
            return Err(Error::InvalidArgs(format!(
                "scan path is not a directory: {}",
                opts.path.display()
            )));
        }
        Ok(())
    }

    fn run_index(&self, opts: &IndexOptions, dry_run: bool) -> Result<IndexSummary> {
        self.preflight(opts)?;

        // Engage the network isolation guard for the whole indexing operation.
        net::reset_blocked_attempts();
        let _guard = OfflineGuard::engage();

        let drive_repo = DriveRepo::new(self.archive);
        let drive = drive_repo
            .get_by_number(opts.drive_number)?
            .ok_or_else(|| Error::InvalidArgs(format!("drive {} not registered", opts.drive_number)))?;
        let root_id = drive_repo.ensure_root(&drive.id, "")?;
        drive_repo.set_status(&drive.id, "online")?;

        // Resume an existing run if requested and present. A run left in either
        // "running" or "interrupted" state is continued under its own id.
        let run_id = if opts.resume {
            match Progress::load(self.paths)? {
                Some(p) if p.status == "running" || p.status == "interrupted" => p.run_id,
                _ => self.start_run(&drive.id, opts, dry_run)?,
            }
        } else {
            self.start_run(&drive.id, opts, dry_run)?
        };

        let logger = self.logger.clone().with_run(&run_id, opts.drive_number);
        logger
            .info("run_start")
            .field("mode", if dry_run { "dry-run" } else { "normal" })
            .field("path", opts.path.to_string_lossy().to_string())
            .emit_best_effort();

        // Enumerate + enqueue (idempotent).
        let scan_opts = ScanOptions {
            exclusions: opts.exclusions.clone(),
            max_files: if dry_run { Some(20) } else { None },
        };
        let discovered = scan::enumerate(&opts.path, &scan_opts)?;
        let q = Queue::new(self.queue);
        let with_mtime: Vec<(scan::DiscoveredFile, i64)> = discovered
            .iter()
            .map(|f| {
                let mtime = SourceSnapshot::capture(&f.abs_path).map(|s| s.mtime_ns).unwrap_or(0);
                (f.clone(), mtime)
            })
            .collect();
        let mut rescan = Rescan::default();
        if !dry_run {
            q.enqueue(&run_id, &drive.id, opts.drive_number, &root_id, &with_mtime)?;

            // Incremental rescan: reconcile the catalogue with what is on disk
            // now. Purely a catalogue operation — originals are only stat'ed.
            rescan = self.reconcile_rescan(&drive.id, &root_id, &with_mtime)?;
            if !rescan.changed.is_empty() {
                q.requeue_changed(&drive.id, &root_id, &rescan.changed)?;
            }
            if rescan.changed_count() > 0 || rescan.missing > 0 {
                logger
                    .info("rescan_reconciled")
                    .field("changed", rescan.changed_count())
                    .field("missing", rescan.missing)
                    .emit_best_effort();
            }
        }

        let mut progress = Progress::new(&run_id, opts.drive_number, &drive.id, &opts.path.to_string_lossy());
        progress.files_discovered = discovered.len() as u64;
        if !dry_run {
            progress.write(self.paths)?;
        }

        // For dry-run we process the discovered list directly into a temp dir.
        let thumbs_dir = if dry_run {
            let tmp = self.paths.cache_dir().join(format!("dryrun-{run_id}"));
            std::fs::create_dir_all(&tmp)?;
            tmp
        } else {
            self.paths.thumbnails_dir()
        };

        let mut summary = IndexSummary {
            run_id: run_id.clone(),
            files_discovered: discovered.len() as u64,
            files_done: 0,
            files_failed: 0,
            batches: 0,
            dry_run,
            halted: false,
            halt_reason: None,
            files_changed: rescan.changed_count(),
            files_missing: rescan.missing,
        };

        let mut consecutive_verifier_failures = 0u32;
        let batch_size = opts.config.batch_size.max(1);
        let mut interrupted = false;

        loop {
            if self.cancel.is_cancelled() {
                logger.warn("cancelled").emit_best_effort();
                interrupted = true;
                break;
            }

            // Obtain the next batch of work.
            let batch: Vec<QueueItem> = if dry_run {
                // Synthesize queue items from the discovered list, once.
                if summary.batches > 0 {
                    Vec::new()
                } else {
                    discovered
                        .iter()
                        .take(20)
                        .map(|f| QueueItem {
                            id: new_uuid(),
                            run_id: run_id.clone(),
                            drive_id: drive.id.clone(),
                            drive_number: opts.drive_number,
                            root_id: root_id.clone(),
                            relative_path: f.relative_path.clone(),
                            abs_path: f.abs_path.to_string_lossy().to_string(),
                            size_bytes: f.size_bytes as i64,
                            source_mtime_ns: 0,
                            source_birthtime_ns: None,
                            inode_or_file_id: None,
                            attempts: 0,
                        })
                        .collect()
                }
            } else {
                q.claim_batch(&drive.id, batch_size, opts.config.lease_ttl_seconds, "worker-1")?
            };

            if batch.is_empty() {
                break;
            }

            summary.batches += 1;
            let batch_no = summary.batches;
            let batch_started = std::time::Instant::now();
            let mut batch_success = 0u64;
            let mut batch_failure = 0u64;

            for item in &batch {
                match self.process_file(opts, &drive, item, &opts.path, &thumbs_dir, dry_run) {
                    Ok(rel) => {
                        batch_success += 1;
                        summary.files_done += 1;
                        progress.last_completed_file = Some(rel);
                        if !dry_run {
                            q.complete(&item.id)?;
                        }
                    }
                    Err(e) if e.is_hard_halt() => {
                        // Immediate hard halt (integrity, unsafe path, network...).
                        logger
                            .error("hard_halt")
                            .relative_path(item.relative_path.clone())
                            .code(format!("{}", e.exit_code()))
                            .field("error", format!("{e}"))
                            .emit_best_effort();
                        progress.status = "halted".into();
                        progress.touch();
                        if !dry_run {
                            progress.write(self.paths)?;
                        }
                        summary.halted = true;
                        summary.halt_reason = Some(format!("{e}"));
                        self.finish_run(&run_id, "halted", &summary)?;
                        return Err(e);
                    }
                    Err(e) => {
                        // Recoverable file-level failure: record and requeue.
                        batch_failure += 1;
                        summary.files_failed += 1;
                        let retryable = item.attempts < 3;
                        logger
                            .warn("file_failed")
                            .relative_path(item.relative_path.clone())
                            .field("error", format!("{e}"))
                            .field("retryable", retryable)
                            .emit_best_effort();
                        if !dry_run {
                            q.fail(&item.id, "PROCESS", &format!("{e}"), retryable)?;
                        }
                    }
                }
            }

            let elapsed = batch_started.elapsed().as_secs_f64().max(1e-6);
            let throughput = batch.len() as f64 / elapsed;

            // Per-batch verification.
            if !dry_run {
                let report = self.verify_batch(opts, throughput)?;
                if report.has_halt() {
                    logger
                        .error("verifier_halt")
                        .field("summary", report.summary())
                        .emit_best_effort();
                    progress.status = "halted".into();
                    progress.write(self.paths)?;
                    summary.halted = true;
                    summary.halt_reason = Some(report.summary());
                    self.write_report(&run_id, &report)?;
                    self.finish_run(&run_id, "halted", &summary)?;
                    return Err(Error::VerifierFailure(report.summary()));
                }
                if !report.ok() {
                    consecutive_verifier_failures += 1;
                    progress.consecutive_verifier_failures = consecutive_verifier_failures;
                    logger
                        .warn("verifier_failure")
                        .batch(batch_no)
                        .field("consecutive", consecutive_verifier_failures)
                        .emit_best_effort();
                    if consecutive_verifier_failures >= opts.config.max_consecutive_verifier_failures {
                        self.write_report(&run_id, &report)?;
                        progress.status = "halted".into();
                        progress.write(self.paths)?;
                        summary.halted = true;
                        summary.halt_reason = Some("repeated verifier failure".into());
                        self.finish_run(&run_id, "halted", &summary)?;
                        return Err(Error::RepeatedVerifierFailure(report.summary()));
                    }
                } else {
                    consecutive_verifier_failures = 0;
                    progress.consecutive_verifier_failures = 0;
                }
            }

            // Persist progress + append a batch log line.
            let stats = if dry_run {
                Default::default()
            } else {
                q.stats(&drive.id)?
            };
            progress.files_done = summary.files_done;
            progress.files_failed = summary.files_failed;
            progress.files_queued = stats.queued as u64;
            progress.current_batch = batch_no;
            progress.touch();
            if !dry_run {
                progress.write(self.paths)?;
            }
            self.record_batch(&run_id, batch_no, batch.len(), batch_success, batch_failure, throughput)?;
            logger
                .event(Level::Info, "batch_complete")
                .batch(batch_no)
                .field("files", batch.len() as i64)
                .field("success", batch_success as i64)
                .field("failed", batch_failure as i64)
                .field("throughput_fps", throughput)
                .emit_best_effort();

            if dry_run {
                break;
            }
        }

        // Finalize.
        if interrupted {
            // Leave the run resumable: record interrupted state, do not complete.
            progress.status = "interrupted".into();
            progress.touch();
            if !dry_run {
                progress.write(self.paths)?;
            }
            self.finish_run(&run_id, "interrupted", &summary)?;
        } else if !summary.halted {
            progress.status = "complete".into();
            progress.touch();
            if !dry_run {
                progress.write(self.paths)?;
                drive_repo.audit(&drive.id, "scan_complete", None)?;
                self.archive.execute(
                    "UPDATE drives SET last_scan_at=?2 WHERE id=?1",
                    params![drive.id, now_iso8601()],
                )?;
            }
            self.finish_run(&run_id, "success", &summary)?;
        }

        // Clean up dry-run temp data.
        if dry_run {
            let _ = std::fs::remove_dir_all(&thumbs_dir);
            logger
                .info("dry_run_complete")
                .field("would_process", summary.files_done as i64)
                .emit_best_effort();
        }

        // Confirm the guard blocked nothing.
        if net::blocked_attempts() > 0 {
            return Err(Error::NetworkIsolation(format!(
                "{} network attempts during indexing",
                net::blocked_attempts()
            )));
        }

        Ok(summary)
    }

    /// Process one file: full read-only analysis and atomic commit.
    /// Returns the relative path on success.
    fn process_file(
        &self,
        opts: &IndexOptions,
        drive: &crate::drive::Drive,
        item: &QueueItem,
        root: &Path,
        thumbs_dir: &Path,
        dry_run: bool,
    ) -> Result<String> {
        let abs = PathBuf::from(&item.abs_path);
        // Containment: the queued path must still be inside the approved root.
        let abs = scan::ensure_contained(root, &abs)?;

        // 1. Pre-processing integrity snapshot.
        let snap = SourceSnapshot::capture(&abs)?;

        // 2. Decode read-only. Unsupported/broken decode is a recoverable error.
        //    HEIC/HEIF go through the macOS system decoder (see `decode`).
        let _ro = integrity::open_readonly(&abs)?; // prove read-only open works
        let rgb = decode::open_rgb(&abs, &self.paths.cache_dir().join("decode"))?;
        let (w, h) = (rgb.width(), rgb.height());

        // 3. Content + perceptual hashes.
        let content_hash = integrity::content_hash(&abs)?;
        let phash = phash::dhash(&rgb);

        // 4. Metadata.
        let md = metadata::extract(&abs, Some((w, h)));

        // 5. AI analysis (all local, offline).
        //
        // Colour and scan-artefact analysis are cheap pixel statistics and always
        // come from the heuristic engine. Everything that needs a model — the
        // embedding, what the photograph shows, its text and its faces — comes
        // from a single-pass analyser when one is registered (Apple Vision), and
        // from the heuristic engine otherwise.
        let cancel = &self.cancel;
        let color = self
            .engines
            .engine_for(Capability::Color)
            .color(&rgb, cancel)?;
        let scan_art = self
            .engines
            .engine_for(Capability::ScanArtifact)
            .scan_artifact(&rgb, cancel)?;

        let analyser = self.engines.file_analyser();
        // A real model failing on one photograph must not fail the run; fall
        // back to the heuristic engine for that file and carry on.
        let analysis = match &analyser {
            Some(engine) => match engine.analyse_file(&abs, cancel) {
                Ok(a) => Some(a),
                Err(e) => {
                    self.logger
                        .warn("file_analysis_fallback")
                        .field("path", item.relative_path.clone())
                        .field("error", format!("{e}"))
                        .emit_best_effort();
                    None
                }
            },
            None => None,
        };

        let (embedding, faces, scene, ocr_text) = match analysis {
            Some(a) => {
                let meta = a.meta.clone();
                let value = a.value;
                let embedding = match value.embedding {
                    Some(e) => crate::ai::Provenanced::new(e, meta.clone()),
                    // An analyser that recognised the image but produced no
                    // vector still leaves search working via the other engine.
                    None => self
                        .engines
                        .engine_for(Capability::VisualEmbedding)
                        .visual_embedding(&rgb, cancel)?,
                };
                let scene = match value.scene {
                    Some(s) => crate::ai::Provenanced::new(s, meta.clone()),
                    None => self.engines.engine_for(Capability::Scene).scene(&rgb, cancel)?,
                };
                let faces = crate::ai::Provenanced::new(value.faces, meta);
                (embedding, faces, scene, value.ocr.map(|o| o.text))
            }
            None => {
                let embedding = self
                    .engines
                    .engine_for(Capability::VisualEmbedding)
                    .visual_embedding(&rgb, cancel)?;
                let scene = self.engines.engine_for(Capability::Scene).scene(&rgb, cancel)?;
                let faces = self
                    .engines
                    .engine_for(Capability::FaceDetection)
                    .detect_faces(&rgb, cancel)?;
                (embedding, faces, scene, None)
            }
        };
        let face_engine = self.engines.engine_for(Capability::FaceEmbedding);

        // 6. Date estimate.
        let filename_year = dates::year_from_text(&item.relative_path);
        let date_est = dates::estimate(&DateInputs {
            exif_capture: md.exif_capture_date.clone(),
            exif_digitized: md.exif_digitized_date.clone(),
            fs_mtime_date: None,
            filename_year,
            likely_scanned_print: scan_art.value.likely_scanned_print,
            is_grayscale: color.value.is_grayscale,
        });

        // 7. Thumbnail (generate + verify decode).
        let file_id = self.file_id_for(&drive.id, &item.root_id, &item.relative_path);
        let thumb = thumbnail::generate(&rgb, thumbs_dir, &file_id, opts.config.thumbnail_max_edge)?;
        if !thumb.decode_ok {
            return Err(Error::Other("thumbnail failed to decode".into()));
        }

        // 8. Re-stat the original and assert it is unchanged. HARD SAFETY GATE.
        snap.assert_unchanged(&abs)?;

        if dry_run {
            // Report the proposed record; write nothing to the catalogue.
            println!(
                "[dry-run] {} | {}x{} | phash={} | faces={} | {} | date={}",
                item.relative_path,
                w,
                h,
                phash,
                faces.value.len(),
                scene.value.description,
                dates::describe(&date_est)
            );
            return Ok(item.relative_path.clone());
        }

        // 9. Atomic commit to archive.db.
        let tx = self.archive.unchecked_transaction()?;
        self.commit_file(
            &tx, &file_id, drive, item, &snap, &content_hash, &phash, &md, &color.value,
            &scene.value, &scan_art.value, &embedding, &date_est, &thumb, &faces.value,
            &(faces.meta.model_id.clone(), faces.meta.model_version.clone()),
            ocr_text.as_deref(), &rgb,
            &face_engine, cancel,
        )?;
        tx.commit()?;

        Ok(item.relative_path.clone())
    }

    /// Deterministic file id so re-running is idempotent (no duplicate rows).
    fn file_id_for(&self, drive_id: &str, root_id: &str, rel: &str) -> String {
        let mut h = blake3::Hasher::new();
        h.update(drive_id.as_bytes());
        h.update(b"\0");
        h.update(root_id.as_bytes());
        h.update(b"\0");
        h.update(rel.as_bytes());
        // Format as a uuid-like hex so thumbnail sharding works.
        h.finalize().to_hex().to_string()[..32].to_string()
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_file(
        &self,
        tx: &Connection,
        file_id: &str,
        drive: &crate::drive::Drive,
        item: &QueueItem,
        snap: &SourceSnapshot,
        content_hash: &str,
        phash: &str,
        md: &metadata::ImageMetadata,
        color: &crate::ai::ColorResult,
        scene: &crate::ai::SceneResult,
        scan_art: &crate::ai::ScanArtifactResult,
        embedding: &crate::ai::Provenanced<crate::ai::Embedding>,
        date_est: &dates::DateEstimate,
        thumb: &thumbnail::ThumbnailInfo,
        faces: &[crate::ai::FaceDetection],
        // (model_id, model_version) of the analyser that produced the faces.
        analysis_model: &(String, String),
        ocr_text: Option<&str>,
        rgb: &image::RgbImage,
        face_engine: &Arc<dyn crate::ai::AiEngine>,
        cancel: &CancelToken,
    ) -> Result<()> {
        let now = now_iso8601();
        let filename = Path::new(&item.relative_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| item.relative_path.clone());
        let ext = Path::new(&item.relative_path)
            .extension()
            .map(|s| s.to_string_lossy().to_ascii_lowercase());

        // files (idempotent upsert on the unique (drive,root,rel) key).
        tx.execute(
            "INSERT INTO files
               (id, drive_id, root_id, relative_path, filename, extension, size_bytes,
                source_mtime_ns, source_birthtime_ns, inode_or_file_id, content_hash,
                perceptual_hash, status, analysis_version, last_verified_at, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'complete',1,?13,?13,?13)
             ON CONFLICT(drive_id, root_id, relative_path) DO UPDATE SET
                content_hash=excluded.content_hash,
                perceptual_hash=excluded.perceptual_hash,
                -- Refresh the recorded source stat. `snap` is the post-processing
                -- snapshot that `assert_unchanged` just validated, so this is what
                -- we genuinely last observed. Leaving these stale would strand a
                -- re-analysed file permanently mismatched against its own original
                -- and trip the integrity verifier on every later run.
                size_bytes=excluded.size_bytes,
                source_mtime_ns=excluded.source_mtime_ns,
                source_birthtime_ns=excluded.source_birthtime_ns,
                inode_or_file_id=excluded.inode_or_file_id,
                status='complete', analysis_version=1, updated_at=excluded.updated_at,
                last_verified_at=excluded.last_verified_at",
            params![
                file_id, drive.id, item.root_id, item.relative_path, filename, ext,
                snap.size_bytes as i64, snap.mtime_ns, snap.birthtime_ns,
                snap.inode_or_file_id.map(|v| v as i64), content_hash, phash, now,
            ],
        )?;

        // thumbnails
        tx.execute(
            "INSERT INTO thumbnails (file_id, rel_path, width, height, format, checksum, decode_ok, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(file_id) DO UPDATE SET
                rel_path=excluded.rel_path, width=excluded.width, height=excluded.height,
                format=excluded.format, checksum=excluded.checksum, decode_ok=excluded.decode_ok",
            params![
                file_id, thumb.rel_path, thumb.width, thumb.height, thumb.format,
                thumb.checksum, thumb.decode_ok as i64, now
            ],
        )?;

        // metadata
        tx.execute(
            "INSERT INTO metadata
               (file_id, width, height, orientation, camera_make, camera_model, lens,
                exif_capture_date, exif_digitized_date, color_profile, raw_json, normalized_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(file_id) DO UPDATE SET
                width=excluded.width, height=excluded.height, orientation=excluded.orientation,
                camera_make=excluded.camera_make, camera_model=excluded.camera_model,
                exif_capture_date=excluded.exif_capture_date",
            params![
                file_id, md.width, md.height, md.orientation, md.camera_make, md.camera_model,
                md.lens, md.exif_capture_date, md.exif_digitized_date, md.color_profile,
                serde_json::to_string(&md.raw)?, Option::<String>::None,
            ],
        )?;

        // scene_analysis
        tx.execute(
            "INSERT INTO scene_analysis
               (file_id, indoor_prob, outdoor_prob, people_count, description, concepts_json,
                ocr_text, ocr_confidence, color_summary_json, likely_scanned_print,
                likely_photo_of_photo, border_fade_json, model_id, model_version, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
             ON CONFLICT(file_id) DO UPDATE SET
                description=excluded.description, concepts_json=excluded.concepts_json,
                ocr_text=excluded.ocr_text, ocr_confidence=excluded.ocr_confidence,
                likely_scanned_print=excluded.likely_scanned_print",
            params![
                file_id, scene.indoor_prob, scene.outdoor_prob, scene.people_count,
                scene.description, serde_json::to_string(&scene.concepts)?,
                ocr_text, if ocr_text.is_some() { 1.0 } else { 0.0 },
                serde_json::to_string(color)?, scan_art.likely_scanned_print as i64,
                scan_art.likely_photo_of_photo as i64,
                serde_json::to_string(scan_art)?,
                embedding.meta.model_id, embedding.meta.model_version, now,
            ],
        )?;

        // visual_embeddings (model-version partitioned)
        tx.execute(
            "INSERT INTO visual_embeddings (file_id, model_id, model_version, dim, vector, created_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(file_id, model_id, model_version) DO UPDATE SET vector=excluded.vector",
            params![
                file_id, embedding.meta.model_id, embedding.meta.model_version,
                embedding.value.dim as i64, encode_vector(&embedding.value.vector), now
            ],
        )?;

        // date_estimates
        tx.execute(
            "INSERT INTO date_estimates
               (file_id, earliest_date, latest_date, confidence, method_version, evidence_json,
                is_user_confirmed, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)
             ON CONFLICT(file_id) DO UPDATE SET
                earliest_date=excluded.earliest_date, latest_date=excluded.latest_date,
                confidence=excluded.confidence, method_version=excluded.method_version,
                evidence_json=excluded.evidence_json, updated_at=excluded.updated_at
             -- A date the user corrected outranks anything a model infers, and
             -- re-analysis must never silently take it back (docs/07: user
             -- confirmations are never removed by model reprocessing).
             WHERE date_estimates.is_user_confirmed = 0",
            params![
                file_id, date_est.earliest_date, date_est.latest_date, date_est.confidence,
                date_est.method_version, serde_json::to_string(&date_est.evidence)?,
                date_est.is_user_confirmed as i64, now,
            ],
        )?;

        // faces + encrypted embeddings. Clear prior faces for idempotency.
        tx.execute("DELETE FROM faces WHERE file_id=?1", [file_id])?;
        let face_repo = FaceRepo::new(tx);
        for f in faces {
            // Prefer the identity embedding the analyser produced from the
            // full-resolution original; only fall back to re-embedding the
            // decoded copy when it did not provide one.
            let (vector, model_id, model_version) = match &f.embedding {
                Some(v) => (v.clone(), analysis_model.0.clone(), analysis_model.1.clone()),
                None => {
                    let fe = face_engine.face_embedding(rgb, f, cancel)?;
                    (fe.value.vector, fe.meta.model_id, fe.meta.model_version)
                }
            };
            let face_id = face_repo.insert_face(
                file_id,
                (f.x, f.y, f.w, f.h),
                f.quality,
                &model_id,
                &model_version,
                &vector,
                self.key,
            )?;

            // A small crop of the face, kept locally so the gallery is browsable
            // with every drive unplugged. Encrypted, like the embedding.
            if let Some((png, w, h)) = crop_face_image(rgb, f) {
                face_repo.store_thumbnail(&face_id, &png, w, h, self.key)?;
            }

            // Recognise people the user has already named. This is only ever a
            // suggestion — naming stays a human decision (D-007).
            if let Some(hit) = face_repo.suggest_person(
                &vector,
                &model_id,
                &model_version,
                self.key,
                crate::faces::PERSON_MATCH_THRESHOLD,
            )? {
                face_repo.suggest_face_is_person(&face_id, &hit.person_id, hit.score)?;
            }
        }

        // Automatic concept tags with provenance.
        for concept in &scene.concepts {
            let tag_id = self.upsert_tag(tx, &concept.tag, "automatic")?;
            tx.execute(
                "INSERT OR IGNORE INTO file_tags (file_id, tag_id, confidence, source, created_at)
                 VALUES (?1,?2,?3,'automatic',?4)",
                params![file_id, tag_id, concept.confidence, now],
            )?;
        }
        if scan_art.likely_scanned_print {
            let tag_id = self.upsert_tag(tx, "likely-scan", "system")?;
            tx.execute(
                "INSERT OR IGNORE INTO file_tags (file_id, tag_id, confidence, source, created_at)
                 VALUES (?1,?2,?3,'system',?4)",
                params![file_id, tag_id, 0.6, now],
            )?;
        }

        // FTS index row (rebuild for this file).
        let tag_text: String = scene.concepts.iter().map(|c| c.tag.clone()).collect::<Vec<_>>().join(" ");
        tx.execute("DELETE FROM files_fts WHERE file_id=?1", [file_id])?;
        tx.execute(
            "INSERT INTO files_fts (file_id, filename, relative_path, tags, ocr_text, description)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                file_id, filename, item.relative_path, tag_text,
                ocr_text.unwrap_or(""), scene.description
            ],
        )?;

        Ok(())
    }

    fn upsert_tag(&self, tx: &Connection, name: &str, tag_type: &str) -> Result<String> {
        if let Ok(id) = tx.query_row(
            "SELECT id FROM tags WHERE name=?1 AND tag_type=?2",
            params![name, tag_type],
            |r| r.get::<_, String>(0),
        ) {
            return Ok(id);
        }
        let id = new_uuid();
        tx.execute(
            "INSERT INTO tags (id, name, tag_type, created_at) VALUES (?1,?2,?3,?4)",
            params![id, name, tag_type, now_iso8601()],
        )?;
        Ok(id)
    }

    fn verify_batch(&self, opts: &IndexOptions, throughput: f64) -> Result<crate::verifier::VerifierReport> {
        let ctx = crate::verifier::VerifyContext {
            archive: self.archive,
            queue: Some(self.queue),
            paths: self.paths,
            config: &opts.config,
            key: Some(self.key),
            face_model: (
                crate::ai::local::MODEL_ID.to_string(),
                crate::ai::local::MODEL_VERSION.to_string(),
            ),
            observed_throughput: Some(throughput),
            network_blocked_attempts: net::blocked_attempts(),
        };
        crate::verifier::run(&ctx)
    }

    fn run_verify_only(&self, opts: &IndexOptions) -> Result<IndexSummary> {
        let report = self.verify_batch(opts, f64::NAN)?;
        self.write_report("verify-only", &report)?;
        if !report.ok() {
            return Err(Error::VerifierFailure(report.summary()));
        }
        Ok(IndexSummary {
            run_id: "verify-only".into(),
            files_discovered: 0,
            files_done: 0,
            files_failed: 0,
            batches: 0,
            dry_run: false,
            halted: false,
            halt_reason: None,
            files_changed: 0,
            files_missing: 0,
        })
    }

    fn run_rebuild_faces(&self, _opts: &IndexOptions) -> Result<IndexSummary> {
        let repo = FaceRepo::new(self.archive);
        let clusters = repo.rebuild_clusters(
            crate::ai::local::MODEL_ID,
            crate::ai::local::MODEL_VERSION,
            self.key,
            crate::faces::DEFAULT_CLUSTER_THRESHOLD,
        )?;
        self.logger
            .info("rebuild_faces_complete")
            .field("clusters", clusters as i64)
            .emit_best_effort();
        Ok(IndexSummary {
            run_id: "rebuild-faces".into(),
            files_discovered: 0,
            files_done: clusters as u64,
            files_failed: 0,
            batches: 0,
            dry_run: false,
            files_changed: 0,
            files_missing: 0,
            halted: false,
            halt_reason: None,
        })
    }

    /// Generate the missing face crops for an already-indexed archive.
    ///
    /// Reads the originals, so the drive must be connected. Faces whose drive is
    /// unplugged are skipped and counted rather than failing the run — the user
    /// can connect the next drive and run it again.
    pub fn backfill_face_thumbnails(&self, limit: usize) -> Result<(u64, u64)> {
        let repo = FaceRepo::new(self.archive);
        let pending = repo.faces_without_thumbnails(limit)?;
        let (mut done, mut skipped) = (0u64, 0u64);

        // Group by file so each original is decoded once, however many faces it
        // holds — decoding a 9MB photograph per face would be absurd.
        let mut by_file: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for (face_id, file_id) in pending {
            by_file.entry(file_id).or_default().push(face_id);
        }

        for (file_id, face_ids) in by_file {
            let Some(abs) = crate::search::resolve_original(self.archive, &file_id)? else {
                skipped += face_ids.len() as u64;
                continue;
            };
            let rgb = match decode::open_rgb(&abs, &self.paths.cache_dir().join("decode")) {
                Ok(v) => v,
                Err(_) => {
                    skipped += face_ids.len() as u64;
                    continue;
                }
            };
            for face_id in face_ids {
                let Some((x, y, w, h)) = repo.bbox(&face_id)? else {
                    skipped += 1;
                    continue;
                };
                let face = crate::ai::FaceDetection { x, y, w, h, quality: 0.0, embedding: None };
                match crop_face_image(&rgb, &face) {
                    Some((png, tw, th)) => {
                        repo.store_thumbnail(&face_id, &png, tw, th, self.key)?;
                        done += 1;
                    }
                    None => skipped += 1,
                }
            }
        }
        Ok((done, skipped))
    }

    /// Reconcile the catalogue against what the scan just found.
    ///
    /// Two independent facts change between scans: a file's bytes can change,
    /// and a file can go away. Both are recorded against the catalogue only —
    /// nothing on the drive is opened, written or removed here, just `stat`ed.
    ///
    /// A file is "changed" when its recorded size or modification time no
    /// longer matches the original. Note this is the *inverse* use of the same
    /// comparison the integrity gate makes: during a run, a mismatch means we
    /// corrupted something and must halt; between runs, a mismatch means the
    /// user edited or replaced the photograph and we should re-analyse it.
    fn reconcile_rescan(
        &self,
        drive_id: &str,
        root_id: &str,
        discovered: &[(scan::DiscoveredFile, i64)],
    ) -> Result<Rescan> {
        use std::collections::HashMap;

        // What the catalogue currently believes about this root.
        let mut known: HashMap<String, (i64, i64, String)> = HashMap::new();
        {
            let mut stmt = self.archive.prepare(
                "SELECT relative_path, size_bytes, source_mtime_ns, status
                   FROM files WHERE drive_id = ?1 AND root_id = ?2",
            )?;
            let rows = stmt.query_map(params![drive_id, root_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (rel, size, mtime, status) = row?;
                known.insert(rel, (size, mtime, status));
            }
        }

        let mut out = Rescan::default();
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

        for (file, mtime) in discovered {
            seen.insert(file.relative_path.as_str());
            let Some((known_size, known_mtime, status)) = known.get(&file.relative_path) else {
                continue; // brand new: ordinary enqueue already covers it
            };
            // Only files we finished are candidates for re-analysis; queued or
            // failed ones are already going to be processed.
            if status != "complete" && status != "missing" {
                continue;
            }
            let changed = *known_size != file.size_bytes as i64 || *known_mtime != *mtime;
            if changed || status == "missing" {
                // 'changed' here is a catalogue state, not a safety alarm: the
                // file is re-analysed and returns to 'complete'.
                self.archive.execute(
                    "UPDATE files SET status='changed', updated_at=?3
                      WHERE drive_id=?1 AND relative_path=?2",
                    params![drive_id, file.relative_path, now_iso8601()],
                )?;
                out.changed.push((file.clone(), *mtime));
            }
        }

        // Anything the catalogue knows that the scan did not find is gone.
        for (rel, (_, _, status)) in &known {
            if seen.contains(rel.as_str()) || status == "missing" {
                continue;
            }
            self.archive.execute(
                "UPDATE files SET status='missing', updated_at=?3
                  WHERE drive_id=?1 AND relative_path=?2",
                params![drive_id, rel, now_iso8601()],
            )?;
            out.missing += 1;
        }

        Ok(out)
    }

    fn start_run(&self, drive_id: &str, opts: &IndexOptions, dry_run: bool) -> Result<String> {
        let run_id = new_uuid();
        let mode = if dry_run { "dry-run" } else { "initial" };
        self.archive.execute(
            "INSERT INTO scan_runs (id, drive_id, drive_number, scan_root, mode, started_at, outcome)
             VALUES (?1,?2,?3,?4,?5,?6,'running')",
            params![run_id, drive_id, opts.drive_number, opts.path.to_string_lossy(), mode, now_iso8601()],
        )?;
        Ok(run_id)
    }

    fn finish_run(&self, run_id: &str, outcome: &str, summary: &IndexSummary) -> Result<()> {
        // dry-run and synthetic run ids may not have a row; ignore missing.
        let _ = self.archive.execute(
            "UPDATE scan_runs SET ended_at=?2, outcome=?3, files_discovered=?4, files_done=?5, files_failed=?6
             WHERE id=?1",
            params![
                run_id, now_iso8601(), outcome, summary.files_discovered as i64,
                summary.files_done as i64, summary.files_failed as i64
            ],
        );
        Ok(())
    }

    fn record_batch(
        &self,
        run_id: &str,
        batch_no: u64,
        file_count: usize,
        success: u64,
        failure: u64,
        throughput: f64,
    ) -> Result<()> {
        let _ = self.archive.execute(
            "INSERT INTO scan_batches
               (id, run_id, batch_number, started_at, ended_at, file_count, success_count,
                failure_count, throughput_fps)
             VALUES (?1,?2,?3,?4,?4,?5,?6,?7,?8)",
            params![
                new_uuid(), run_id, batch_no as i64, now_iso8601(), file_count as i64,
                success as i64, failure as i64, throughput
            ],
        );
        Ok(())
    }

    fn write_report(&self, run_id: &str, report: &crate::verifier::VerifierReport) -> Result<()> {
        let dir = self.paths.reports_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("verifier-{run_id}.json"));
        let bytes = serde_json::to_vec_pretty(report)?;
        crate::util::atomic_write(&path, &bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
