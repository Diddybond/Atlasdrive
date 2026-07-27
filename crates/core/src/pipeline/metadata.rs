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

/// Longest EXIF value kept verbatim.
///
/// Any genuine, readable EXIF value — a camera model, an exposure, a date — is
/// a few dozen characters. Anything longer is a binary field being rendered as
/// text, and it is the *rendering* that is enormous, not the information.
const MAX_TAG_VALUE_CHARS: usize = 256;

/// Hard ceiling on the whole raw dump for one photograph.
const MAX_RAW_TOTAL_CHARS: usize = 32 * 1024;

/// Record one EXIF field, refusing to let a binary blob in.
///
/// This is the fix for the single worst defect AtlasDrive has had. The raw dump
/// was built by calling `display_value()` on every EXIF field and keeping the
/// result whole. For scalar tags that is a few characters. For binary ones —
/// MakerNote, colour matrices, embedded previews — it renders every byte as
/// text, so a tag holding a 200MB payload became a 600MB string.
///
/// On a real catalogue of 8,486 photographs this produced **38.08 GB** of
/// `raw_json`, averaging 4.6MB per photograph with one row at 865MB, against
/// 0.07GB for every face crop and 0.03GB for every embedding combined. It also
/// caused the "string or blob too big" failures: a row past SQLite's 1GB limit
/// cannot be stored at all, so those photographs were dropped from the
/// catalogue entirely.
///
/// What is kept is the fact that the tag was present and how large it was,
/// which is what provenance actually needs. What is discarded is a
/// hex-rendering of bytes no part of AtlasDrive reads.
fn record_raw(
    raw: &mut std::collections::BTreeMap<String, String>,
    total: &mut usize,
    tag: &str,
    val: &str,
) {
    if *total >= MAX_RAW_TOTAL_CHARS {
        raw.insert(
            "_note".into(),
            format!("further tags omitted after {MAX_RAW_TOTAL_CHARS} characters"),
        );
        return;
    }
    let kept = if val.len() > MAX_TAG_VALUE_CHARS {
        format!("[{} characters omitted — binary or oversized field]", val.len())
    } else {
        val.to_string()
    };
    *total += tag.len() + kept.len();
    raw.insert(tag.to_string(), kept);
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

    let mut raw_total = 0usize;
    for field in exif.fields() {
        let tag = field.tag.to_string();
        let val = field.display_value().to_string();
        record_raw(&mut md.raw, &mut raw_total, &tag, &val);
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

#[cfg(test)]
mod raw_size_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn record(pairs: &[(&str, String)]) -> BTreeMap<String, String> {
        let mut raw = BTreeMap::new();
        let mut total = 0usize;
        for (tag, val) in pairs {
            record_raw(&mut raw, &mut total, tag, val);
        }
        raw
    }

    /// The defect, at the scale it actually occurred: one binary tag rendered
    /// as 600MB of text. It must not reach the catalogue, and the fact that it
    /// existed must not be lost either.
    #[test]
    fn a_binary_tag_is_recorded_but_not_stored() {
        let huge = "0x00, ".repeat(2_000_000); // ~12MB, same shape as the real one
        let raw = record(&[("MakerNote", huge.clone())]);
        let kept = &raw["MakerNote"];
        assert!(kept.len() < MAX_TAG_VALUE_CHARS, "still {} chars", kept.len());
        assert!(kept.contains(&huge.len().to_string()), "must say how big it was: {kept}");
        assert!(kept.contains("omitted"));
    }

    /// Ordinary values must survive untouched — this is provenance, and a
    /// camera model that reads "[47 characters omitted]" would be useless.
    #[test]
    fn ordinary_values_are_kept_exactly() {
        let raw = record(&[
            ("Model", "NIKON D810".into()),
            ("DateTimeOriginal", "2018-01-18 21:42:17".into()),
            ("FNumber", "f/2.8".into()),
        ]);
        assert_eq!(raw["Model"], "NIKON D810");
        assert_eq!(raw["DateTimeOriginal"], "2018-01-18 21:42:17");
        assert_eq!(raw["FNumber"], "f/2.8");
    }

    /// Many medium-sized tags must not add up to something enormous either.
    #[test]
    fn the_whole_dump_is_bounded() {
        let pairs: Vec<(&str, String)> =
            (0..2000).map(|_| ("Tag", "x".repeat(200))).collect();
        let raw = record(&pairs);
        let total: usize = raw.iter().map(|(k, v)| k.len() + v.len()).sum();
        assert!(
            total < MAX_RAW_TOTAL_CHARS * 2,
            "raw dump reached {total} characters"
        );
    }

    /// A photograph's whole dump has to stay far below SQLite's 1GB value
    /// limit — exceeding it is what dropped photographs from the catalogue
    /// with "string or blob too big".
    #[test]
    fn a_dump_can_never_approach_the_database_limit() {
        let pairs: Vec<(&str, String)> = (0..50)
            .map(|_| ("MakerNote", "0x00, ".repeat(1_000_000)))
            .collect();
        let raw = record(&pairs);
        let json = serde_json::to_string(&raw).unwrap();
        assert!(
            json.len() < 1_000_000,
            "serialised dump is {} bytes; SQLite refuses at 1,000,000,000",
            json.len()
        );
    }
}
