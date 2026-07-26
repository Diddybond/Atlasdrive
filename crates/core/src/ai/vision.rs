//! Apple Vision engine — real, on-device image understanding.
//!
//! This is the engine that lets AtlasDrive say what a photograph actually shows.
//! It provides object and scene classification, genuine text recognition, real
//! face detection and a learned 768-dimension feature print, all from Apple's
//! Vision framework: on-device, no model download, no licence, no network.
//!
//! It talks to the Swift worker in `vision/atlasdrive-vision.swift` over a
//! line-oriented pipe (D-009 anticipates a local analysis worker in another
//! language where model support is stronger). The worker is long-lived, because
//! spawning a process per photograph would dominate the cost of indexing a large
//! archive.
//!
//! ## Why this is a separate model partition
//!
//! Its embedding is Vision's feature print — 768 learned dimensions, from a
//! completely different space to the heuristic engine's 65-dimension colour
//! grid. The two must never be compared, which is exactly what the
//! `(model_id, model_version)` partitioning in the catalogue is for.
//!
//! ## What it deliberately does not provide
//!
//! No text embedding. Vision has no text tower, so a natural-language query
//! cannot be projected into the feature-print space. Object search works a
//! better way here: the classification labels are written into the catalogue's
//! full-text index, so "bike" matches photographs Vision labelled `bicycle`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use serde::Deserialize;

use super::types::*;
use super::{AiEngine, CancelToken, Capability};
use crate::error::{Error, Result};
use crate::util::now_iso8601;

pub const MODEL_ID: &str = "apple-vision";
/// Bump when the worker's output contract changes in a way that alters stored
/// analysis, so old rows stay in their own partition.
pub const MODEL_VERSION: &str = "1.0.0";

/// Name of the compiled worker binary.
pub const HELPER_BINARY: &str = "atlasdrive-vision";

/// Labels at or above this are worth storing as a concept tag. Vision emits a
/// long tail of low-confidence guesses; keeping them would fill the catalogue
/// with noise and make search worse, not better.
const TAG_CONFIDENCE_FLOOR: f32 = 0.20;

// ---------------------------------------------------------------------------
// Worker protocol
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct WorkerLabel {
    id: String,
    c: f32,
}

#[derive(Debug, Deserialize)]
struct WorkerFace {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    c: f32,
    /// Vision's capture-quality score for the face.
    #[serde(default)]
    q: f32,
    /// Feature print of the face crop — the identity embedding.
    #[serde(default)]
    fp: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct WorkerAnalysis {
    ok: bool,
    error: Option<String>,
    width: u32,
    height: u32,
    labels: Vec<WorkerLabel>,
    ocr: String,
    faces: Vec<WorkerFace>,
    #[serde(rename = "print")]
    feature_print: Vec<f32>,
}

/// The worker process plus its pipes, held together so a failed exchange can
/// tear the whole thing down and be restarted.
struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Photographs this process has analysed, so it can be retired before its
    /// memory becomes a problem for the machine.
    served: u32,
}

/// Retire the worker after this many photographs and start a fresh one.
///
/// A long-lived worker is the right design — spawning a process per photograph
/// would dominate the cost of indexing 200,000 files — but "long-lived" cannot
/// mean "for as long as the scan lasts". Observed on the owner's Mac during a
/// real overnight run: the worker reached 28GB resident after 77 minutes and
/// peaked around 40GB, pushing the machine 10GB into swap. Everything slowed
/// down, including the scan the worker existed to perform.
///
/// The cause was not reproducible from the files alone — feeding the thirty
/// largest TIFFs on that drive straight to the worker plateaus under 2GB — so
/// this does not pretend to fix a diagnosed leak. It removes the consequence:
/// however memory is being retained, it cannot accumulate past a few hundred
/// photographs, because the process holding it is replaced.
///
/// A restart costs roughly the time of one photograph, which against 400 is
/// under a third of a percent.
const MAX_PHOTOGRAPHS_PER_WORKER: u32 = 400;

pub struct VisionEngine {
    helper: PathBuf,
    caps: Vec<Capability>,
    /// `None` until first use, and reset to `None` if an exchange fails so the
    /// next call gets a fresh process rather than a broken pipe.
    worker: Mutex<Option<Worker>>,
}

impl VisionEngine {
    /// Locate the worker and confirm it responds, or return `None`.
    ///
    /// Returning `None` rather than an error is deliberate: a missing worker
    /// means "fall back to the heuristic engine", which is a supported state,
    /// not a failure. Indexing must never depend on it being present.
    pub fn detect() -> Option<Self> {
        Self::locate_helper().and_then(|p| Self::new(p).ok())
    }

    /// Build an engine around a specific worker binary.
    pub fn new(helper: PathBuf) -> Result<Self> {
        // Confirm this is really our worker before trusting it.
        let out = Command::new(&helper)
            .arg("--selftest")
            .output()
            .map_err(|e| Error::ModelMissing(format!("vision worker not runnable: {e}")))?;
        let banner = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || !banner.starts_with(HELPER_BINARY) {
            return Err(Error::ModelMissing(format!(
                "unexpected vision worker at {}: {banner:?}",
                helper.display()
            )));
        }
        Ok(Self {
            helper,
            caps: vec![
                Capability::VisualEmbedding,
                Capability::FaceDetection,
                Capability::Ocr,
                Capability::Scene,
            ],
            worker: Mutex::new(None),
        })
    }

    /// Where the worker may be found, in order of preference.
    ///
    /// The override comes first so a packaged app can point at the copy inside
    /// its own bundle without guessing.
    fn locate_helper() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("ATLASDRIVE_VISION_BIN") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Some(p);
            }
        }
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                // Alongside the CLI.
                candidates.push(dir.join(HELPER_BINARY));
                // Inside a .app. Tauri rewrites a `../vision/bin/x` resource path
                // to `Resources/_up_/vision/bin/x`, preserving the `..` as `_up_`,
                // so both the mangled and the flat layout are checked.
                let resources = dir.join("../Resources");
                candidates.push(resources.join("_up_/vision/bin").join(HELPER_BINARY));
                candidates.push(resources.join(HELPER_BINARY));
            }
        }
        // Development checkout.
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../vision/bin")
                .join(HELPER_BINARY),
        );
        candidates.into_iter().find(|p| p.is_file())
    }

    /// Send one path and read its result, restarting the worker once if the pipe
    /// has died (a crashed worker must not fail the whole run).
    fn exchange(&self, abs: &Path) -> Result<WorkerAnalysis> {
        match self.exchange_once(abs) {
            Ok(v) => Ok(v),
            Err(_first) => {
                *self.worker.lock().unwrap() = None;
                self.exchange_once(abs)
            }
        }
    }

    fn exchange_once(&self, abs: &Path) -> Result<WorkerAnalysis> {
        let mut guard = self.worker.lock().unwrap();

        // Retire a worker that has done its shift before handing it more work.
        if guard.as_ref().is_some_and(|w| w.served >= MAX_PHOTOGRAPHS_PER_WORKER) {
            if let Some(mut old) = guard.take() {
                // Closing stdin ends the worker's read loop, so it exits of its
                // own accord rather than being killed mid-analysis.
                drop(old.stdin);
                let _ = old.child.wait();
            }
        }

        if guard.is_none() {
            *guard = Some(self.spawn()?);
        }
        let worker = guard.as_mut().expect("worker present");

        // JSON-encode the request: a macOS filename may contain a newline, and a
        // bare-path protocol would let it inject an extra line — desynchronising
        // the stream so that every later photograph received the previous one's
        // analysis. Escaping makes framing independent of the filename.
        let request = serde_json::json!({ "path": abs.to_string_lossy() });
        writeln!(worker.stdin, "{request}")
            .map_err(|e| Error::Other(format!("vision worker write failed: {e}")))?;
        worker
            .stdin
            .flush()
            .map_err(|e| Error::Other(format!("vision worker flush failed: {e}")))?;

        worker.served += 1;

        let mut line = String::new();
        let read = worker
            .stdout
            .read_line(&mut line)
            .map_err(|e| Error::Other(format!("vision worker read failed: {e}")))?;
        if read == 0 {
            return Err(Error::Other("vision worker closed its output".into()));
        }
        serde_json::from_str::<WorkerAnalysis>(line.trim())
            .map_err(|e| Error::Other(format!("vision worker sent malformed JSON: {e}")))
    }

    fn spawn(&self) -> Result<Worker> {
        let mut child = Command::new(&self.helper)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::ModelMissing(format!("could not start vision worker: {e}")))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Ok(Worker { child, stdin, stdout, served: 0 })
    }

    fn meta(&self, confidence: f32, exec_ms: u64) -> AiMeta {
        AiMeta {
            model_id: MODEL_ID.to_string(),
            model_version: MODEL_VERSION.to_string(),
            processed_at: now_iso8601(),
            confidence,
            exec_ms,
        }
    }
}

impl Drop for VisionEngine {
    fn drop(&mut self) {
        if let Some(mut worker) = self.worker.lock().unwrap().take() {
            // Closing stdin ends the worker's read loop; then reap it.
            drop(worker.stdin);
            let _ = worker.child.wait();
        }
    }
}

/// Turn Vision's labels into the catalogue's scene shape.
///
/// The description is what the user reads on a result card, so it is built from
/// the labels themselves rather than invented, and the concepts carry Vision's
/// own confidences unchanged.
fn scene_from_labels(labels: &[WorkerLabel], faces: usize) -> SceneResult {
    let concepts: Vec<Concept> = labels
        .iter()
        .filter(|l| l.c >= TAG_CONFIDENCE_FLOOR)
        .map(|l| Concept { tag: l.id.clone(), confidence: l.c })
        .collect();

    // Vision's taxonomy includes explicit indoor/outdoor-ish scene terms; use
    // them when present rather than guessing from colour.
    const OUTDOOR: &[&str] = &[
        "outdoor", "sky", "beach", "mountain", "field", "forest", "garden", "landscape",
        "sea", "ocean", "snow", "street", "park", "tree", "plant", "water", "sunset",
    ];
    const INDOOR: &[&str] = &[
        "indoor", "room", "furniture", "kitchen", "table", "document", "screenshot",
        "food", "wall", "curtain", "bed", "chair",
    ];
    let score = |terms: &[&str]| -> f32 {
        labels
            .iter()
            .filter(|l| terms.iter().any(|t| l.id.to_lowercase().contains(t)))
            .map(|l| l.c)
            .fold(0.0f32, f32::max)
    };
    let outdoor = score(OUTDOOR);
    let indoor = score(INDOOR);

    let description = match labels.first() {
        Some(top) => {
            let named: Vec<&str> = labels
                .iter()
                .take(3)
                .filter(|l| l.c >= TAG_CONFIDENCE_FLOOR)
                .map(|l| l.id.as_str())
                .collect();
            if named.is_empty() {
                format!("possibly {}", top.id)
            } else {
                named.join(", ")
            }
        }
        None => "no recognisable subject".to_string(),
    };

    SceneResult {
        indoor_prob: indoor,
        outdoor_prob: outdoor,
        people_count: faces as u32,
        description,
        concepts,
    }
}

impl AiEngine for VisionEngine {
    fn model_id(&self) -> &str {
        MODEL_ID
    }
    fn model_version(&self) -> &str {
        MODEL_VERSION
    }
    fn capabilities(&self) -> &[Capability] {
        &self.caps
    }
    fn supports_file_analysis(&self) -> bool {
        true
    }

    fn analyse_file(
        &self,
        abs: &Path,
        cancel: &CancelToken,
    ) -> Result<Provenanced<FileAnalysis>> {
        if cancel.is_cancelled() {
            return Err(Error::Other("cancelled".into()));
        }
        let started = std::time::Instant::now();
        let raw = self.exchange(abs)?;
        if !raw.ok {
            return Err(Error::Other(format!(
                "vision could not analyse the image: {}",
                raw.error.unwrap_or_else(|| "unknown reason".into())
            )));
        }

        let faces: Vec<FaceDetection> = raw
            .faces
            .iter()
            .map(|f| FaceDetection {
                x: f.x,
                y: f.y,
                w: f.w,
                h: f.h,
                // Capture quality is the useful signal for "is this face worth
                // identifying"; detection confidence is near 1.0 for everything.
                quality: if f.q > 0.0 { f.q } else { f.c },
                embedding: (!f.fp.is_empty()).then(|| f.fp.clone()),
            })
            .collect();
        let scene = scene_from_labels(&raw.labels, faces.len());
        // Top-label confidence is the honest summary of "did it recognise this".
        let confidence = raw.labels.first().map(|l| l.c).unwrap_or(0.0);

        let embedding = (!raw.feature_print.is_empty())
            .then(|| Embedding::new(raw.feature_print.clone()));
        let ocr = (!raw.ocr.trim().is_empty()).then(|| OcrResult {
            text: raw.ocr.clone(),
            // Vision's per-line confidences are not surfaced by the worker;
            // presence of recognised text is the signal the catalogue needs.
            confidence: 1.0,
        });

        Ok(Provenanced::new(
            FileAnalysis {
                embedding,
                ocr,
                faces,
                scene: Some(scene),
                width: raw.width,
                height: raw.height,
            },
            self.meta(confidence, started.elapsed().as_millis() as u64),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worker is built by `vision/build.sh`; skip rather than fail when a
    /// checkout has not built it yet.
    fn engine() -> Option<VisionEngine> {
        VisionEngine::detect()
    }

    fn write_png(path: &Path, colour: [u8; 3], w: u32, h: u32) {
        image::RgbImage::from_pixel(w, h, image::Rgb(colour))
            .save(path)
            .unwrap();
    }

    #[test]
    fn reports_its_own_model_partition() {
        let Some(e) = engine() else { return };
        assert_eq!(e.model_id(), "apple-vision");
        assert!(e.supports_file_analysis());
        assert!(e.is_offline());
        // Must not collide with the heuristic engine's partition.
        assert_ne!(e.model_id(), crate::ai::local::MODEL_ID);
    }

    #[test]
    fn produces_a_real_feature_print_of_stable_dimension() {
        let Some(e) = engine() else { return };
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        write_png(&a, [200, 40, 40], 64, 64);

        let r = e.analyse_file(&a, &CancelToken::new()).unwrap();
        let emb = r.value.embedding.expect("a feature print");
        assert_eq!(emb.dim, 768, "Vision feature prints are 768-dimensional");
        assert!(emb.is_finite());
        assert_eq!(r.meta.model_id, MODEL_ID);
        assert_eq!(r.value.width, 64);
    }

    #[test]
    fn similar_images_are_closer_than_different_ones_in_the_learned_space() {
        let Some(e) = engine() else { return };
        let dir = tempfile::tempdir().unwrap();
        let (red, red2, blue) = (
            dir.path().join("red.png"),
            dir.path().join("red2.png"),
            dir.path().join("blue.png"),
        );
        write_png(&red, [200, 40, 40], 96, 96);
        write_png(&red2, [210, 50, 45], 96, 96);
        write_png(&blue, [30, 60, 200], 96, 96);

        let c = CancelToken::new();
        let v = |p: &Path| e.analyse_file(p, &c).unwrap().value.embedding.unwrap().vector;
        let (vr, vr2, vb) = (v(&red), v(&red2), v(&blue));
        let close = crate::util::cosine_similarity(&vr, &vr2);
        let far = crate::util::cosine_similarity(&vr, &vb);
        assert!(close > far, "near-identical {close} should beat different {far}");
    }

    /// A filename containing a newline must not desynchronise the worker stream.
    ///
    /// macOS permits newlines in filenames. With a bare-path protocol such a name
    /// injected an extra request line, so the worker replied twice while the
    /// caller read once — and from then on every photograph was given the
    /// *previous* one's labels, faces and OCR. Silent catalogue corruption.
    #[test]
    fn a_newline_in_a_filename_cannot_desync_the_worker() {
        let Some(e) = engine() else { return };
        let dir = tempfile::tempdir().unwrap();

        let awkward = dir.path().join("evil\nsecond.png");
        let normal = dir.path().join("normal.png");
        write_png(&awkward, [200, 30, 30], 64, 64);
        write_png(&normal, [30, 200, 30], 64, 64);

        let c = CancelToken::new();
        // The awkward name analyses correctly rather than half-consuming the pipe.
        let a = e.analyse_file(&awkward, &c).unwrap();
        assert_eq!(a.value.width, 64);

        // And the next call gets *its own* answer, not a stale one. A 96x64 image
        // proves the reply belongs to this request and not the previous file.
        let wide = dir.path().join("wide.png");
        write_png(&wide, [30, 30, 200], 96, 64);
        let b = e.analyse_file(&wide, &c).unwrap();
        assert_eq!(b.value.width, 96, "reply must correspond to the file just sent");

        let n = e.analyse_file(&normal, &c).unwrap();
        assert_eq!(n.value.width, 64);
    }

    #[test]
    fn an_unreadable_file_is_an_error_not_a_crash() {
        let Some(e) = engine() else { return };
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("not-an-image.png");
        std::fs::write(&bad, b"this is not a PNG").unwrap();
        assert!(e.analyse_file(&bad, &CancelToken::new()).is_err());

        // And the engine still works afterwards — one bad file must not poison
        // the worker for the rest of the run.
        let good = dir.path().join("good.png");
        write_png(&good, [10, 120, 90], 48, 48);
        assert!(e.analyse_file(&good, &CancelToken::new()).is_ok());
    }

    #[test]
    fn the_original_is_never_modified_by_analysis() {
        let Some(e) = engine() else { return };
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("original.png");
        write_png(&p, [123, 200, 90], 80, 60);
        let before = std::fs::metadata(&p).unwrap();

        e.analyse_file(&p, &CancelToken::new()).unwrap();

        let after = std::fs::metadata(&p).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }

    #[test]
    fn cancellation_is_respected() {
        let Some(e) = engine() else { return };
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.png");
        write_png(&p, [1, 2, 3], 32, 32);
        let c = CancelToken::new();
        c.cancel();
        assert!(e.analyse_file(&p, &c).is_err());
    }

    #[test]
    fn labels_become_concept_tags_above_the_confidence_floor() {
        let labels = vec![
            WorkerLabel { id: "bicycle".into(), c: 0.91 },
            WorkerLabel { id: "wheel".into(), c: 0.42 },
            WorkerLabel { id: "spurious".into(), c: 0.05 },
        ];
        let scene = scene_from_labels(&labels, 0);
        let tags: Vec<&str> = scene.concepts.iter().map(|c| c.tag.as_str()).collect();
        assert_eq!(tags, vec!["bicycle", "wheel"], "low-confidence noise is dropped");
        assert!(scene.description.contains("bicycle"));
    }

    #[test]
    fn outdoor_and_indoor_are_read_from_labels_not_guessed_from_colour() {
        let outdoors = scene_from_labels(
            &[WorkerLabel { id: "beach".into(), c: 0.8 }],
            0,
        );
        assert!(outdoors.outdoor_prob > outdoors.indoor_prob);

        let indoors = scene_from_labels(
            &[WorkerLabel { id: "kitchen".into(), c: 0.7 }],
            0,
        );
        assert!(indoors.indoor_prob > indoors.outdoor_prob);
    }
}

#[cfg(test)]
mod worker_lifetime_tests {
    use super::*;
    use std::io::Write as _;

    /// A stand-in worker that answers the selftest and then reports its own
    /// process id for every request, so a restart is directly observable.
    fn stub_worker(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("atlasdrive-vision");
        let script = r#"#!/bin/sh
if [ "$1" = "--selftest" ]; then echo "atlasdrive-vision 1"; exit 0; fi
while IFS= read -r _line; do
  printf '{"ok":true,"error":null,"width":%s,"height":1,"faces":[],"labels":[],"ocr":"","print":[]}\n' "$$"
done
"#;
        std::fs::write(&path, script).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    /// The behaviour that matters: one process serves many photographs — the
    /// whole reason the worker is long-lived — but it is replaced before it has
    /// served too many.
    #[test]
    fn the_worker_is_reused_and_then_retired() {
        let dir = tempfile::tempdir().unwrap();
        let engine = VisionEngine::new(stub_worker(dir.path())).unwrap();
        let file = dir.path().join("photo.jpg");
        std::fs::File::create(&file).unwrap().write_all(b"x").unwrap();

        // The stub reports its pid in `width`.
        let first = engine.exchange(&file).unwrap().width;
        for _ in 1..MAX_PHOTOGRAPHS_PER_WORKER {
            assert_eq!(
                engine.exchange(&file).unwrap().width,
                first,
                "the worker must be reused, not respawned per photograph"
            );
        }

        // The next request crosses the limit and must land on a new process.
        let after = engine.exchange(&file).unwrap().width;
        assert_ne!(after, first, "the worker should have been retired by now");
    }

    /// Retiring must not lose the request that triggered it.
    #[test]
    fn the_photograph_that_triggers_a_restart_is_still_analysed() {
        let dir = tempfile::tempdir().unwrap();
        let engine = VisionEngine::new(stub_worker(dir.path())).unwrap();
        let file = dir.path().join("photo.jpg");
        std::fs::File::create(&file).unwrap().write_all(b"x").unwrap();

        for _ in 0..MAX_PHOTOGRAPHS_PER_WORKER {
            engine.exchange(&file).unwrap();
        }
        let result = engine.exchange(&file).expect("the restart must not drop the request");
        assert!(result.ok);
    }
}
