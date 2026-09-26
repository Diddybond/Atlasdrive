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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};

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
    /// Extra file types to index for this run only (RAW, HEIC, …). Empty by
    /// default: the scan takes JPEG, PNG, TIFF and PSD unless told otherwise.
    pub extra_extensions: Vec<String>,
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
            extra_extensions: Vec::new(),
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

/// Write the run's counters to `progress.json`, and stamp the time.
///
/// Called after every photograph, whether it was catalogued or failed. Two
/// things depend on that:
///
/// * **The display.** A batch is 64 files, which on a slow drive is five
///   minutes — long enough that a progress display sits perfectly still and the
///   whole run looks stuck.
/// * **The heartbeat.** `updated_at` is how anyone else decides whether the
///   scan is alive (see [`crate::progress::STALL_AFTER_MINUTES`]). Publishing
///   only on success means a stretch of slow failures reads as a stall.
///
/// The write is a few hundred bytes against seconds of analysis per file, so
/// the cost is nothing next to being able to see the run working. Failure to
/// write is ignored on purpose: losing a progress update must never fail a
/// photograph that was catalogued correctly.
fn publish(
    progress: &mut crate::progress::Progress,
    summary: &IndexSummary,
    batch_no: u64,
    paths: &AppPaths,
    dry_run: bool,
) {
    progress.files_done = summary.files_done;
    progress.files_failed = summary.files_failed;
    progress.current_batch = batch_no;
    progress.touch();
    if !dry_run {
        let _ = progress.write(paths);
    }
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

/// Deterministic file id so re-running is idempotent (no duplicate rows).
fn file_id_for(drive_id: &str, root_id: &str, rel: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(drive_id.as_bytes());
    h.update(b"\0");
    h.update(root_id.as_bytes());
    h.update(b"\0");
    h.update(rel.as_bytes());
    // Format as a uuid-like hex so thumbnail sharding works.
    h.finalize().to_hex().to_string()[..32].to_string()
}

/// A face found in a photograph, with everything the catalogue stores about it
/// already worked out.
struct PreparedFace {
    detection: crate::ai::FaceDetection,
    /// The identity embedding, and the model partition it belongs to.
    vector: Vec<f32>,
    model_id: String,
    model_version: String,
    /// A small JPEG of the face and its size, when it is big enough to show.
    crop: Option<(Vec<u8>, u32, u32)>,
}

/// Everything about one photograph that can be worked out without the
/// catalogue, ready to be written in one transaction.
struct Analysed {
    file_id: String,
    snap: SourceSnapshot,
    content_hash: String,
    phash: String,
    md: metadata::ImageMetadata,
    color: crate::ai::ColorResult,
    scene: crate::ai::SceneResult,
    scan_art: crate::ai::ScanArtifactResult,
    embedding: crate::ai::Provenanced<crate::ai::Embedding>,
    date_est: dates::DateEstimate,
    thumb: thumbnail::ThumbnailInfo,
    faces: Vec<PreparedFace>,
    ocr_text: Option<String>,
    width: u32,
    height: u32,
}

/// What reading a photograph concluded.
enum Prepared {
    /// Its bytes match a photograph this drive's catalogue has lost track of:
    /// the same picture, moved (D-073). Nothing was decoded, because the
    /// catalogue already knows everything about it.
    Moved { abs: PathBuf, snap: SourceSnapshot, content_hash: String },
    Analysed(Box<Analysed>),
}

/// The read-only half of indexing a photograph, shared by every analysis thread.
///
/// Reading, decoding and analysing a photograph is nearly all of the time it
/// takes to index, and none of it needs the catalogue — so this holds nothing
/// that touches it. The catalogue connection stays on the pipeline's thread: a
/// rusqlite `Connection` cannot be shared between threads, and one writer loses
/// nothing when each write is a few milliseconds against seconds of analysis
/// (the same division as D-044).
struct Analyst<'r> {
    engines: &'r EngineRegistry,
    paths: &'r AppPaths,
    logger: &'r Logger,
    cancel: &'r CancelToken,
    root: &'r Path,
    thumbs_dir: &'r Path,
    thumbnail_max_edge: u32,
    drive_id: &'r str,
    /// Content hashes of this drive's `missing` rows, read once per run. Rows
    /// only become `missing` while a run reconciles, before any photograph is
    /// read, so the set cannot go stale in a way that loses a move.
    moved_candidates: &'r HashSet<String>,
    dry_run: bool,
}

impl Analyst<'_> {
    /// Read one photograph and work out everything the catalogue will hold
    /// about it. Never writes to the catalogue; never writes to the drive.
    fn prepare(&self, item: &QueueItem) -> Result<Prepared> {
        let abs = PathBuf::from(&item.abs_path);
        // Containment: the queued path must still be inside the approved root.
        let abs = scan::ensure_contained(self.root, &abs)?;

        // 1. Pre-processing integrity snapshot.
        let snap = SourceSnapshot::capture(&abs)?;

        // 2. Content hash — before decoding, because it can settle whether this
        //    file needs analysing at all.
        let content_hash = integrity::content_hash(&abs)?;

        // A file that moved is the same photograph.
        //
        // Re-scanning a drive from a different root records every path afresh,
        // so a photograph first catalogued as `edits/x.jpg` reappears as
        // `Aimee and Kent/edits/x.jpg`: the old row goes `missing` and the new
        // path arrives as a stranger. Without this, the same picture would be
        // decoded and analysed again, and — worse — its faces would exist
        // twice, once on a row Reveal-in-Finder can never find. Names the
        // owner had confirmed stay attached to the old faces, so the doubles
        // would even disagree about who is in them. 758 photographs on a real
        // drive sat in exactly this state.
        //
        // Content hash is identity here, as it already is for bit-rot
        // detection and drive comparison. Adoption re-points the existing row
        // at the new path; everything keyed by file id — faces, names, tags,
        // embeddings, thumbnails, dates — simply remains true.
        if !self.dry_run && self.moved_candidates.contains(&content_hash) {
            return Ok(Prepared::Moved { abs, snap, content_hash });
        }

        self.analyse(item, &abs, snap, content_hash)
            .map(|a| Prepared::Analysed(Box::new(a)))
    }

    /// Decode and analyse a photograph whose snapshot and hash are taken.
    fn analyse(
        &self,
        item: &QueueItem,
        abs: &Path,
        snap: SourceSnapshot,
        content_hash: String,
    ) -> Result<Analysed> {
        // 3. Decode read-only. Unsupported/broken decode is a recoverable error.
        //    HEIC/HEIF go through the macOS system decoder (see `decode`).
        let _ro = integrity::open_readonly(abs)?; // prove read-only open works
        let rgb = decode::open_rgb(abs, &self.paths.cache_dir().join("decode"))?;
        let (w, h) = (rgb.width(), rgb.height());

        let phash = phash::dhash(&rgb);

        // 4. Metadata.
        let md = metadata::extract(abs, Some((w, h)));

        // 5. AI analysis (all local, offline).
        //
        // Colour and scan-artefact analysis are cheap pixel statistics and always
        // come from the heuristic engine. Everything that needs a model — the
        // embedding, what the photograph shows, its text and its faces — comes
        // from a single-pass analyser when one is registered (Apple Vision), and
        // from the heuristic engine otherwise.
        let cancel = self.cancel;
        let color = self
            .engines
            .engine_for(Capability::Color)
            .color(&rgb, cancel)?;
        let mut scan_art = self
            .engines
            .engine_for(Capability::ScanArtifact)
            .scan_artifact(&rgb, cancel)?;
        // A camera exposure is not a flatbed scan, whatever the border looks
        // like (D-088).
        if md.camera_exposure() {
            scan_art.value.likely_scanned_print = false;
        }

        let analyser = self.engines.file_analyser();
        // A real model failing on one photograph must not fail the run; fall
        // back to the heuristic engine for that file and carry on.
        let analysis = match &analyser {
            Some(engine) => match engine.analyse_file(abs, cancel) {
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

        // Each face's identity embedding and crop. Prefer the embedding the
        // analyser produced from the full-resolution original; only fall back
        // to re-embedding the decoded copy when it did not provide one.
        let face_engine = self.engines.engine_for(Capability::FaceEmbedding);
        let mut prepared_faces = Vec::with_capacity(faces.value.len());
        for f in faces.value {
            let (vector, model_id, model_version) = match &f.embedding {
                Some(v) => (v.clone(), faces.meta.model_id.clone(), faces.meta.model_version.clone()),
                None => {
                    let fe = face_engine.face_embedding(&rgb, &f, cancel)?;
                    (fe.value.vector, fe.meta.model_id, fe.meta.model_version)
                }
            };
            let crop = crop_face_image(&rgb, &f);
            prepared_faces.push(PreparedFace { detection: f, vector, model_id, model_version, crop });
        }

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
        let file_id = file_id_for(self.drive_id, &item.root_id, &item.relative_path);
        let thumb = thumbnail::generate(&rgb, self.thumbs_dir, &file_id, self.thumbnail_max_edge)?;
        if !thumb.decode_ok {
            return Err(Error::Other("thumbnail failed to decode".into()));
        }

        // 8. Re-stat the original and assert it is unchanged. HARD SAFETY GATE.
        snap.assert_unchanged(abs)?;

        Ok(Analysed {
            file_id,
            snap,
            content_hash,
            phash,
            md,
            color: color.value,
            scene: scene.value,
            scan_art: scan_art.value,
            embedding,
            date_est,
            thumb,
            faces: prepared_faces,
            ocr_text,
            width: w,
            height: h,
        })
    }
}

/// Turn a caught panic into that photograph's failure.
fn panic_to_error(payload: Box<dyn std::any::Any + Send>) -> Error {
    let msg = payload
        .downcast_ref::<&str>()
        .map(|m| m.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown internal error".into());
    Error::Other(format!("internal error: {msg}"))
}

/// Work through `items` on up to `workers` threads, handing each result to
/// `on_result` on the calling thread as soon as it is ready.
///
/// Items are started in order, one per free thread. Nothing new is started
/// once `stop` says so — it is asked before every item and while waiting — or
/// once `on_result` returns `false`; results already in flight are still
/// delivered, so a photograph that was fully read is never thrown away.
///
/// Returns the index of the first item never started (everything from there
/// on was left untouched) and whether it was `stop` that ended the work.
fn in_parallel<T: Sync, R: Send>(
    items: &[T],
    workers: usize,
    stop: &(dyn Fn() -> bool + Sync),
    work: &(dyn Fn(&T) -> R + Sync),
    mut on_result: impl FnMut(usize, R) -> bool,
) -> (usize, bool) {
    let next = AtomicUsize::new(0);
    let halt = AtomicBool::new(false);
    let stopped = AtomicBool::new(false);
    std::thread::scope(|s| {
        let (tx, rx) = std::sync::mpsc::channel::<(usize, R)>();
        for _ in 0..workers.clamp(1, items.len().max(1)) {
            let tx = tx.clone();
            let (next, halt, stopped) = (&next, &halt, &stopped);
            s.spawn(move || loop {
                if halt.load(Ordering::SeqCst) {
                    break;
                }
                if stop() {
                    stopped.store(true, Ordering::SeqCst);
                    halt.store(true, Ordering::SeqCst);
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= items.len() {
                    break;
                }
                if tx.send((i, work(&items[i]))).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(250)) {
                Ok((i, r)) => {
                    if !on_result(i, r) {
                        halt.store(true, Ordering::SeqCst);
                    }
                }
                // Every thread may be deep inside one slow photograph; a stop
                // asked for meanwhile still stops anything new from starting.
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if !halt.load(Ordering::SeqCst) && stop() {
                        stopped.store(true, Ordering::SeqCst);
                        halt.store(true, Ordering::SeqCst);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    (next.load(Ordering::SeqCst).min(items.len()), stopped.load(Ordering::SeqCst))
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
        // The same cause deserves the same words wherever it is noticed. A
        // drive that is simply not plugged in was reporting "scan path is not a
        // directory", which describes a syscall rather than the situation.
        if !opts.path.exists() {
            return Err(Error::DriveDisconnected(format!(
                "{} is no longer available — reconnect the drive and start the scan again",
                opts.path.display()
            )));
        }
        if !opts.path.is_dir() {
            return Err(Error::InvalidArgs(format!(
                "scan path is not a directory: {}",
                opts.path.display()
            )));
        }
        Ok(())
    }

    fn run_index(&self, opts: &IndexOptions, dry_run: bool) -> Result<IndexSummary> {
        // Everything from here on belongs to this run, including a stop asked
        // for while preflight is still working. Taken before preflight so that
        // window is covered rather than being a gap where Stop does nothing.
        let run_started_at = std::time::SystemTime::now();

        self.preflight(opts)?;

        // Engage the network isolation guard for the whole indexing operation.
        net::reset_blocked_attempts();
        let _guard = OfflineGuard::engage();

        let drive_repo = DriveRepo::new(self.archive);
        let drive = drive_repo
            .get_by_number(opts.drive_number)?
            .ok_or_else(|| Error::InvalidArgs(format!("drive {} not registered", opts.drive_number)))?;
        let root_id = drive_repo.ensure_root(&drive.id, "")?;

        // One scan of a drive at a time. Two would claim each other's
        // photographs — a lease lasts five minutes and one slow photograph can
        // hold a batch far longer — and double every heartbeat and failure.
        // Only a scan that is really alive blocks: its heartbeat is recent and
        // its process still exists, so a scan killed a minute ago never locks
        // the owner out of their own drive (D-090).
        if !dry_run {
            if let Some(other) = crate::inventory::running_scans(self.archive)?
                .into_iter()
                .find(|r| r.drive_number == opts.drive_number && r.alive && r.pid != Some(i64::from(std::process::id())))
            {
                return Err(Error::InvalidArgs(format!(
                    "Drive {} is already being scanned (last activity {} minute(s) ago). \
                     Wait for that scan to finish, or stop it first.",
                    opts.drive_number,
                    other.silent_for_minutes.unwrap_or(0)
                )));
            }
        }
        drive_repo.set_status(&drive.id, "online")?;

        // Resume an existing run if requested and present. A run left in either
        // "running" or "interrupted" state is continued under its own id.
        let run_id = if opts.resume {
            match Progress::load(self.paths)? {
                Some(p) if p.status == "running" || p.status == "interrupted" => {
                    // The run is this process's now: say so, or anything
                    // asking whether the drive is being scanned would read the
                    // old process — or "interrupted" — and be wrong.
                    let _ = self.archive.execute(
                        "UPDATE scan_runs SET outcome = 'running', pid = ?2, heartbeat_at = ?3
                          WHERE id = ?1",
                        params![p.run_id, i64::from(std::process::id()), now_iso8601()],
                    );
                    p.run_id
                }
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
            extra_extensions: opts.extra_extensions.clone(),
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
        // Set when the scan root vanished mid-run, so the outer loop stops trying
        // to claim more work from a drive that is no longer there.
        let mut disconnected: Option<String> = None;

        // Photographs are read and analysed several at a time, and written to
        // the catalogue one at a time, here, as each is ready (D-087).
        let workers = opts.config.analysis_workers.max(1);
        let moved_candidates =
            if dry_run { HashSet::new() } else { self.missing_hashes(&drive.id)? };
        let analyst = Analyst {
            engines: &self.engines,
            paths: self.paths,
            logger: &self.logger,
            cancel: &self.cancel,
            root: &opts.path,
            thumbs_dir: &thumbs_dir,
            thumbnail_max_edge: opts.config.thumbnail_max_edge,
            drive_id: &drive.id,
            moved_candidates: &moved_candidates,
            dry_run,
        };
        let (cancel, paths) = (&self.cancel, self.paths);

        loop {
            // Two ways to be asked to stop, checked in the same place.
            //
            // The token stops a run from inside this process. The file stops
            // whichever process is actually scanning — which matters because a
            // scan is often started from the command line and left for two
            // days, while the owner is looking at the desktop app. See
            // `crate::stop`.
            if self.cancel.is_cancelled() || crate::stop::requested_since(self.paths, run_started_at) {
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
            // The batch is on record from the moment it is claimed, not only
            // once it finishes, so a run that dies inside one leaves a row that
            // says which batch it was in.
            let batch_id = if dry_run {
                String::new()
            } else {
                self.begin_batch(&run_id, batch_no, batch.len())
            };
            let mut batch_success = 0u64;
            let mut batch_failure = 0u64;
            // What this batch wrote, for the verifier to read back.
            let mut batch_file_ids: Vec<String> = Vec::new();
            // An error that ends the run: a hard safety halt, or the queue
            // itself failing to record what happened.
            let mut fatal: Option<Error> = None;

            // Stop is answered between photographs, not only between batches.
            // Every commit is per-file and atomic, so stopping there is exactly
            // as safe as stopping at the batch boundary — and a batch of 64
            // large TIFFs can take an hour, which is how "stop at the next
            // batch boundary" became "carry on for an hour after being told to
            // stop" on a real drive. Photographs already being read are
            // finished and kept; nothing new is started.
            // Named people, decrypted once for the batch rather than once per
            // face. Someone named mid-batch is recognised from the next one.
            let exemplars = if dry_run {
                crate::faces::PersonExemplars::default()
            } else {
                FaceRepo::new(self.archive).person_exemplars(self.key)?
            };
            let stop_seen = AtomicBool::new(false);
            let should_stop = || {
                let stop = cancel.is_cancelled() || crate::stop::requested_since(paths, run_started_at);
                if stop {
                    stop_seen.store(true, Ordering::SeqCst);
                }
                stop
            };
            // A panic while processing one photograph is that photograph's
            // failure, never the run's. Without this, a slice-index bug tripped
            // by OCR text in one folder crashed the whole scan every couple of
            // minutes, and before crashes were caught at the thread boundary it
            // froze the app for two days. Per-file commits make unwinding safe:
            // the file's transaction either committed or it did not.
            let work = |item: &QueueItem| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| analyst.prepare(item)))
                    .unwrap_or_else(|p| Err(panic_to_error(p)))
            };

            let (first_unstarted, stop_asked) = in_parallel(&batch, workers, &should_stop, &work, |i, prepared| {
                let item = &batch[i];
                // Nothing is written once the run is halting.
                if fatal.is_some() {
                    return false;
                }
                let processed = prepared.and_then(|p| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        self.commit_prepared(&analyst, &drive, item, p, &exemplars, dry_run)
                    }))
                    .unwrap_or_else(|p| Err(panic_to_error(p)))
                });
                // After a stop or an unplug, a photograph still being read when
                // it happened most likely failed *because* of it.
                let winding_down = disconnected.is_some() || stop_seen.load(Ordering::SeqCst);
                let queue_result = match processed {
                    Ok((rel, file_id)) => {
                        batch_success += 1;
                        summary.files_done += 1;
                        progress.last_completed_file = Some(rel);
                        batch_file_ids.push(file_id);
                        let done = if dry_run { Ok(()) } else { q.complete(&item.id) };
                        publish(&mut progress, &summary, batch_no, self.paths, dry_run);
                        if !dry_run {
                            self.beat(&run_id);
                        }
                        done
                    }
                    Err(e) if e.is_hard_halt() => {
                        // Immediate hard halt (integrity, unsafe path, network...).
                        logger
                            .error("hard_halt")
                            .relative_path(item.relative_path.clone())
                            .code(format!("{}", e.exit_code()))
                            .field("error", format!("{e}"))
                            .emit_best_effort();
                        fatal = Some(e);
                        return false;
                    }
                    Err(Error::DriveDisconnected(reason)) => {
                        // The drive left. Everything still queued lives on it,
                        // so carrying on would fail thousands of files one at a
                        // time, burn their retries and leave the queue looking
                        // like the photographs were bad rather than absent.
                        //
                        // This is the interruption the whole pipeline is built
                        // around — the same shape as a stop — so the lease is
                        // released, the item stays queued, and reconnecting and
                        // starting again carries on where it left off.
                        logger
                            .warn("drive_disconnected")
                            .relative_path(item.relative_path.clone())
                            .field("reason", reason.clone())
                            .emit_best_effort();
                        disconnected = Some(reason);
                        if dry_run { Ok(()) } else { q.release(&item.id) }
                    }
                    // Handed back unspent, not counted against the photograph.
                    Err(_) if winding_down => {
                        if dry_run { Ok(()) } else { q.release(&item.id) }
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
                        let recorded = if dry_run {
                            Ok(())
                        } else {
                            q.fail(&item.id, "PROCESS", &format!("{e}"), retryable)
                        };
                        // A failure is news too. Without this the heartbeat only
                        // beats on success, so a run of files that each take
                        // minutes to fail — a wedged decoder times out at ten —
                        // writes nothing for long enough to be called stalled
                        // while it is working exactly as designed, and the
                        // failure count on screen lags behind the truth.
                        publish(&mut progress, &summary, batch_no, self.paths, dry_run);
                        if !dry_run {
                            self.beat(&run_id);
                        }
                        recorded
                    }
                };
                if let Err(e) = queue_result {
                    fatal = Some(e);
                    return false;
                }
                disconnected.is_none()
            });

            if let Some(e) = fatal {
                if e.is_hard_halt() {
                    progress.files_done = summary.files_done;
                    progress.files_failed = summary.files_failed;
                    progress.status = "halted".into();
                    progress.touch();
                    if !dry_run {
                        progress.write(self.paths)?;
                    }
                    summary.halted = true;
                    summary.halt_reason = Some(format!("{e}"));
                    self.finish_run(&run_id, "halted", &summary)?;
                }
                return Err(e);
            }
            if stop_asked {
                logger.warn("cancelled").emit_best_effort();
            }
            if stop_asked || disconnected.is_some() {
                interrupted = true;
                // Photographs never started go straight back to the queue,
                // unspent, rather than waiting out a lease.
                if !dry_run {
                    for item in &batch[first_unstarted..] {
                        q.release(&item.id)?;
                    }
                }
            }

            // Nothing left to read from a drive that has gone; stop before
            // claiming another batch of files that are all on it.
            if disconnected.is_some() {
                break;
            }

            let elapsed = batch_started.elapsed().as_secs_f64().max(1e-6);
            let throughput = batch.len() as f64 / elapsed;

            // Per-batch verification of what this batch wrote.
            if !dry_run {
                let report = self.verify_batch(
                    opts,
                    throughput,
                    crate::verifier::Scope::Files(&batch_file_ids),
                )?;
                self.act_on_report(
                    &report, opts, &run_id, batch_no, &logger,
                    &mut consecutive_verifier_failures, &mut progress, &mut summary,
                )?;
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
            if !dry_run {
                self.record_batch(&batch_id, batch_success, batch_failure, throughput)?;
            }
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
        if let Some(reason) = &disconnected {
            // Says what happened in the owner's terms. "Stopped for safety" was
            // both alarming and wrong for a drive someone unplugged.
            summary.halt_reason = Some(reason.clone());
        }

        if interrupted {
            // The request has been carried out, so take it off disk. Leaving it
            // would not block the next run — that one starts later than this
            // file was written — but a stale request makes `stop::requested`
            // read true for ever, which is a lie about the state of the system.
            let _ = crate::stop::clear(self.paths);

            // Leave the run resumable: record interrupted state, do not complete.
            // The counters come from the summary rather than from whatever the
            // last batch boundary happened to publish, because an interruption
            // lands mid-batch far more often than not.
            progress.files_done = summary.files_done;
            progress.files_failed = summary.files_failed;
            progress.status = "interrupted".into();
            progress.touch();
            if !dry_run {
                progress.write(self.paths)?;
            }
            self.finish_run(&run_id, "interrupted", &summary)?;
        } else if !summary.halted {
            // Every photograph on the drive, read back once more now the run
            // is over. Batches verify only what they wrote (D-087); this is
            // what still notices a thumbnail lost, or an original changed,
            // after its own batch had passed. Linear in the drive, once per
            // scan, where the old per-batch sweep was quadratic in the archive.
            if !dry_run {
                let report = self.verify_batch(
                    opts,
                    f64::NAN,
                    crate::verifier::Scope::Drive(&drive.id),
                )?;
                logger
                    .info("drive_verified")
                    .field("summary", report.summary())
                    .emit_best_effort();
                // Anything short of a pass is written down, even when the
                // failure policy lets the run finish: this is the last look at
                // the drive before the owner is told it can be unplugged.
                if !report.ok() {
                    self.write_report(&run_id, &report)?;
                }
                self.act_on_report(
                    &report, opts, &run_id, summary.batches, &logger,
                    &mut consecutive_verifier_failures, &mut progress, &mut summary,
                )?;
            }
            // Group this drive's new faces with their look-alikes, so the
            // People screen offers groups to name rather than every face one by
            // one (D-089). A convenience: failing here must not fail a scan
            // whose photographs are all safely catalogued.
            if !dry_run {
                let repo = FaceRepo::new(self.archive);
                let grouped = repo.group_ungrouped(Some(&drive.id), self.key).and_then(|r| {
                    // Then join this drive's groups to the same people on
                    // other drives (D-091).
                    repo.merge_lookalike_groups(self.key).map(|m| (r, m))
                });
                match grouped {
                    Ok((r, m)) => logger
                        .info("faces_grouped")
                        .field("faces", r.faces_considered as i64)
                        .field("groups", r.groups_created as i64)
                        .field("grouped", r.faces_grouped as i64)
                        .field("merged_across_drives", m.groups_merged as i64)
                        .emit_best_effort(),
                    Err(e) => logger
                        .warn("faces_grouping_failed")
                        .field("error", format!("{e}"))
                        .emit_best_effort(),
                }
            }
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

    /// Content hashes of this drive's photographs that are recorded as
    /// `missing` — the ones a moved file could turn out to be.
    fn missing_hashes(&self, drive_id: &str) -> Result<HashSet<String>> {
        let mut stmt = self.archive.prepare(
            "SELECT content_hash FROM files
              WHERE drive_id = ?1 AND status = 'missing' AND content_hash IS NOT NULL",
        )?;
        let hashes = stmt
            .query_map([drive_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<HashSet<_>, _>>()?;
        Ok(hashes)
    }

    /// Re-point a `missing` catalogue row at a file that has reappeared
    /// elsewhere on the same drive. Returns the adopted row's file id, `None`
    /// when this really is a new photograph.
    fn adopt_relocated_file(
        &self,
        drive: &crate::drive::Drive,
        item: &QueueItem,
        snap: &SourceSnapshot,
        content_hash: &str,
    ) -> Result<Option<String>> {
        let prior: Option<String> = self
            .archive
            .query_row(
                "SELECT id FROM files
                  WHERE drive_id = ?1 AND content_hash = ?2 AND status = 'missing'
                  LIMIT 1",
                params![drive.id, content_hash],
                |r| r.get(0),
            )
            .optional()?;
        let Some(file_id) = prior else { return Ok(None) };

        let now = now_iso8601();
        self.archive.execute(
            "UPDATE files
                SET relative_path = ?2, filename = ?3, status = 'complete',
                    size_bytes = ?4, source_mtime_ns = ?5, updated_at = ?6
              WHERE id = ?1",
            params![
                file_id,
                item.relative_path,
                std::path::Path::new(&item.relative_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| item.relative_path.clone()),
                snap.size_bytes as i64,
                snap.mtime_ns,
                now,
            ],
        )?;
        self.logger
            .info("file_relocated")
            .field("path", item.relative_path.clone())
            .emit_best_effort();
        Ok(Some(file_id))
    }

    /// Write one prepared photograph to the catalogue, atomically.
    ///
    /// Runs on the pipeline's own thread — the only one holding the catalogue
    /// connection. Returns the relative path and the file id that were written.
    fn commit_prepared(
        &self,
        analyst: &Analyst,
        drive: &crate::drive::Drive,
        item: &QueueItem,
        prepared: Prepared,
        exemplars: &crate::faces::PersonExemplars,
        dry_run: bool,
    ) -> Result<(String, String)> {
        let analysed = match prepared {
            Prepared::Moved { abs, snap, content_hash } => {
                if let Some(file_id) =
                    self.adopt_relocated_file(drive, item, &snap, &content_hash)?
                {
                    return Ok((item.relative_path.clone(), file_id));
                }
                // Another photograph in this run has already claimed that row —
                // two copies of one moved picture — so this one is new after all.
                analyst.analyse(item, &abs, snap, content_hash)?
            }
            Prepared::Analysed(a) => *a,
        };

        if dry_run {
            // Report the proposed record; write nothing to the catalogue.
            println!(
                "[dry-run] {} | {}x{} | phash={} | faces={} | {} | date={}",
                item.relative_path,
                analysed.width,
                analysed.height,
                analysed.phash,
                analysed.faces.len(),
                analysed.scene.description,
                dates::describe(&analysed.date_est)
            );
            return Ok((item.relative_path.clone(), analysed.file_id));
        }

        // 9. Atomic commit to archive.db.
        let tx = self.archive.unchecked_transaction()?;
        self.commit_file(&tx, drive, item, &analysed, exemplars)?;
        tx.commit()?;

        Ok((item.relative_path.clone(), analysed.file_id))
    }

    fn commit_file(
        &self,
        tx: &Connection,
        drive: &crate::drive::Drive,
        item: &QueueItem,
        a: &Analysed,
        exemplars: &crate::faces::PersonExemplars,
    ) -> Result<()> {
        let file_id = a.file_id.as_str();
        let (snap, md, color, scene, scan_art, embedding, date_est, thumb) = (
            &a.snap, &a.md, &a.color, &a.scene, &a.scan_art, &a.embedding, &a.date_est, &a.thumb,
        );
        let (content_hash, phash, ocr_text) = (&a.content_hash, &a.phash, a.ocr_text.as_deref());
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
        for f in &a.faces {
            let d = &f.detection;
            let face_id = face_repo.insert_face(
                file_id,
                (d.x, d.y, d.w, d.h),
                d.quality,
                &f.model_id,
                &f.model_version,
                &f.vector,
                self.key,
            )?;

            // A small crop of the face, kept locally so the gallery is browsable
            // with every drive unplugged. Encrypted, like the embedding.
            if let Some((jpeg, w, h)) = &f.crop {
                face_repo.store_thumbnail(&face_id, jpeg, *w, *h, self.key)?;
            }

            // Recognise people the user has already named. This is only ever a
            // suggestion — naming stays a human decision (D-007).
            if let Some(hit) = exemplars.best_match(
                &f.vector,
                &f.model_id,
                &f.model_version,
                crate::faces::PERSON_MATCH_THRESHOLD,
            ) {
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
        // Names AtlasDrive actually read on things in the picture — a bottle,
        // a van, a shop front, a magazine.
        //
        // Sourced from OCR text, never from image features: the tag means "this
        // name appears in the photograph", which is a checkable claim. Guessing
        // a logo from pixels would not be. See D-061.
        for hit in crate::ai::names::detect(ocr_text.unwrap_or("")) {
            let tag_id = self.upsert_tag(tx, &hit.tag, "automatic")?;
            tx.execute(
                "INSERT OR IGNORE INTO file_tags (file_id, tag_id, confidence, source, created_at)
                 VALUES (?1,?2,?3,'name',?4)",
                params![file_id, tag_id, hit.confidence(), now],
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
        //
        // Read the tags back from the catalogue rather than from the scene
        // analysis: names read off things in the picture and system tags like
        // likely-scan are inserted above but were never scene concepts, so
        // building the text index from concepts alone left them unsearchable
        // by typing — the tag existed, the chip showed it, and the search box
        // denied it.
        let tag_text: String = {
            let mut stmt = tx.prepare(
                "SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
                  WHERE ft.file_id = ?1 ORDER BY t.name",
            )?;
            let names: Vec<String> = stmt
                .query_map([file_id], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            names.join(" ")
        };
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

    fn verify_batch(
        &self,
        opts: &IndexOptions,
        throughput: f64,
        scope: crate::verifier::Scope,
    ) -> Result<crate::verifier::VerifierReport> {
        let ctx = crate::verifier::VerifyContext {
            archive: self.archive,
            queue: Some(self.queue),
            paths: self.paths,
            config: &opts.config,
            key: Some(self.key),
            observed_throughput: Some(throughput),
            network_blocked_attempts: net::blocked_attempts(),
        };
        crate::verifier::run_scoped(&ctx, scope)
    }

    /// Apply the failure policy (`docs/13`) to a verifier report.
    ///
    /// A halting check ends the run at once; a failing one is tolerated until
    /// `max_consecutive_verifier_failures` in a row, then ends it with a
    /// report. `Err` means the run is over and has been recorded as halted.
    #[allow(clippy::too_many_arguments)]
    fn act_on_report(
        &self,
        report: &crate::verifier::VerifierReport,
        opts: &IndexOptions,
        run_id: &str,
        batch_no: u64,
        logger: &Logger,
        consecutive_verifier_failures: &mut u32,
        progress: &mut Progress,
        summary: &mut IndexSummary,
    ) -> Result<()> {
        if report.has_halt() {
            logger
                .error("verifier_halt")
                .field("summary", report.summary())
                .emit_best_effort();
            progress.status = "halted".into();
            progress.write(self.paths)?;
            summary.halted = true;
            summary.halt_reason = Some(report.summary());
            self.write_report(run_id, report)?;
            self.finish_run(run_id, "halted", summary)?;
            return Err(Error::VerifierFailure(report.summary()));
        }
        if report.ok() {
            *consecutive_verifier_failures = 0;
            progress.consecutive_verifier_failures = 0;
            return Ok(());
        }
        *consecutive_verifier_failures += 1;
        progress.consecutive_verifier_failures = *consecutive_verifier_failures;
        logger
            .warn("verifier_failure")
            .batch(batch_no)
            .field("consecutive", *consecutive_verifier_failures)
            .emit_best_effort();
        if *consecutive_verifier_failures >= opts.config.max_consecutive_verifier_failures {
            self.write_report(run_id, report)?;
            progress.status = "halted".into();
            progress.write(self.paths)?;
            summary.halted = true;
            summary.halt_reason = Some("repeated verifier failure".into());
            self.finish_run(run_id, "halted", summary)?;
            return Err(Error::RepeatedVerifierFailure(report.summary()));
        }
        Ok(())
    }

    fn run_verify_only(&self, opts: &IndexOptions) -> Result<IndexSummary> {
        let report = self.verify_batch(opts, f64::NAN, crate::verifier::Scope::Catalogue)?;
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
        // One transaction for the whole reconciliation: a drive re-scanned from
        // a different root marks every photograph on it, and one commit per row
        // is one disk sync per row.
        let tx = self.archive.unchecked_transaction()?;

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

        tx.commit()?;
        Ok(out)
    }

    fn start_run(&self, drive_id: &str, opts: &IndexOptions, dry_run: bool) -> Result<String> {
        let run_id = new_uuid();
        let mode = if dry_run { "dry-run" } else { "initial" };
        self.archive.execute(
            "INSERT INTO scan_runs (id, drive_id, drive_number, scan_root, mode, started_at, outcome, pid)
             VALUES (?1,?2,?3,?4,?5,?6,'running',?7)",
            params![
                run_id, drive_id, opts.drive_number, opts.path.to_string_lossy(), mode, now_iso8601(),
                i64::from(std::process::id())
            ],
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

    /// Record that a batch has been claimed and is being worked on.
    ///
    /// `docs/06` stage 3 asks for the batch *start* to be recorded, and it was
    /// not: a row appeared only once the batch had finished, so a batch in
    /// flight — including the one a run died inside — left no trace at all.
    /// Returns the row's id, which [`Self::record_batch`] closes.
    fn begin_batch(&self, run_id: &str, batch_no: u64, file_count: usize) -> String {
        let id = new_uuid();
        let _ = self.archive.execute(
            "INSERT INTO scan_batches (id, run_id, batch_number, started_at, file_count)
             VALUES (?1,?2,?3,?4,?5)",
            params![id, run_id, batch_no as i64, now_iso8601(), file_count as i64],
        );
        id
    }

    fn record_batch(
        &self,
        batch_id: &str,
        success: u64,
        failure: u64,
        throughput: f64,
    ) -> Result<()> {
        let _ = self.archive.execute(
            "UPDATE scan_batches
                SET ended_at=?2, success_count=?3, failure_count=?4, throughput_fps=?5
              WHERE id=?1",
            params![batch_id, now_iso8601(), success as i64, failure as i64, throughput],
        );
        Ok(())
    }

    /// Stamp the run as still alive.
    ///
    /// Called for every photograph, so "no heartbeat" means no work at all
    /// rather than no *successful* work. One indexed update against seconds of
    /// analysis per file; a failure to write must never fail a photograph, so
    /// the result is dropped.
    fn beat(&self, run_id: &str) {
        let _ = self.archive.execute(
            "UPDATE scan_runs SET heartbeat_at=?2 WHERE id=?1",
            params![run_id, now_iso8601()],
        );
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
