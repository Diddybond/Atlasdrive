//! Typed results and provenance metadata for AI Engine outputs.
//!
//! Every AI task records model id, model version, processing date, confidence
//! and execution time so the catalogue always knows which model generated a
//! result (see the AI Engine brief).

use serde::{Deserialize, Serialize};

/// Provenance attached to every AI result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiMeta {
    pub model_id: String,
    pub model_version: String,
    /// ISO-8601 processing timestamp.
    pub processed_at: String,
    /// 0.0–1.0 confidence for this result.
    pub confidence: f32,
    /// Wall-clock execution time in milliseconds.
    pub exec_ms: u64,
}

/// A result value paired with its provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenanced<T> {
    pub value: T,
    pub meta: AiMeta,
}

impl<T> Provenanced<T> {
    pub fn new(value: T, meta: AiMeta) -> Self {
        Self { value, meta }
    }
}

/// A dense feature vector plus the dimension for validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub dim: usize,
    pub vector: Vec<f32>,
}

impl Embedding {
    pub fn new(vector: Vec<f32>) -> Self {
        Self {
            dim: vector.len(),
            vector,
        }
    }
    /// True if all values are finite (verifier sanity check).
    pub fn is_finite(&self) -> bool {
        self.vector.iter().all(|v| v.is_finite())
    }
}

/// A detected face region in normalized [0,1] image coordinates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceDetection {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub quality: f32,
    /// Identity embedding for this face, when the analyser produced one.
    /// `None` means the caller must fall back to `AiEngine::face_embedding`.
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
}

/// OCR output. `text` may be empty; the catalogue does not log it by default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    pub text: String,
    pub confidence: f32,
}

/// One weighted concept tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Concept {
    pub tag: String,
    pub confidence: f32,
}

/// Structured scene signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneResult {
    pub indoor_prob: f32,
    pub outdoor_prob: f32,
    pub people_count: u32,
    pub description: String,
    pub concepts: Vec<Concept>,
}

/// Colour characteristics of an image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorResult {
    pub avg_r: f32,
    pub avg_g: f32,
    pub avg_b: f32,
    pub brightness: f32,
    pub saturation: f32,
    pub is_grayscale: bool,
}

/// Everything a single-pass analyser produces from one read of a file.
///
/// Real vision models do object classification, text recognition, face
/// detection and embedding in one pass over the pixels. Asking for each
/// capability separately would re-run the whole model per capability, so an
/// engine that works this way reports it via
/// [`super::AiEngine::supports_file_analysis`] and the pipeline takes this path
/// instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAnalysis {
    pub embedding: Option<Embedding>,
    pub ocr: Option<OcrResult>,
    pub faces: Vec<FaceDetection>,
    pub scene: Option<SceneResult>,
    /// Pixel dimensions as the analyser saw them.
    pub width: u32,
    pub height: u32,
}

/// Signals that an image is a scan of a physical print.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanArtifactResult {
    pub likely_scanned_print: bool,
    pub likely_photo_of_photo: bool,
    /// Uniform-border fraction detected around the edges.
    pub border_fraction: f32,
    pub fading_score: f32,
}
