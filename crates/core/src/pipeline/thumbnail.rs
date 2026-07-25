//! Thumbnail generation and verification.
//!
//! Thumbnails are written under the app-owned `thumbnails/` directory, never to
//! the source drive. Every generated thumbnail is re-decoded to prove it opens,
//! and a checksum + dimensions are recorded so the verifier can detect silent
//! corruption (see `docs/13_TESTING_AND_VERIFIER.md`).

use std::path::{Path, PathBuf};

use image::RgbImage;

use crate::error::{Error, Result};

/// Metadata recorded for a generated thumbnail.
#[derive(Debug, Clone)]
pub struct ThumbnailInfo {
    /// Path relative to the app thumbnails/ directory.
    pub rel_path: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub checksum: String,
    pub decode_ok: bool,
}

/// Compute the sharded relative path for a file's thumbnail (`ab/<id>.png`).
pub fn rel_path_for(file_id: &str) -> String {
    let shard = &file_id.get(0..2).unwrap_or("00");
    format!("{shard}/{file_id}.png")
}

/// Generate a thumbnail for `img`, write it under `thumbnails_dir`, verify it
/// decodes, and return its recorded metadata.
pub fn generate(
    img: &RgbImage,
    thumbnails_dir: &Path,
    file_id: &str,
    max_edge: u32,
) -> Result<ThumbnailInfo> {
    let (w, h) = img.dimensions();
    let (tw, th) = fit_within(w, h, max_edge);
    let thumb = image::imageops::resize(img, tw, th, image::imageops::FilterType::Lanczos3);

    let rel = rel_path_for(file_id);
    let abs: PathBuf = thumbnails_dir.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    thumb
        .save_with_format(&abs, image::ImageFormat::Png)
        .map_err(|e| Error::Other(format!("thumbnail save failed: {e}")))?;

    // Verify by re-decoding from disk.
    let reloaded = image::open(&abs)
        .map_err(|e| Error::Other(format!("thumbnail re-decode failed: {e}")))?;
    let decode_ok = reloaded.width() == tw && reloaded.height() == th;

    let bytes = std::fs::read(&abs)?;
    let checksum = blake3::hash(&bytes).to_hex().to_string();

    Ok(ThumbnailInfo {
        rel_path: rel,
        width: tw,
        height: th,
        format: "png".into(),
        checksum,
        decode_ok,
    })
}

/// Verify an existing thumbnail file still matches recorded checksum/dimensions
/// and decodes. Used by the verifier.
pub fn verify(thumbnails_dir: &Path, info: &ThumbnailInfo) -> Result<()> {
    let abs = thumbnails_dir.join(&info.rel_path);
    if !abs.exists() {
        return Err(Error::VerifierFailure(format!(
            "thumbnail missing: {}",
            info.rel_path
        )));
    }
    let bytes = std::fs::read(&abs)?;
    let checksum = blake3::hash(&bytes).to_hex().to_string();
    if checksum != info.checksum {
        return Err(Error::VerifierFailure(format!(
            "thumbnail checksum mismatch: {}",
            info.rel_path
        )));
    }
    let decoded = image::open(&abs)
        .map_err(|e| Error::VerifierFailure(format!("thumbnail cannot decode {}: {e}", info.rel_path)))?;
    if decoded.width() != info.width || decoded.height() != info.height {
        return Err(Error::VerifierFailure(format!(
            "thumbnail dimensions mismatch: {}",
            info.rel_path
        )));
    }
    Ok(())
}

fn fit_within(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    if w <= max_edge && h <= max_edge {
        return (w.max(1), h.max(1));
    }
    let scale = max_edge as f32 / w.max(h) as f32;
    (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn generate_and_verify() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbImage::from_pixel(1000, 800, Rgb([90, 90, 90]));
        let info = generate(&img, dir.path(), "abcd1234", 256).unwrap();
        assert!(info.decode_ok);
        assert!(info.width <= 256 && info.height <= 256);
        assert_eq!(info.rel_path, "ab/abcd1234.png");
        verify(dir.path(), &info).unwrap();
    }

    #[test]
    fn corrupted_thumbnail_fails_verify() {
        let dir = tempfile::tempdir().unwrap();
        let img = RgbImage::from_pixel(100, 100, Rgb([1, 2, 3]));
        let info = generate(&img, dir.path(), "ffee0001", 64).unwrap();
        // Corrupt the file.
        let abs = dir.path().join(&info.rel_path);
        std::fs::write(&abs, b"not a png").unwrap();
        assert!(verify(dir.path(), &info).is_err());
    }
}
