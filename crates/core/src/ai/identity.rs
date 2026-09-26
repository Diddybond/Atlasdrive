//! Face identity: telling one person from another (D-102).
//!
//! Apple Vision's feature print describes what an image *looks like*, not whose
//! face it is. On the owner's archive it scored different children at the same
//! 89% as each other, so "who else is this?" could not be answered from it.
//! This module computes a proper identity embedding with a model trained for
//! exactly that question:
//!
//! 1. **Find the face's landmarks** — eyes, nose tip, mouth corners — with the
//!    SCRFD detector, run on the stored face crop.
//! 2. **Align** the face to the standard 112×112 ArcFace template with a
//!    similarity transform, so every face is presented upright and centred.
//! 3. **Embed** with ArcFace R50 (InsightFace `w600k_r50`): 512 numbers,
//!    L2-normalised, where the same person lands close together and different
//!    people far apart, whatever the lighting, age or angle.
//!
//! Everything runs in-process through `tract`, a pure-Rust ONNX engine: no
//! native runtime, no network. The model files are fetched once at build time
//! by `scripts/fetch-face-model.sh`, verified by SHA-256, and bundled.
//!
//! It works from the 200px face crops already in the catalogue, so the whole
//! archive can be re-embedded with every drive unplugged.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::RgbImage;
use tract_onnx::prelude::*;

use crate::error::{Error, Result};

/// The embedding partition this model writes under. Changing the model means
/// a new id, never reusing this one: embeddings from different models are
/// different spaces and must not be compared.
pub const MODEL_ID: &str = "arcface-r50-w600k";
pub const MODEL_VERSION: &str = "1";
pub const DIM: usize = 512;

pub const DETECTOR_FILE: &str = "det_10g.onnx";
pub const RECOGNISER_FILE: &str = "w600k_r50.onnx";
/// Where the files live, relative to a models root.
pub const MODEL_DIR: &str = "face-identity";

/// Detector input edge. A multiple of 32 (the coarsest stride); a 200px crop
/// shrinks slightly into it, and the face fills most of it.
const DET_SIZE: usize = 192;
const REC_SIZE: usize = 112;
/// Below this the detector is guessing.
const MIN_DET_SCORE: f32 = 0.35;
const STRIDES: [usize; 3] = [8, 16, 32];
const ANCHORS_PER_CELL: usize = 2;

/// Where ArcFace expects the eyes, nose tip and mouth corners in a 112×112
/// face — the alignment the recogniser was trained on.
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

type Plan = TypedRunnableModel;

/// The loaded detector and recogniser. Cheap to clone and safe to share
/// between threads: a plan is immutable and each run gets its own state.
#[derive(Clone)]
pub struct IdentityModel {
    det: Arc<Plan>,
    rec: Arc<Plan>,
}

/// Five facial landmarks in pixel coordinates of the image they were found in:
/// left eye, right eye, nose tip, left and right mouth corners.
pub type Landmarks = [[f32; 2]; 5];

impl IdentityModel {
    /// Load both models from a directory holding [`DETECTOR_FILE`] and
    /// [`RECOGNISER_FILE`].
    pub fn load(dir: &Path) -> Result<Self> {
        let plan = |file: &str, edge: usize| -> Result<Arc<Plan>> {
            let path = dir.join(file);
            if !path.is_file() {
                return Err(Error::Other(format!(
                    "face identity model not found at {} — run scripts/fetch-face-model.sh",
                    path.display()
                )));
            }
            let model = tract_onnx::onnx()
                .model_for_path(&path)
                .and_then(|m| m.with_input_fact(0, f32::fact([1, 3, edge, edge]).into()))
                .and_then(|m| m.into_optimized())
                .and_then(|m| m.into_runnable())
                .map_err(|e| Error::Other(format!("could not load {file}: {e}")))?;
            Ok(model)
        };
        Ok(Self { det: plan(DETECTOR_FILE, DET_SIZE)?, rec: plan(RECOGNISER_FILE, REC_SIZE)? })
    }

    /// Find the model directory: an explicit override, then inside the app
    /// bundle, then the application-support folder, then a development
    /// checkout. `extra` lets the app name its own resource directory first.
    pub fn find(extra: &[PathBuf]) -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(p) = std::env::var("ATLASDRIVE_FACE_MODELS") {
            candidates.push(PathBuf::from(p));
        }
        candidates.extend(extra.iter().cloned());
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                // Tauri keeps a `../models/…` resource as `Resources/_up_/models/…`.
                let resources = dir.join("../Resources");
                candidates.push(resources.join("_up_/models").join(MODEL_DIR));
                candidates.push(resources.join("models").join(MODEL_DIR));
                candidates.push(dir.join("models").join(MODEL_DIR));
            }
        }
        candidates.push(crate::config::AppPaths::default_root().join("models").join(MODEL_DIR));
        candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models").join(MODEL_DIR));
        candidates
            .into_iter()
            .find(|d| d.join(DETECTOR_FILE).is_file() && d.join(RECOGNISER_FILE).is_file())
    }

    /// Load from wherever [`find`](Self::find) finds the files.
    pub fn locate(extra: &[PathBuf]) -> Result<Self> {
        let dir = Self::find(extra).ok_or_else(|| {
            Error::Other(
                "the face identity model is not installed — rebuild the app, which fetches it".into(),
            )
        })?;
        Self::load(&dir)
    }

    /// The identity embedding of the face at the centre of a crop, or `None`
    /// when no face can be found in it confidently enough to align.
    ///
    /// Crops are cut around a detected face with a margin, so the face wanted
    /// is the one covering the middle; a neighbour at the edge of a group shot
    /// is ignored.
    pub fn embed_crop(&self, crop: &RgbImage) -> Result<Option<Vec<f32>>> {
        let Some(points) = self.landmarks(crop)? else { return Ok(None) };
        let aligned = align(crop, &points);
        self.embed_aligned(&aligned).map(Some)
    }

    /// The identity embedding of a face already aligned to 112×112.
    pub fn embed_aligned(&self, face: &RgbImage) -> Result<Vec<f32>> {
        debug_assert_eq!((face.width(), face.height()), (REC_SIZE as u32, REC_SIZE as u32));
        let input = to_tensor(face, REC_SIZE, 127.5, 127.5);
        let out = self
            .rec
            .run(tvec!(input.into()))
            .map_err(|e| Error::Other(format!("face identity model failed: {e}")))?;
        let mut v: Vec<f32> = out[0]
            .to_plain_array_view::<f32>()
            .map_err(|e| Error::Other(format!("face identity output: {e}")))?
            .iter()
            .copied()
            .collect();
        if v.len() != DIM || v.iter().any(|x| !x.is_finite()) {
            return Err(Error::Other("face identity model produced an invalid vector".into()));
        }
        normalise(&mut v);
        Ok(v)
    }

    /// Landmarks of the most central confident face in `img`.
    pub fn landmarks(&self, img: &RgbImage) -> Result<Option<Landmarks>> {
        let dets = self.detect(img)?;
        let (cx, cy) = (img.width() as f32 / 2.0, img.height() as f32 / 2.0);
        let contains_centre =
            |d: &Detection| d.bbox[0] <= cx && cx <= d.bbox[2] && d.bbox[1] <= cy && cy <= d.bbox[3];
        let best = dets
            .iter()
            .filter(|d| contains_centre(d))
            .max_by(|a, b| a.score.total_cmp(&b.score))
            .or_else(|| dets.iter().filter(|d| d.score >= 0.6).max_by(|a, b| a.score.total_cmp(&b.score)));
        Ok(best.map(|d| d.points))
    }

    /// Every face the detector finds in `img`, in `img` pixel coordinates.
    pub fn detect(&self, img: &RgbImage) -> Result<Vec<Detection>> {
        // Letterbox into DET_SIZE, keeping the aspect ratio; the rest is black.
        let scale = DET_SIZE as f32 / img.width().max(img.height()) as f32;
        let (w, h) = (
            ((img.width() as f32 * scale).round() as u32).clamp(1, DET_SIZE as u32),
            ((img.height() as f32 * scale).round() as u32).clamp(1, DET_SIZE as u32),
        );
        let resized = image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle);
        let mut canvas = RgbImage::new(DET_SIZE as u32, DET_SIZE as u32);
        image::imageops::replace(&mut canvas, &resized, 0, 0);

        let input = to_tensor(&canvas, DET_SIZE, 127.5, 128.0);
        let out = self
            .det
            .run(tvec!(input.into()))
            .map_err(|e| Error::Other(format!("face detector failed: {e}")))?;
        if out.len() != 9 {
            return Err(Error::Other(format!("face detector gave {} outputs, expected 9", out.len())));
        }

        let mut found = Vec::new();
        for (level, &stride) in STRIDES.iter().enumerate() {
            let view = |i: usize| -> Result<Vec<f32>> {
                Ok(out[i]
                    .to_plain_array_view::<f32>()
                    .map_err(|e| Error::Other(format!("face detector output: {e}")))?
                    .iter()
                    .copied()
                    .collect())
            };
            let (scores, boxes, points) = (view(level)?, view(level + 3)?, view(level + 6)?);
            let cells_total = (DET_SIZE / stride).pow(2) * ANCHORS_PER_CELL;
            if scores.len() != cells_total || boxes.len() != cells_total * 4 || points.len() != cells_total * 10 {
                return Err(Error::Other("face detector outputs have an unexpected shape".into()));
            }
            let cells = DET_SIZE / stride;
            let s = stride as f32;
            for (i, &score) in scores.iter().enumerate() {
                if score < MIN_DET_SCORE {
                    continue;
                }
                let cell = i / ANCHORS_PER_CELL;
                let (ax, ay) = ((cell % cells) as f32 * s, (cell / cells) as f32 * s);
                let b = &boxes[i * 4..i * 4 + 4];
                let k = &points[i * 10..i * 10 + 10];
                let mut pts = [[0.0f32; 2]; 5];
                for (j, p) in pts.iter_mut().enumerate() {
                    *p = [(ax + k[2 * j] * s) / scale, (ay + k[2 * j + 1] * s) / scale];
                }
                found.push(Detection {
                    score,
                    bbox: [
                        (ax - b[0] * s) / scale,
                        (ay - b[1] * s) / scale,
                        (ax + b[2] * s) / scale,
                        (ay + b[3] * s) / scale,
                    ],
                    points: pts,
                });
            }
        }
        Ok(suppress(found))
    }
}

/// The installed model, loaded once per process and shared by every scan and
/// upgrade. `None` when it is not installed, or when
/// `ATLASDRIVE_NO_FACE_IDENTITY` is set (the Vision feature print is then
/// used, as before D-102).
pub fn shared() -> Option<IdentityModel> {
    static MODEL: std::sync::OnceLock<Option<IdentityModel>> = std::sync::OnceLock::new();
    MODEL
        .get_or_init(|| {
            if std::env::var_os("ATLASDRIVE_NO_FACE_IDENTITY").is_some() {
                return None;
            }
            IdentityModel::find(&[]).and_then(|dir| IdentityModel::load(&dir).ok())
        })
        .clone()
}

/// One face found by the detector.
#[derive(Debug, Clone)]
pub struct Detection {
    pub score: f32,
    /// x1, y1, x2, y2.
    pub bbox: [f32; 4],
    pub points: Landmarks,
}

/// Keep the best of each cluster of overlapping detections.
fn suppress(mut dets: Vec<Detection>) -> Vec<Detection> {
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for d in dets {
        if kept.iter().all(|k| iou(&k.bbox, &d.bbox) < 0.4) {
            kept.push(d);
        }
    }
    kept
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let area = |r: &[f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let union = area(a) + area(b) - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// NCHW float tensor, RGB, `(pixel - mean) / std`.
fn to_tensor(img: &RgbImage, edge: usize, mean: f32, std: f32) -> Tensor {
    let arr = tract_ndarray::Array4::from_shape_fn((1, 3, edge, edge), |(_, c, y, x)| {
        (img.get_pixel(x as u32, y as u32)[c] as f32 - mean) / std
    });
    arr.into()
}

fn normalise(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

/// Cosine similarity of two embeddings (both already unit length).
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The similarity transform (rotation, uniform scale, translation) taking
/// `src` points onto `dst` in the least-squares sense, as `[a, b, tx, ty]`
/// for `x' = a·x − b·y + tx`, `y' = b·x + a·y + ty`.
///
/// This is Umeyama's estimate restricted to proper rotations — a mirror image
/// is never the right answer for a face.
pub fn similarity_transform(src: &Landmarks, dst: &Landmarks) -> [f32; 4] {
    let n = src.len() as f32;
    let mean = |p: &Landmarks| {
        let (sx, sy) = p.iter().fold((0.0, 0.0), |(x, y), q| (x + q[0], y + q[1]));
        (sx / n, sy / n)
    };
    let (msx, msy) = mean(src);
    let (mdx, mdy) = mean(dst);
    let (mut num_a, mut num_b, mut den) = (0.0f32, 0.0f32, 0.0f32);
    for (s, d) in src.iter().zip(dst) {
        let (xs, ys) = (s[0] - msx, s[1] - msy);
        let (xd, yd) = (d[0] - mdx, d[1] - mdy);
        num_a += xs * xd + ys * yd;
        num_b += xs * yd - ys * xd;
        den += xs * xs + ys * ys;
    }
    if den <= f32::EPSILON {
        return [1.0, 0.0, mdx - msx, mdy - msy];
    }
    let (a, b) = (num_a / den, num_b / den);
    [a, b, mdx - (a * msx - b * msy), mdy - (b * msx + a * msy)]
}

/// Warp the face so its landmarks sit on the ArcFace template, 112×112.
pub fn align(img: &RgbImage, points: &Landmarks) -> RgbImage {
    let [a, b, tx, ty] = similarity_transform(points, &TEMPLATE);
    let det = a * a + b * b;
    let mut out = RgbImage::new(REC_SIZE as u32, REC_SIZE as u32);
    if det <= f32::EPSILON {
        return out;
    }
    for (u, v, px) in out.enumerate_pixels_mut() {
        // Invert x' = M·x + t  →  x = M⁻¹·(x' − t), with M⁻¹ = [[a, b], [−b, a]] / det.
        let (du, dv) = (u as f32 - tx, v as f32 - ty);
        let x = (a * du + b * dv) / det;
        let y = (-b * du + a * dv) / det;
        *px = sample(img, x, y);
    }
    out
}

/// Bilinear sample; black outside the image.
fn sample(img: &RgbImage, x: f32, y: f32) -> image::Rgb<u8> {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |xi: i64, yi: i64| -> [f32; 3] {
        if xi < 0 || yi < 0 || xi >= w || yi >= h {
            [0.0; 3]
        } else {
            let p = img.get_pixel(xi as u32, yi as u32);
            [p[0] as f32, p[1] as f32, p[2] as f32]
        }
    };
    let (p00, p10, p01, p11) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    let mut out = [0u8; 3];
    for c in 0..3 {
        let top = p00[c] * (1.0 - fx) + p10[c] * fx;
        let bottom = p01[c] * (1.0 - fx) + p11[c] * fx;
        out[c] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    image::Rgb(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transform_recovers_a_known_rotation_scale_and_shift() {
        // Template rotated by 20°, scaled by 1.7 and shifted.
        let (s, t) = (1.7f32, 20f32.to_radians());
        let (a, b) = (s * t.cos(), s * t.sin());
        let moved: Landmarks = TEMPLATE.map(|[x, y]| [a * x - b * y + 13.0, b * x + a * y - 4.0]);
        let [ea, eb, etx, ety] = similarity_transform(&TEMPLATE, &moved);
        assert!((ea - a).abs() < 1e-3 && (eb - b).abs() < 1e-3, "{ea} {eb}");
        assert!((etx - 13.0).abs() < 1e-2 && (ety + 4.0).abs() < 1e-2, "{etx} {ety}");
    }

    #[test]
    fn aligning_a_face_already_on_the_template_changes_nothing() {
        let mut img = RgbImage::new(112, 112);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 2) as u8, (y * 2) as u8, 90]);
        }
        let out = align(&img, &TEMPLATE);
        for (x, y) in [(10u32, 10u32), (56, 56), (100, 30)] {
            let (a, b) = (img.get_pixel(x, y), out.get_pixel(x, y));
            for c in 0..3 {
                assert!((a[c] as i32 - b[c] as i32).abs() <= 1, "({x},{y}) {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn overlapping_detections_collapse_to_the_strongest() {
        let d = |score, x| Detection { score, bbox: [x, 0.0, x + 10.0, 10.0], points: TEMPLATE };
        let kept = suppress(vec![d(0.6, 0.0), d(0.9, 1.0), d(0.8, 50.0)]);
        let scores: Vec<f32> = kept.iter().map(|k| k.score).collect();
        assert_eq!(scores, [0.9, 0.8]);
    }

    /// Runs the real models when they are present (they are fetched at build
    /// time, not committed). Checks the embedding is a finite unit vector and
    /// that the same face, lightly altered, still matches itself strongly.
    #[test]
    fn the_real_model_embeds_a_face_when_installed() {
        let Some(dir) = IdentityModel::find(&[]) else {
            eprintln!("face identity model not installed; skipping");
            return;
        };
        let Ok(photo) = std::env::var("ATLASDRIVE_FACE_TEST_PHOTO") else {
            eprintln!("ATLASDRIVE_FACE_TEST_PHOTO not set; skipping");
            return;
        };
        let model = IdentityModel::load(&dir).unwrap();
        let img = image::open(photo).unwrap().to_rgb8();
        let dets = model.detect(&img).unwrap();
        assert!(!dets.is_empty(), "no faces found");
        let d = &dets[0];
        let v = model.embed_aligned(&align(&img, &d.points)).unwrap();
        assert_eq!(v.len(), DIM);
        let n: f32 = v.iter().map(|x| x * x).sum();
        assert!((n - 1.0).abs() < 1e-3);
        let darker = image::imageops::colorops::brighten(&img, -25);
        let w = model.embed_aligned(&align(&darker, &d.points)).unwrap();
        assert!(similarity(&v, &w) > 0.8, "{}", similarity(&v, &w));
    }
}
