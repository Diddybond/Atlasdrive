//! Default offline AI engine: fast, deterministic image heuristics.
//!
//! This is a real, dependency-light backend that runs entirely in-process with
//! no network. It is intentionally a *stand-in* for heavier local models
//! (CoreML / ONNX vision + face networks) which plug in later via
//! [`crate::ai::EngineRegistry`] without changing any database contract or call
//! site. Determinism matters: identical input always yields identical output,
//! which keeps the verifier's face-sanity and embedding checks meaningful and
//! makes re-analysis with a newer model a clean, versioned operation.

use image::{imageops::FilterType, RgbImage};

use super::text;
use super::types::*;
use super::{AiEngine, CancelToken, Capability};
use crate::error::{Error, Result};
use crate::util::now_iso8601;

pub const MODEL_ID: &str = "local-heuristic";
/// `0.2.0` added the brightness-anchor dimension below. Embeddings are
/// partitioned by `(model_id, model_version)`, so vectors written by `0.1.0`
/// are simply a separate partition and are never compared against these.
pub const MODEL_VERSION: &str = "0.2.0";

/// Fixed embedding dimension: a 4×4 grid × (R,G,B,brightness), plus one
/// constant anchor dimension.
///
/// The anchor exists because L2 normalisation otherwise discards absolute
/// brightness entirely — without it a near-black frame and a near-white frame
/// of the same hue normalise to the *same* direction, so "night" and "snow"
/// are indistinguishable. Holding one dimension constant means the remaining
/// dimensions shrink or grow relative to it, and overall lightness survives
/// normalisation.
pub const EMBED_DIM: usize = 4 * 4 * 4 + 1;

/// Value of the constant anchor dimension (see [`EMBED_DIM`]).
const BRIGHTNESS_ANCHOR: f32 = 1.0;
const FACE_EMBED_DIM: usize = 32;

pub struct LocalHeuristicEngine {
    caps: Vec<Capability>,
}

impl LocalHeuristicEngine {
    pub fn new() -> Self {
        Self {
            caps: vec![
                Capability::VisualEmbedding,
                Capability::TextEmbedding,
                Capability::FaceDetection,
                Capability::FaceEmbedding,
                Capability::Ocr,
                Capability::Scene,
                Capability::Color,
                Capability::ScanArtifact,
            ],
        }
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

impl Default for LocalHeuristicEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Monotonic-ish elapsed millis without pulling in extra deps.
fn timed<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let start = std::time::Instant::now();
    let out = f();
    (out, start.elapsed().as_millis() as u64)
}

impl AiEngine for LocalHeuristicEngine {
    fn model_id(&self) -> &str {
        MODEL_ID
    }
    fn model_version(&self) -> &str {
        MODEL_VERSION
    }
    fn capabilities(&self) -> &[Capability] {
        &self.caps
    }

    fn visual_embedding(&self, img: &RgbImage, cancel: &CancelToken) -> Result<Provenanced<Embedding>> {
        if cancel.is_cancelled() {
            return Err(Error::Other("cancelled".into()));
        }
        let (vector, ms) = timed(|| embed_image(img));
        let emb = Embedding::new(vector);
        Ok(Provenanced::new(emb, self.meta(0.5, ms)))
    }

    fn text_embedding(&self, query: &str, cancel: &CancelToken) -> Result<Provenanced<Embedding>> {
        if cancel.is_cancelled() {
            return Err(Error::Other("cancelled".into()));
        }
        // The query is rendered into the same 4x4 conceptual frame the image
        // encoder reduces every photograph to, then embedded by the *identical*
        // function — so query and image vectors cannot drift into separate
        // spaces. Confidence carries the lexicon's honest coverage of the query.
        let (rendered, ms) = timed(|| text::render_query(query));
        let vector = embed_grid(&rendered.grid);
        Ok(Provenanced::new(
            Embedding::new(vector),
            self.meta(rendered.coverage, ms),
        ))
    }

    fn detect_faces(&self, img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<Vec<FaceDetection>>> {
        let (faces, ms) = timed(|| detect_skin_blobs(img));
        let conf = if faces.is_empty() { 1.0 } else { 0.6 };
        Ok(Provenanced::new(faces, self.meta(conf, ms)))
    }

    fn face_embedding(&self, img: &RgbImage, face: &FaceDetection, _cancel: &CancelToken) -> Result<Provenanced<Embedding>> {
        let (vector, ms) = timed(|| embed_face_region(img, face));
        Ok(Provenanced::new(Embedding::new(vector), self.meta(0.5, ms)))
    }

    fn ocr(&self, _img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<OcrResult>> {
        // The heuristic backend does not perform true OCR. It returns an empty
        // low-confidence result; a real OCR model plugs in via the registry.
        Ok(Provenanced::new(
            OcrResult { text: String::new(), confidence: 0.0 },
            self.meta(0.0, 0),
        ))
    }

    fn scene(&self, img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<SceneResult>> {
        let (result, ms) = timed(|| scene_from_image(img));
        Ok(Provenanced::new(result, self.meta(0.4, ms)))
    }

    fn color(&self, img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<ColorResult>> {
        let (result, ms) = timed(|| color_stats(img));
        Ok(Provenanced::new(result, self.meta(0.9, ms)))
    }

    fn scan_artifact(&self, img: &RgbImage, _cancel: &CancelToken) -> Result<Provenanced<ScanArtifactResult>> {
        let (result, ms) = timed(|| scan_artifacts(img));
        Ok(Provenanced::new(result, self.meta(0.4, ms)))
    }
}

// ---------------------------------------------------------------------------
// Feature helpers (all deterministic)
// ---------------------------------------------------------------------------

/// Downscale to a fixed grid and build a normalized feature vector combining a
/// coarse spatial-color map. Similar images produce similar vectors.
fn embed_image(img: &RgbImage) -> Vec<f32> {
    // 4x4 grid × (R,G,B,brightness) = 64 dims.
    let small = image::imageops::resize(img, text::GRID, text::GRID, FilterType::Triangle);
    embed_grid(&small)
}

/// Embed an already-reduced 4×4 grid. This is the single definition of the
/// embedding space: both photographs (via [`embed_image`]) and natural-language
/// queries (via [`text::render_query`]) pass through here, which is what makes
/// text and image vectors directly comparable.
fn embed_grid(small: &RgbImage) -> Vec<f32> {
    debug_assert_eq!(small.dimensions(), (text::GRID, text::GRID));
    let mut v = Vec::with_capacity(EMBED_DIM);
    for p in small.pixels() {
        let r = p[0] as f32 / 255.0;
        let g = p[1] as f32 / 255.0;
        let b = p[2] as f32 / 255.0;
        let brightness = (r + g + b) / 3.0;
        v.push(r);
        v.push(g);
        v.push(b);
        v.push(brightness);
    }
    // Constant anchor: keeps absolute lightness meaningful after normalisation.
    v.push(BRIGHTNESS_ANCHOR);
    l2_normalize(&mut v);
    v
}

fn embed_face_region(img: &RgbImage, face: &FaceDetection) -> Vec<f32> {
    let (w, h) = img.dimensions();
    let x0 = ((face.x.clamp(0.0, 1.0)) * w as f32) as u32;
    let y0 = ((face.y.clamp(0.0, 1.0)) * h as f32) as u32;
    let fw = ((face.w.clamp(0.0, 1.0)) * w as f32).max(1.0) as u32;
    let fh = ((face.h.clamp(0.0, 1.0)) * h as f32).max(1.0) as u32;
    let x1 = (x0 + fw).min(w);
    let y1 = (y0 + fh).min(h);
    let mut crop = RgbImage::new((x1 - x0).max(1), (y1 - y0).max(1));
    for (cx, x) in (x0..x1).enumerate() {
        for (cy, y) in (y0..y1).enumerate() {
            crop.put_pixel(cx as u32, cy as u32, *img.get_pixel(x, y));
        }
    }
    let small = image::imageops::resize(&crop, 4, 2, FilterType::Triangle);
    let mut v = Vec::with_capacity(FACE_EMBED_DIM);
    for p in small.pixels() {
        v.push(p[0] as f32 / 255.0);
        v.push(p[1] as f32 / 255.0);
        v.push(p[2] as f32 / 255.0);
        v.push(((p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0) / 255.0);
    }
    l2_normalize(&mut v);
    v
}

fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// True if a pixel looks skin-toned (broad heuristic in RGB space).
fn is_skin(r: u8, g: u8, b: u8) -> bool {
    let (rf, gf, bf) = (r as i32, g as i32, b as i32);
    r > 95
        && g > 40
        && b > 20
        && rf > gf
        && rf > bf
        && (rf - gf) > 15
        && (rf.max(gf).max(bf) - rf.min(gf).min(bf)) > 15
}

/// Deterministically find up to a few skin-tone blobs and report them as face
/// detections. A synthetic fixture with a skin-tone rectangle is detected; a
/// grayscale document is not. A real face model replaces this later.
fn detect_skin_blobs(img: &RgbImage) -> Vec<FaceDetection> {
    let grid = 16u32;
    let (w, h) = img.dimensions();
    if w < grid || h < grid {
        return Vec::new();
    }
    let cw = w / grid;
    let ch = h / grid;
    // skin mask per cell
    let mut mask = vec![false; (grid * grid) as usize];
    for gy in 0..grid {
        for gx in 0..grid {
            let mut skin = 0u32;
            let mut total = 0u32;
            for yy in 0..ch {
                for xx in 0..cw {
                    let px = img.get_pixel(gx * cw + xx, gy * ch + yy);
                    if is_skin(px[0], px[1], px[2]) {
                        skin += 1;
                    }
                    total += 1;
                }
            }
            if total > 0 && (skin as f32 / total as f32) > 0.5 {
                mask[(gy * grid + gx) as usize] = true;
            }
        }
    }
    // Connected-component labelling on the coarse mask.
    let mut visited = vec![false; mask.len()];
    let mut faces = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || visited[start] {
            continue;
        }
        let mut stack = vec![start];
        let (mut min_gx, mut min_gy, mut max_gx, mut max_gy) = (grid, grid, 0u32, 0u32);
        let mut count = 0u32;
        while let Some(idx) = stack.pop() {
            if visited[idx] || !mask[idx] {
                continue;
            }
            visited[idx] = true;
            count += 1;
            let gx = (idx as u32) % grid;
            let gy = (idx as u32) / grid;
            min_gx = min_gx.min(gx);
            min_gy = min_gy.min(gy);
            max_gx = max_gx.max(gx);
            max_gy = max_gy.max(gy);
            let neigh = [
                (gx.wrapping_sub(1), gy),
                (gx + 1, gy),
                (gx, gy.wrapping_sub(1)),
                (gx, gy + 1),
            ];
            for (nx, ny) in neigh {
                if nx < grid && ny < grid {
                    stack.push((ny * grid + nx) as usize);
                }
            }
        }
        // Require a plausible minimum size to avoid noise.
        if count >= 2 {
            let x = min_gx as f32 / grid as f32;
            let y = min_gy as f32 / grid as f32;
            let fw = (max_gx - min_gx + 1) as f32 / grid as f32;
            let fh = (max_gy - min_gy + 1) as f32 / grid as f32;
            let quality = (count as f32 / (grid * grid) as f32).min(1.0);
            faces.push(FaceDetection { x, y, w: fw, h: fh, quality, embedding: None });
        }
    }
    // Deterministic ordering: top-to-bottom, left-to-right.
    faces.sort_by(|a, b| {
        a.y.partial_cmp(&b.y)
            .unwrap()
            .then(a.x.partial_cmp(&b.x).unwrap())
    });
    faces
}

fn color_stats(img: &RgbImage) -> ColorResult {
    let small = image::imageops::resize(img, 32, 32, FilterType::Triangle);
    let mut sr = 0f64;
    let mut sg = 0f64;
    let mut sb = 0f64;
    let mut sat = 0f64;
    let mut gray_pixels = 0u32;
    let n = (small.width() * small.height()) as f64;
    for p in small.pixels() {
        let r = p[0] as f64;
        let g = p[1] as f64;
        let b = p[2] as f64;
        sr += r;
        sg += g;
        sb += b;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let s = if max > 0.0 { (max - min) / max } else { 0.0 };
        sat += s;
        if (max - min) < 12.0 {
            gray_pixels += 1;
        }
    }
    let brightness = ((sr + sg + sb) / (3.0 * n)) / 255.0;
    ColorResult {
        avg_r: (sr / n) as f32,
        avg_g: (sg / n) as f32,
        avg_b: (sb / n) as f32,
        brightness: brightness as f32,
        saturation: (sat / n) as f32,
        is_grayscale: (gray_pixels as f64 / n) > 0.95,
    }
}

fn scene_from_image(img: &RgbImage) -> SceneResult {
    let c = color_stats(img);
    // Brighter, bluer-topped images lean outdoor; warm/dim lean indoor.
    let outdoor = ((c.brightness - 0.35) * 1.5 + (c.avg_b - c.avg_r) / 255.0).clamp(0.0, 1.0);
    let indoor = (1.0 - outdoor).clamp(0.0, 1.0);
    let mut concepts = Vec::new();
    if c.is_grayscale {
        concepts.push(Concept { tag: "black-and-white".into(), confidence: 0.8 });
    }
    if c.brightness > 0.6 {
        concepts.push(Concept { tag: "bright".into(), confidence: 0.6 });
    } else if c.brightness < 0.3 {
        concepts.push(Concept { tag: "dark".into(), confidence: 0.6 });
    }
    if c.saturation > 0.4 {
        concepts.push(Concept { tag: "colorful".into(), confidence: 0.5 });
    }
    let description = if c.is_grayscale {
        "black and white photograph".to_string()
    } else if outdoor > indoor {
        "likely outdoor scene".to_string()
    } else {
        "likely indoor scene".to_string()
    };
    let faces = detect_skin_blobs(img);
    SceneResult {
        indoor_prob: indoor,
        outdoor_prob: outdoor,
        people_count: faces.len() as u32,
        description,
        concepts,
    }
}

fn scan_artifacts(img: &RgbImage) -> ScanArtifactResult {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return ScanArtifactResult {
            likely_scanned_print: false,
            likely_photo_of_photo: false,
            border_fraction: 0.0,
            fading_score: 0.0,
        };
    }
    // Sample a 3% border band; a near-uniform bright/white border suggests a
    // scanned print laid on a scanner bed.
    let bw = (w as f32 * 0.03).max(1.0) as u32;
    let bh = (h as f32 * 0.03).max(1.0) as u32;
    let mut border_bright = 0u32;
    let mut border_total = 0u32;
    let mut check = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        let lum = (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3;
        if lum > 200 {
            border_bright += 1;
        }
        border_total += 1;
    };
    for x in 0..w {
        for y in 0..bh {
            check(x, y);
            check(x, h - 1 - y);
        }
    }
    for y in 0..h {
        for x in 0..bw {
            check(x, y);
            check(w - 1 - x, y);
        }
    }
    let border_fraction = if border_total > 0 {
        border_bright as f32 / border_total as f32
    } else {
        0.0
    };
    let c = color_stats(img);
    // Faded prints lose saturation. Combine with a strong uniform border.
    let fading_score = (1.0 - c.saturation).clamp(0.0, 1.0);
    let likely_scanned_print = border_fraction > 0.6 && c.saturation < 0.5;
    ScanArtifactResult {
        likely_scanned_print,
        likely_photo_of_photo: false,
        border_fraction,
        fading_score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn solid(w: u32, h: u32, color: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb(color))
    }

    #[test]
    fn embedding_is_deterministic_and_finite() {
        let img = solid(64, 64, [120, 130, 140]);
        let e = LocalHeuristicEngine::new();
        let c = CancelToken::new();
        let a = e.visual_embedding(&img, &c).unwrap();
        let b = e.visual_embedding(&img, &c).unwrap();
        assert_eq!(a.value.dim, EMBED_DIM);
        assert!(a.value.is_finite());
        assert_eq!(a.value.vector, b.value.vector);
        assert_eq!(a.meta.model_id, MODEL_ID);
    }

    #[test]
    fn similar_images_closer_than_different() {
        let e = LocalHeuristicEngine::new();
        let c = CancelToken::new();
        let red = solid(32, 32, [200, 20, 20]);
        let red2 = solid(32, 32, [210, 25, 25]);
        let blue = solid(32, 32, [20, 20, 200]);
        let vr = e.visual_embedding(&red, &c).unwrap().value.vector;
        let vr2 = e.visual_embedding(&red2, &c).unwrap().value.vector;
        let vb = e.visual_embedding(&blue, &c).unwrap().value.vector;
        let sim_close = crate::util::cosine_similarity(&vr, &vr2);
        let sim_far = crate::util::cosine_similarity(&vr, &vb);
        assert!(sim_close > sim_far);
    }

    #[test]
    fn text_embedding_shares_the_image_embedding_space() {
        let e = LocalHeuristicEngine::new();
        let c = CancelToken::new();
        let img = solid(64, 64, [120, 130, 140]);
        let image_vec = e.visual_embedding(&img, &c).unwrap();
        let text_vec = e.text_embedding("beach", &c).unwrap();
        // Same dimension, same model partition — otherwise vector_search would
        // be comparing vectors from two different spaces.
        assert_eq!(text_vec.value.dim, image_vec.value.dim);
        assert_eq!(text_vec.value.dim, EMBED_DIM);
        assert_eq!(text_vec.meta.model_id, image_vec.meta.model_id);
        assert_eq!(text_vec.meta.model_version, image_vec.meta.model_version);
        assert!(text_vec.value.is_finite());
    }

    #[test]
    fn text_query_ranks_the_matching_photograph_first() {
        let e = LocalHeuristicEngine::new();
        let c = CancelToken::new();
        // A "snow" query should sit closer to a bright white frame than to a
        // dark one — the encoder has to move in the right direction, not just
        // produce a well-formed vector.
        let snowy = solid(32, 32, [235, 240, 248]);
        let night = solid(32, 32, [20, 22, 35]);
        let q = e.text_embedding("snow", &c).unwrap().value.vector;
        let sim_snow = crate::util::cosine_similarity(&q, &e.visual_embedding(&snowy, &c).unwrap().value.vector);
        let sim_night = crate::util::cosine_similarity(&q, &e.visual_embedding(&night, &c).unwrap().value.vector);
        assert!(sim_snow > sim_night, "snow {sim_snow} should beat night {sim_night}");

        // And the reverse query must flip the ordering.
        let q2 = e.text_embedding("night", &c).unwrap().value.vector;
        let sim_snow2 = crate::util::cosine_similarity(&q2, &e.visual_embedding(&snowy, &c).unwrap().value.vector);
        let sim_night2 = crate::util::cosine_similarity(&q2, &e.visual_embedding(&night, &c).unwrap().value.vector);
        assert!(sim_night2 > sim_snow2, "night {sim_night2} should beat snow {sim_snow2}");
    }

    #[test]
    fn unrecognised_query_reports_zero_confidence() {
        let e = LocalHeuristicEngine::new();
        let c = CancelToken::new();
        let r = e.text_embedding("zzzz qqqq", &c).unwrap();
        // Callers use this to fall back to text search rather than present a
        // meaningless visual ranking.
        assert_eq!(r.meta.confidence, 0.0);
    }

    #[test]
    fn detects_skin_blob_but_not_document() {
        let e = LocalHeuristicEngine::new();
        let c = CancelToken::new();
        // Skin-tone square on dark background.
        let mut img = solid(64, 64, [10, 10, 10]);
        for y in 20..44 {
            for x in 20..44 {
                img.put_pixel(x, y, Rgb([200, 150, 120]));
            }
        }
        let faces = e.detect_faces(&img, &c).unwrap();
        assert!(!faces.value.is_empty(), "should detect a skin blob");

        // A white/grey document has no skin.
        let doc = solid(64, 64, [235, 235, 235]);
        let none = e.detect_faces(&doc, &c).unwrap();
        assert!(none.value.is_empty(), "document should have no faces");
    }

    #[test]
    fn grayscale_and_scan_detection() {
        let e = LocalHeuristicEngine::new();
        let c = CancelToken::new();
        let gray = solid(64, 64, [128, 128, 128]);
        let col = e.color(&gray, &c).unwrap();
        assert!(col.value.is_grayscale);

        // Faded photo with a bright uniform border.
        let mut img = solid(100, 100, [180, 175, 170]);
        for x in 0..100 {
            for y in 0..6 {
                img.put_pixel(x, y, Rgb([250, 250, 250]));
                img.put_pixel(x, 99 - y, Rgb([250, 250, 250]));
            }
        }
        for y in 0..100 {
            for x in 0..6 {
                img.put_pixel(x, y, Rgb([250, 250, 250]));
                img.put_pixel(99 - x, y, Rgb([250, 250, 250]));
            }
        }
        let sa = e.scan_artifact(&img, &c).unwrap();
        assert!(sa.value.border_fraction > 0.0);
    }
}
