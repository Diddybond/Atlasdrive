//! EXIF and technical metadata extraction (read-only).
//!
//! Raw and normalized values are preserved separately (see `docs/04`). Capture
//! and digitised dates are surfaced distinctly for the date estimator.

use std::io::BufReader;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::integrity::open_readonly;

/// Extracted metadata, all optional.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<u32>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    /// Normalized `YYYY-MM-DD` if a capture date was found.
    pub exif_capture_date: Option<String>,
    pub exif_digitized_date: Option<String>,
    pub color_profile: Option<String>,
    /// Raw EXIF field dump for provenance.
    pub raw: std::collections::BTreeMap<String, String>,
}

/// Read EXIF metadata from a file strictly read-only. Missing or malformed EXIF
/// is not an error: it returns whatever could be parsed.
pub fn extract(path: &Path, decoded_dims: Option<(u32, u32)>) -> ImageMetadata {
    let mut md = ImageMetadata::default();
    if let Some((w, h)) = decoded_dims {
        md.width = Some(w);
        md.height = Some(h);
    }

    let file = match open_readonly(path) {
        Ok(f) => f,
        Err(_) => return md,
    };
    let mut reader = BufReader::new(file);
    let exifreader = exif::Reader::new();
    let exif = match exifreader.read_from_container(&mut reader) {
        Ok(e) => e,
        Err(_) => return md, // no/broken EXIF is fine
    };

    for field in exif.fields() {
        let tag = field.tag.to_string();
        let val = field.display_value().to_string();
        md.raw.insert(tag.clone(), val.clone());
        match field.tag {
            exif::Tag::Make => md.camera_make = Some(clean(&val)),
            exif::Tag::Model => md.camera_model = Some(clean(&val)),
            exif::Tag::LensModel => md.lens = Some(clean(&val)),
            exif::Tag::Orientation => {
                md.orientation = field.value.get_uint(0);
            }
            exif::Tag::DateTimeOriginal => {
                md.exif_capture_date = normalize_exif_date(&val);
            }
            exif::Tag::DateTimeDigitized => {
                md.exif_digitized_date = normalize_exif_date(&val);
            }
            exif::Tag::PixelXDimension if md.width.is_none() => {
                md.width = field.value.get_uint(0);
            }
            exif::Tag::PixelYDimension if md.height.is_none() => {
                md.height = field.value.get_uint(0);
            }
            _ => {}
        }
    }
    md
}

fn clean(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

/// Normalize an EXIF `YYYY:MM:DD HH:MM:SS` value to `YYYY-MM-DD`.
fn normalize_exif_date(s: &str) -> Option<String> {
    let s = s.trim().trim_matches('"');
    let date_part = s.split_whitespace().next()?;
    let mut it = date_part.split([':', '-']);
    let y = it.next()?;
    let m = it.next()?;
    let d = it.next()?;
    if y.len() == 4 && m.len() <= 2 && d.len() <= 2 {
        let yi: i32 = y.parse().ok()?;
        if (1900..=2100).contains(&yi) {
            return Some(format!("{y}-{:0>2}-{:0>2}", m, d));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_dates() {
        assert_eq!(
            normalize_exif_date("2008:06:14 10:11:12"),
            Some("2008-06-14".into())
        );
        assert_eq!(normalize_exif_date("garbage"), None);
    }

    #[test]
    fn missing_exif_is_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("plain.bin");
        std::fs::write(&p, b"no exif here").unwrap();
        let md = extract(&p, Some((10, 20)));
        assert_eq!(md.width, Some(10));
        assert_eq!(md.height, Some(20));
        assert!(md.exif_capture_date.is_none());
    }
}
