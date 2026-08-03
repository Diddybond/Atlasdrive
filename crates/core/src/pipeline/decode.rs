//! Reading an original into pixels, including HEIC/HEIF on macOS.
//!
//! The `image` crate cannot decode HEIC, and pulling in `libheif` would add a
//! C dependency and a licence to every build. Every Mac already ships a HEIC
//! decoder, so on macOS we hand the file to `sips` — the system image tool that
//! sits on top of ImageIO, the same decoder Preview and Photos use.
//!
//! Two properties matter and are enforced here:
//!
//!   * **The original is never written.** `sips` is invoked with an explicit
//!     `--out` pointing into our own cache directory, so it converts *to a copy*
//!     and never rewrites in place. The original is only read.
//!   * **No network.** `sips` is a local system binary. Indexing stays offline.
//!
//! On non-macOS builds HEIC simply reports as unsupported, which the pipeline
//! already treats as a recoverable per-file failure rather than a fatal one.

use std::path::Path;

use image::RgbImage;

use crate::error::{Error, Result};

/// Extensions that need the system decoder rather than the `image` crate.
///
/// PSD is here for the same reason as HEIC: the `image` crate cannot read it,
/// but ImageIO can, and it flattens the composite exactly as Photoshop and
/// Bridge display it. Verified on a real 276MB layered PSD.
///
/// RAW formats are listed so that a scan which *opts in* to them can decode
/// them too — ImageIO reads all of these.
const SYSTEM_DECODE_EXTENSIONS: &[&str] =
    &["heic", "heif", "psd", "arw", "cr2", "cr3", "nef", "dng", "raf", "rw2", "orf"];

/// True when `path` needs the platform decoder.
pub fn needs_system_decoder(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SYSTEM_DECODE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Decode an original into RGB pixels.
///
/// `scratch_dir` is an app-owned directory used only for the intermediate copy
/// when the system decoder is needed; it is cleaned up before returning.
pub fn open_rgb(abs: &Path, scratch_dir: &Path) -> Result<RgbImage> {
    if needs_system_decoder(abs) {
        return open_via_system_decoder(abs, scratch_dir);
    }

    // A photograph far larger than any camera produces is decoded at reduced
    // size, because nothing downstream needs the full grid.
    //
    // The pixels are used for a thumbnail (a few hundred pixels), a perceptual
    // fingerprint (32x32), colour statistics and scanned-print detection. Apple
    // Vision is handed the original *path* and reads the file itself, so what
    // the catalogue knows about the picture is unaffected.
    //
    // Without this, one shoot on a real drive brought a scan to a visible
    // standstill: single TIFFs of 2.6GB — over four hundred megapixels —
    // decoding to gigabytes in memory and then walked several times over. The
    // scan was working the whole time; from the outside it was indistinguishable
    // from frozen, and a handful of those files can hold up a night.
    if let Some((w, h)) = probe_dimensions(abs) {
        if u64::from(w) * u64::from(h) > MAX_FULL_DECODE_PIXELS {
            if let Ok(img) = open_downsampled(abs, scratch_dir) {
                return Ok(img);
            }
            // Falling through is deliberate: a slow decode beats no photograph.
        }
    }

    match open_with_generous_limits(abs) {
        Ok(img) => Ok(img),
        Err(first) => {
            // Anything the Rust decoder cannot manage is handed to ImageIO
            // before being called a failure.
            //
            // This is not hypothetical. A real archive scan failed 221
            // photographs with "Memory limit exceeded", every one of them a
            // flattened 16-bit TIFF — 7360 x 4912 at 16 bits is about 207MB
            // once decoded, past what the `image` crate will allocate by
            // default. macOS reads them without complaint, and the file is a
            // perfectly good photograph. Treating a decoder's limitation as the
            // photograph's fault leaves silent holes in the catalogue.
            match open_via_system_decoder(abs, scratch_dir) {
                Ok(img) => Ok(img),
                // Report the original failure: it describes the format problem,
                // whereas the fallback's error is usually just "sips failed".
                Err(_) => Err(first),
            }
        }
    }
}

/// Above this, decode a reduced copy rather than the full grid.
///
/// Generously above any camera: a 100-megapixel medium-format back still takes
/// the direct path. Only scanned composites and stitched panoramas — the files
/// that measure in gigabytes — go the other way.
const MAX_FULL_DECODE_PIXELS: u64 = 120_000_000;

/// Longest edge of the reduced copy.
///
/// Comfortably more than every downstream use needs, so the reduction can never
/// cost detail that ends up in the catalogue.
const DOWNSAMPLE_EDGE: u32 = 4096;

/// Read an image's dimensions without decoding it — a header read only.
fn probe_dimensions(abs: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(abs)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Decode a reduced copy through ImageIO, which resizes as it reads.
///
/// The original is only ever read: `sips` writes to a path inside the app's own
/// scratch directory, and that copy is deleted before returning.
#[cfg(target_os = "macos")]
fn open_downsampled(abs: &Path, scratch_dir: &Path) -> Result<RgbImage> {
    use std::process::Command;

    std::fs::create_dir_all(scratch_dir)?;
    let out = scratch_dir.join(format!("large-{}.jpg", crate::util::new_uuid()));

    let result = Command::new("/usr/bin/sips")
        .args(["-Z", &DOWNSAMPLE_EDGE.to_string()])
        .args(["-s", "format", "jpeg"])
        .arg(abs)
        .arg("--out")
        .arg(&out)
        .output();

    let decoded = (|| -> Result<RgbImage> {
        let output = result.map_err(|e| Error::Other(format!("sips could not run: {e}")))?;
        if !output.status.success() {
            return Err(Error::Other(format!(
                "reduced decode failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(image::open(&out)
            .map_err(|e| Error::Other(format!("reduced copy failed to decode: {e}")))?
            .to_rgb8())
    })();

    let _ = std::fs::remove_file(&out);
    decoded
}

#[cfg(not(target_os = "macos"))]
fn open_downsampled(abs: &Path, _scratch_dir: &Path) -> Result<RgbImage> {
    open_with_generous_limits(abs)
}

/// Decode with limits sized for photographs rather than for untrusted input.
///
/// The crate's defaults are chosen to make a malicious image safe to open. Every
/// file here came off the owner's own drive and has already been walked,
/// stat-ed and hashed, so the threat model is different — and a professional
/// archive is full of images that exceed those defaults honestly.
fn open_with_generous_limits(abs: &Path) -> Result<RgbImage> {
    let mut limits = image::Limits::default();
    // 1GB decoded: comfortably above a 16-bit 50-megapixel frame, and still a
    // bound rather than no bound at all.
    limits.max_alloc = Some(1024 * 1024 * 1024);
    limits.max_image_width = None;
    limits.max_image_height = None;

    let mut reader = image::ImageReader::open(abs)
        .map_err(|e| Error::Other(format!("decode failed: {e}")))?
        .with_guessed_format()
        .map_err(|e| Error::Other(format!("decode failed: {e}")))?;
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|e| Error::Other(format!("decode failed: {e}")))?;
    Ok(decoded.to_rgb8())
}

#[cfg(target_os = "macos")]
fn open_via_system_decoder(abs: &Path, scratch_dir: &Path) -> Result<RgbImage> {
    use std::process::Command;

    std::fs::create_dir_all(scratch_dir)?;
    // A unique name so concurrent workers cannot collide.
    let out = scratch_dir.join(format!("heic-{}.png", crate::util::new_uuid()));

    let result = Command::new("/usr/bin/sips")
        .arg("-s")
        .arg("format")
        .arg("png")
        .arg(abs)
        // Writing to our own path is what keeps this a read of the original.
        .arg("--out")
        .arg(&out)
        .output();

    let decoded = (|| -> Result<RgbImage> {
        let output = result.map_err(|e| Error::Other(format!("sips could not run: {e}")))?;
        if !output.status.success() {
            return Err(Error::Other(format!(
                "system HEIC decode failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let img = image::open(&out)
            .map_err(|e| Error::Other(format!("converted HEIC failed to decode: {e}")))?;
        Ok(img.to_rgb8())
    })();

    // The intermediate is never part of the catalogue; drop it either way.
    let _ = std::fs::remove_file(&out);
    decoded
}

#[cfg(not(target_os = "macos"))]
fn open_via_system_decoder(abs: &Path, _scratch_dir: &Path) -> Result<RgbImage> {
    Err(Error::Other(format!(
        "HEIC/HEIF decoding requires macOS; cannot read {}",
        abs.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_which_files_need_the_system_decoder() {
        assert!(needs_system_decoder(Path::new("/x/IMG_0001.heic")));
        assert!(needs_system_decoder(Path::new("/x/IMG_0001.HEIC")));
        assert!(needs_system_decoder(Path::new("/x/live.heif")));
        assert!(!needs_system_decoder(Path::new("/x/a.jpg")));
        assert!(!needs_system_decoder(Path::new("/x/a.png")));
        assert!(!needs_system_decoder(Path::new("/x/noext")));
    }

    #[test]
    fn ordinary_formats_still_go_through_the_image_crate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("solid.png");
        image::RgbImage::from_pixel(8, 8, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        let img = open_rgb(&path, &dir.path().join("scratch")).unwrap();
        assert_eq!(img.dimensions(), (8, 8));
        assert_eq!(img.get_pixel(0, 0).0, [10, 20, 30]);
    }

    /// A real HEIC round-trip through the macOS system decoder. `sips` is used
    /// to *create* the fixture too, so the test proves the platform pipeline
    /// end to end rather than asserting against a checked-in blob.
    #[cfg(target_os = "macos")]
    #[test]
    fn decodes_a_real_heic_via_the_system_pipeline() {
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("src.png");
        image::RgbImage::from_pixel(32, 24, image::Rgb([200, 40, 40]))
            .save(&png)
            .unwrap();

        let heic = dir.path().join("photo.heic");
        let made = Command::new("/usr/bin/sips")
            .args(["-s", "format", "heic"])
            .arg(&png)
            .arg("--out")
            .arg(&heic)
            .output()
            .expect("sips must exist on macOS");
        assert!(made.status.success(), "could not build a HEIC fixture");

        let before = std::fs::metadata(&heic).unwrap();
        let img = open_rgb(&heic, &dir.path().join("scratch")).unwrap();
        assert_eq!(img.dimensions(), (32, 24));
        // Colour survives the round trip (HEIC is lossy, so allow drift).
        let px = img.get_pixel(16, 12).0;
        assert!(px[0] > 150 && px[1] < 100, "expected a red-ish pixel, got {px:?}");

        // The original HEIC must not have been touched.
        let after = std::fs::metadata(&heic).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());

        // And no intermediate was left behind.
        let scratch = dir.path().join("scratch");
        let leftovers: Vec<_> = std::fs::read_dir(&scratch)
            .map(|d| d.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "scratch dir should be empty: {leftovers:?}");
    }
}

#[cfg(test)]
mod large_image_tests {
    use super::*;

    /// A 16-bit TIFF larger than the decoder's default allocation limit.
    ///
    /// This is the exact shape that failed 221 photographs on a real archive:
    /// 7360 x 4912 at 16 bits is roughly 207MB decoded, and the `image` crate
    /// refuses that by default. The fixture is built with `sips` so the test
    /// proves the real pipeline rather than asserting against a checked-in blob.
    #[cfg(target_os = "macos")]
    #[test]
    fn decodes_a_tiff_past_the_default_memory_limit() {
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        // Big enough to exceed the crate's default allocation, small enough
        // that building it does not dominate the suite.
        let (w, h) = (4200u32, 3200u32);
        let png = dir.path().join("src.png");
        image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
        })
        .save(&png)
        .unwrap();

        let tiff = dir.path().join("big.tif");
        let made = Command::new("/usr/bin/sips")
            .args(["-s", "format", "tiff"])
            .arg(&png)
            .arg("--out")
            .arg(&tiff)
            .output()
            .expect("sips must exist on macOS");
        assert!(made.status.success(), "could not build a TIFF fixture");

        // The default limits are what rejected these photographs.
        let mut strict = image::ImageReader::open(&tiff).unwrap();
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(16 * 1024 * 1024);
        strict.limits(limits);
        assert!(
            strict.decode().is_err(),
            "fixture must be large enough to trip a tight limit, or it proves nothing"
        );

        // The pipeline reads it.
        let img = open_rgb(&tiff, &dir.path().join("scratch")).unwrap();
        assert_eq!(img.dimensions(), (w, h));

        // And the original is untouched, as with every other read.
        let before = std::fs::metadata(&tiff).unwrap();
        open_rgb(&tiff, &dir.path().join("scratch")).unwrap();
        let after = std::fs::metadata(&tiff).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }

    /// A file that is genuinely not an image must still fail, and say so —
    /// the fallback must not turn every error into a silent success.
    #[test]
    fn a_file_that_is_not_an_image_still_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.jpg");
        std::fs::write(&path, b"this is plainly not a photograph").unwrap();
        let err = open_rgb(&path, &dir.path().join("scratch")).unwrap_err().to_string();
        assert!(err.contains("decode failed"), "unhelpful error: {err}");
    }
}

#[cfg(test)]
mod downsample_tests {
    use super::*;

    /// Dimensions must come from the header alone: the whole point is to know
    /// a file is enormous *before* paying to decode it.
    #[test]
    fn dimensions_are_read_without_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wide.png");
        image::RgbImage::from_pixel(1200, 800, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        assert_eq!(probe_dimensions(&path), Some((1200, 800)));
        assert_eq!(probe_dimensions(&dir.path().join("absent.png")), None);
    }

    /// An ordinary photograph must keep every pixel — the reduced path is for
    /// gigapixel composites, not for the archive's normal work.
    #[test]
    fn an_ordinary_photograph_is_not_reduced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("normal.png");
        image::RgbImage::from_pixel(900, 600, image::Rgb([200, 40, 40]))
            .save(&path)
            .unwrap();
        let img = open_rgb(&path, &dir.path().join("scratch")).unwrap();
        assert_eq!(img.dimensions(), (900, 600));
        assert!(
            u64::from(900u32) * 600 < MAX_FULL_DECODE_PIXELS,
            "the fixture must sit below the threshold or it proves nothing"
        );
    }

    /// The reduced path bounds the long edge and leaves the original untouched.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_reduced_copy_is_bounded_and_the_original_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        image::RgbImage::from_fn(6000, 4000, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
        })
        .save(&path)
        .unwrap();
        let before = std::fs::metadata(&path).unwrap();

        let scratch = dir.path().join("scratch");
        let img = open_downsampled(&path, &scratch).unwrap();
        assert!(
            img.width().max(img.height()) <= DOWNSAMPLE_EDGE,
            "long edge {} exceeds the cap",
            img.width().max(img.height())
        );
        // Aspect ratio survives, so a fingerprint taken from this still
        // describes the same picture.
        let ratio = img.width() as f64 / img.height() as f64;
        assert!((ratio - 1.5).abs() < 0.01, "aspect drifted to {ratio}");

        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());

        let leftovers: Vec<_> = std::fs::read_dir(&scratch)
            .map(|d| d.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "scratch must be left clean: {leftovers:?}");
    }
}
