//! # Local AI Engine
//!
//! A dedicated subsystem for all machine-learning analysis, deliberately
//! decoupled from the UI, database and indexing pipeline (see the AI Engine
//! section of the build brief and `docs/03_ARCHITECTURE.md`).
//!
//! Design points honoured here:
//!   * **Stable interfaces.** Every capability is a trait method returning a
//!     typed result plus [`AiMeta`] provenance (model id, version, processing
//!     date, confidence, execution time). Underlying models can be replaced
//!     without touching database contracts.
//!   * **Local + offline.** The default [`local::LocalHeuristicEngine`] performs
//!     all work in-process with no network. Inference must never require the
//!     internet; a future cloud plugin is opt-in only.
//!   * **Plug-in architecture.** [`EngineRegistry`] lets additional local models
//!     be registered per capability without architectural change.
//!   * **Model versioning.** Results carry model id + version so analysis can be
//!     re-run with newer models without rebuilding the whole archive, and so
//!     embedding spaces are never silently mixed.
//!   * **Cancellation & background friendliness.** A [`CancelToken`] threads
//!     through long operations; the pipeline supplies queuing/resume.

pub mod brands;
pub mod names;
pub mod local;
pub mod text;
#[cfg(target_os = "macos")]
pub mod vision;
pub mod types;

pub use types::*;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use image::RgbImage;

use crate::error::Result;

/// Cooperative cancellation token for background AI work.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// The capabilities an engine may implement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    VisualEmbedding,
    /// Embedding a natural-language query into the *visual* embedding space.
    TextEmbedding,
    FaceDetection,
    FaceEmbedding,
    Ocr,
    Scene,
    Color,
    ScanArtifact,
    DateEvidence,
}

/// The stable interface every AI backend implements. Methods default to
/// `unsupported` so a plugin need only implement the capabilities it provides.
pub trait AiEngine: Send + Sync {
    /// Identifier of the backing model family (e.g. `local-heuristic`).
    fn model_id(&self) -> &str;
    /// Model version string; participates in DB partitioning.
    fn model_version(&self) -> &str;
    /// Which capabilities this engine provides.
    fn capabilities(&self) -> &[Capability];
    /// Whether this engine can run without any network access.
    fn is_offline(&self) -> bool {
        true
    }

    /// True when [`AiEngine::analyse_file`] is the efficient path for this
    /// engine, and the pipeline should prefer it over the per-capability calls.
    /// Capabilities this engine can perform on an image already in memory.
    ///
    /// Distinct from [`capabilities`], which is what the engine can produce
    /// *by any route*. An engine that works by handing a file path to an
    /// outside process cannot be asked for one property of a decoded
    /// `RgbImage`; it only offers the whole analysis at once.
    ///
    /// The distinction is not academic. The Vision engine declared
    /// `VisualEmbedding`, `Scene`, `Ocr` and `FaceDetection` but implements
    /// only `analyse_file`, so whenever Vision returned an analysis without a
    /// feature print, the pipeline asked the registry for a fallback, the
    /// registry handed back Vision again, and the direct call landed on the
    /// trait's stub. The photograph failed with "capability not supported:
    /// visual_embedding" — 717 of them on a real drive — when a perfectly good
    /// local engine was registered and able to do the work.
    fn direct_capabilities(&self) -> &[Capability] {
        self.capabilities()
    }

    fn supports_file_analysis(&self) -> bool {
        false
    }

    /// Analyse an original in a single pass, reading it directly from disk.
    ///
    /// Implementations must treat the path as strictly read-only. Working from
    /// the file rather than a decoded [`RgbImage`] lets a real model see the
    /// photograph at full fidelity instead of a re-encode, which matters for
    /// text recognition in particular.
    fn analyse_file(
        &self,
        _abs: &std::path::Path,
        _cancel: &CancelToken,
    ) -> Result<Provenanced<FileAnalysis>> {
        Err(unsupported("analyse_file"))
    }

    fn visual_embedding(&self, _img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<Embedding>> {
        Err(unsupported("visual_embedding"))
    }
    /// Embed a natural-language query into the *same* space as
    /// [`AiEngine::visual_embedding`], so `vector_search` can compare the two
    /// directly. An engine offering this capability must guarantee the two
    /// vectors share a dimension and a `model_id`/`model_version` partition;
    /// otherwise embedding spaces would silently mix.
    ///
    /// [`AiMeta::confidence`] reports how much of the query the encoder actually
    /// understood, so a caller can decline to rank by vision when the answer
    /// would be meaningless.
    fn text_embedding(&self, _text: &str, _cancel: &CancelToken) -> Result<Provenanced<Embedding>> {
        Err(unsupported("text_embedding"))
    }
    fn detect_faces(&self, _img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<Vec<FaceDetection>>> {
        Err(unsupported("detect_faces"))
    }
    fn face_embedding(&self, _img: &RgbImage, _face: &FaceDetection, _cancel: &CancelToken) -> Result<Provenanced<Embedding>> {
        Err(unsupported("face_embedding"))
    }
    fn ocr(&self, _img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<OcrResult>> {
        Err(unsupported("ocr"))
    }
    fn scene(&self, _img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<SceneResult>> {
        Err(unsupported("scene"))
    }
    fn color(&self, _img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<ColorResult>> {
        Err(unsupported("color"))
    }
    fn scan_artifact(&self, _img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<ScanArtifactResult>> {
        Err(unsupported("scan_artifact"))
    }
}

fn unsupported(what: &str) -> crate::error::Error {
    crate::error::Error::ModelMissing(format!("capability not supported: {what}"))
}

/// A plug-in registry mapping each capability to a chosen engine.
///
/// The pipeline asks the registry for the engine to use per capability, so new
/// local models drop in without touching call sites.
pub struct EngineRegistry {
    engines: Vec<Arc<dyn AiEngine>>,
    default: Arc<dyn AiEngine>,
}

impl EngineRegistry {
    /// A registry backed solely by the offline local heuristic engine.
    pub fn local_default() -> Self {
        let engine: Arc<dyn AiEngine> = Arc::new(local::LocalHeuristicEngine::new());
        Self {
            engines: vec![engine.clone()],
            default: engine,
        }
    }

    /// The default registry, plus Apple Vision when its worker is available.
    ///
    /// This is what callers should use: it gives real image understanding when
    /// the platform can provide it, and silently falls back to the deterministic
    /// heuristic engine when it cannot. Indexing never depends on Vision being
    /// present.
    pub fn local_with_vision() -> Self {
        let mut reg = Self::local_default();
        #[cfg(target_os = "macos")]
        if let Some(v) = vision::VisionEngine::detect() {
            reg.register(Arc::new(v));
        }
        reg
    }

    /// The engine that should be used for whole-file analysis, if any engine
    /// offers it (most-recently registered wins, as with capabilities).
    pub fn file_analyser(&self) -> Option<Arc<dyn AiEngine>> {
        self.engines
            .iter()
            .rev()
            .find(|e| e.supports_file_analysis())
            .cloned()
    }

    /// Register an additional engine (plugin). Later registrations take
    /// precedence for the capabilities they declare.
    pub fn register(&mut self, engine: Arc<dyn AiEngine>) {
        self.engines.push(engine);
    }

    /// The engine that should handle `cap` (most-recently registered wins).
    pub fn engine_for(&self, cap: Capability) -> Arc<dyn AiEngine> {
        // Only engines that can act on an in-memory image are eligible: this
        // call always ends in a direct method call, never in `analyse_file`.
        for e in self.engines.iter().rev() {
            if e.direct_capabilities().contains(&cap) {
                return e.clone();
            }
        }
        self.default.clone()
    }

    /// True if every registered engine can run offline (verifier uses this).
    pub fn all_offline(&self) -> bool {
        self.engines.iter().all(|e| e.is_offline())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_returns_engine_for_capability() {
        let reg = EngineRegistry::local_default();
        let e = reg.engine_for(Capability::VisualEmbedding);
        assert_eq!(e.model_id(), "local-heuristic");
        assert!(reg.all_offline());
    }

    #[test]
    fn cancel_token() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
        t.cancel();
        assert!(t.is_cancelled());
    }
}

#[cfg(test)]
mod registry_invariant_tests {
    use super::*;

    /// An engine that claims everything but can only work from a file path —
    /// the shape of the real Vision engine.
    struct FileOnlyEngine {
        caps: Vec<Capability>,
    }

    impl AiEngine for FileOnlyEngine {
        fn model_id(&self) -> &str {
            "file-only"
        }
        fn model_version(&self) -> &str {
            "1"
        }
        fn capabilities(&self) -> &[Capability] {
            &self.caps
        }
        fn direct_capabilities(&self) -> &[Capability] {
            &[]
        }
        fn supports_file_analysis(&self) -> bool {
            true
        }
    }

    /// The bug this exists to prevent, stated as an invariant: whatever the
    /// registry hands back for a capability must be able to perform it on an
    /// image in memory. Returning an engine that answers "capability not
    /// supported" fails the photograph while a working engine sits registered
    /// alongside it — which is exactly what happened to 717 photographs.
    #[test]
    fn every_engine_the_registry_offers_can_do_the_work_directly() {
        let mut reg = EngineRegistry::local_default();
        reg.register(Arc::new(FileOnlyEngine {
            caps: vec![
                Capability::VisualEmbedding,
                Capability::Scene,
                Capability::Ocr,
                Capability::FaceDetection,
            ],
        }));

        let img = image::RgbImage::from_pixel(16, 16, image::Rgb([120, 90, 60]));
        let cancel = CancelToken::new();

        // Each of these went through the stub before the fix.
        reg.engine_for(Capability::VisualEmbedding)
            .visual_embedding(&img, &cancel)
            .expect("visual_embedding must be answerable");
        reg.engine_for(Capability::Scene)
            .scene(&img, &cancel)
            .expect("scene must be answerable");
        reg.engine_for(Capability::FaceDetection)
            .detect_faces(&img, &cancel)
            .expect("detect_faces must be answerable");
    }

    /// The file-only engine must still win the whole-file path — the fix must
    /// not quietly stop AtlasDrive using Apple Vision.
    #[test]
    fn the_file_only_engine_is_still_chosen_for_whole_file_analysis() {
        let mut reg = EngineRegistry::local_default();
        reg.register(Arc::new(FileOnlyEngine { caps: vec![Capability::VisualEmbedding] }));

        let analyser = reg.file_analyser().expect("a file analyser must be found");
        assert_eq!(analyser.model_id(), "file-only");
        // And it still reports what it can produce, for diagnostics.
        assert!(analyser.capabilities().contains(&Capability::VisualEmbedding));
    }
}
