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
        if !dry_run {
            q.enqueue(&run_id, &drive.id, opts.drive_number, &root_id, &with_mtime)?;
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
        let _ro = integrity::open_readonly(&abs)?; // prove read-only open works
        let decoded = image::open(&abs)
            .map_err(|e| Error::Other(format!("decode failed: {e}")))?;
        let rgb = decoded.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());

        // 3. Content + perceptual hashes.
        let content_hash = integrity::content_hash(&abs)?;
        let phash = phash::dhash(&rgb);

        // 4. Metadata.
        let md = metadata::extract(&abs, Some((w, h)));

        // 5. AI analysis (all local, offline).
        let cancel = &self.cancel;
        let color = self
            .engines
            .engine_for(Capability::Color)
            .color(&rgb, cancel)?;
        let scene = self
            .engines
            .engine_for(Capability::Scene)
            .scene(&rgb, cancel)?;
        let scan_art = self
            .engines
            .engine_for(Capability::ScanArtifact)
            .scan_artifact(&rgb, cancel)?;
        let embed_engine = self.engines.engine_for(Capability::VisualEmbedding);
        let embedding = embed_engine.visual_embedding(&rgb, cancel)?;
        let face_engine = self.engines.engine_for(Capability::FaceDetection);
        let faces = face_engine.detect_faces(&rgb, cancel)?;

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
            &scene.value, &scan_art.value, &embedding, &date_est, &thumb, &faces.value, &rgb,
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
                likely_scanned_print=excluded.likely_scanned_print",
            params![
                file_id, scene.indoor_prob, scene.outdoor_prob, scene.people_count,
                scene.description, serde_json::to_string(&scene.concepts)?,
                Option::<String>::None, 0.0,
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
                evidence_json=excluded.evidence_json, updated_at=excluded.updated_at",
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
            let fe = face_engine.face_embedding(rgb, f, cancel)?;
            face_repo.insert_face(
                file_id,
                (f.x, f.y, f.w, f.h),
                f.quality,
                &fe.meta.model_id,
                &fe.meta.model_version,
                &fe.value.vector,
                self.key,
            )?;
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
            params![file_id, filename, item.relative_path, tag_text, "", scene.description],
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
            halted: false,
            halt_reason: None,
        })
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
