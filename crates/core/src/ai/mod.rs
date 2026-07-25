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

pub mod local;
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

    fn visual_embedding(&self, _img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<Embedding>> {
        Err(unsupported("visual_embedding"))
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

    /// Register an additional engine (plugin). Later registrations take
    /// precedence for the capabilities they declare.
    pub fn register(&mut self, engine: Arc<dyn AiEngine>) {
        self.engines.push(engine);
    }

    /// The engine that should handle `cap` (most-recently registered wins).
    pub fn engine_for(&self, cap: Capability) -> Arc<dyn AiEngine> {
        for e in self.engines.iter().rev() {
            if e.capabilities().contains(&cap) {
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
