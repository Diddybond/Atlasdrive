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
    let decoded =
        image::open(abs).map_err(|e| Error::Other(format!("decode failed: {e}")))?;
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
