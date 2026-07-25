//! Date-range estimation (see `docs/09_DATE_ESTIMATION.md`).
//!
//! Never fabricate an exact capture date. Every automatic estimate is a range
//! with a confidence and an explainable evidence list. User-confirmed dates
//! always win and are stored distinctly.

use serde::{Deserialize, Serialize};

pub const METHOD_VERSION: &str = "date-estimator-0.1.0";

/// Distinct date sources, in the authority order from `docs/09`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateSource {
    UserConfirmed,
    ExifCapture,
    ExifDigitized,
    VisibleDateStamp,
    FilenameOrFolder,
    Estimated,
    FileSystem,
}

/// The stored estimate contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateEstimate {
    pub earliest_date: String, // ISO date (YYYY-MM-DD)
    pub latest_date: String,
    pub confidence: f32,
    pub evidence: Vec<String>,
    pub method_version: String,
    pub primary_source: DateSource,
    pub is_user_confirmed: bool,
}

/// Inputs available to the estimator.
#[derive(Debug, Clone, Default)]
pub struct DateInputs {
    /// EXIF original capture date, if reliably parsed (`YYYY-MM-DD`).
    pub exif_capture: Option<String>,
    pub exif_digitized: Option<String>,
    /// Filesystem modification date (`YYYY-MM-DD`); weak evidence only.
    pub fs_mtime_date: Option<String>,
    /// A 4-digit year discovered in the filename or folder path.
    pub filename_year: Option<i32>,
    pub likely_scanned_print: bool,
    pub is_grayscale: bool,
}

/// Produce an automatic date estimate honouring the authority order.
///
/// This never returns a single fabricated exact date: when only weak evidence
/// exists it widens to a decade-or-more range with low confidence.
pub fn estimate(inputs: &DateInputs) -> DateEstimate {
    let mut evidence = Vec::new();

    // 1. Trusted EXIF capture date → narrow, high confidence.
    if let Some(d) = &inputs.exif_capture {
        if let Some((y, m, day)) = parse_ymd(d) {
            evidence.push("trusted EXIF capture date".to_string());
            let iso = format!("{y:04}-{m:02}-{day:02}");
            return DateEstimate {
                earliest_date: iso.clone(),
                latest_date: iso,
                confidence: 0.95,
                evidence,
                method_version: METHOD_VERSION.into(),
                primary_source: DateSource::ExifCapture,
                is_user_confirmed: false,
            };
        }
    }

    // 2. EXIF digitized (e.g. scan of a print at a known digitisation time).
    if let Some(d) = &inputs.exif_digitized {
        if let Some((y, _, _)) = parse_ymd(d) {
            evidence.push("EXIF digitized date present; original capture unknown".into());
            if inputs.likely_scanned_print {
                evidence.push("image looks like a scanned print".into());
                // Digitised year is an upper bound; original could be older.
                return DateEstimate {
                    earliest_date: "1900-01-01".into(),
                    latest_date: format!("{y:04}-12-31"),
                    confidence: 0.25,
                    evidence,
                    method_version: METHOD_VERSION.into(),
                    primary_source: DateSource::Estimated,
                    is_user_confirmed: false,
                };
            }
            return DateEstimate {
                earliest_date: format!("{y:04}-01-01"),
                latest_date: format!("{y:04}-12-31"),
                confidence: 0.5,
                evidence,
                method_version: METHOD_VERSION.into(),
                primary_source: DateSource::ExifDigitized,
                is_user_confirmed: false,
            };
        }
    }

    // 3. Strong filename/folder year evidence.
    if let Some(year) = inputs.filename_year {
        if (1900..=2100).contains(&year) {
            evidence.push(format!("year {year} found in filename or folder"));
            return DateEstimate {
                earliest_date: format!("{year:04}-01-01"),
                latest_date: format!("{year:04}-12-31"),
                confidence: 0.55,
                evidence,
                method_version: METHOD_VERSION.into(),
                primary_source: DateSource::FilenameOrFolder,
                is_user_confirmed: false,
            };
        }
    }

    // 4. Weak visual estimate. Grayscale + scan cues nudge older ranges but we
    //    stay honest with a wide range and low confidence.
    if inputs.is_grayscale {
        evidence.push("black-and-white image (weak age cue)".into());
    }
    if inputs.likely_scanned_print {
        evidence.push("likely scanned colour print (weak age cue)".into());
    }
    if let Some(d) = &inputs.fs_mtime_date {
        evidence.push(format!("filesystem modification date {d} (unreliable)"));
    }
    evidence.push("no reliable capture-date evidence; range is uncertain".into());

    // Default honest fallback: broad plausible window for family photos.
    let (earliest, latest, conf) = if inputs.is_grayscale {
        ("1930-01-01", "1979-12-31", 0.20)
    } else if inputs.likely_scanned_print {
        ("1960-01-01", "2005-12-31", 0.18)
    } else {
        ("1970-01-01", "2025-12-31", 0.10)
    };

    DateEstimate {
        earliest_date: earliest.into(),
        latest_date: latest.into(),
        confidence: conf,
        evidence,
        method_version: METHOD_VERSION.into(),
        primary_source: DateSource::Estimated,
        is_user_confirmed: false,
    }
}

/// A user-confirmed range always overrides automatic estimates.
pub fn user_confirmed(earliest: &str, latest: &str) -> DateEstimate {
    DateEstimate {
        earliest_date: earliest.to_string(),
        latest_date: latest.to_string(),
        confidence: 1.0,
        evidence: vec!["user-confirmed date range".into()],
        method_version: METHOD_VERSION.into(),
        primary_source: DateSource::UserConfirmed,
        is_user_confirmed: true,
    }
}

/// Human-friendly phrasing that never presents an estimate as certain.
pub fn describe(est: &DateEstimate) -> String {
    if est.is_user_confirmed {
        if est.earliest_date == est.latest_date {
            return format!("Taken on {}", est.earliest_date);
        }
        return format!("Between {} and {} (confirmed)", est.earliest_date, est.latest_date);
    }
    match est.primary_source {
        DateSource::ExifCapture => format!("Taken on {}", est.earliest_date),
        DateSource::ExifDigitized => {
            format!("Digitised around {}; original date unknown", &est.latest_date[..4])
        }
        _ if est.earliest_date == est.latest_date => format!("Around {}", est.earliest_date),
        _ => {
            let ey = &est.earliest_date[..4];
            let ly = &est.latest_date[..4];
            if est.confidence < 0.2 {
                "Date uncertain".to_string()
            } else {
                format!("Likely taken between {ey} and {ly}")
            }
        }
    }
}

/// Extract a plausible 4-digit year from a filename or path fragment.
pub fn year_from_text(text: &str) -> Option<i32> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            let year: i32 = text[i..i + 4].parse().unwrap_or(0);
            if (1900..=2099).contains(&year) {
                return Some(year);
            }
        }
        i += 1;
    }
    None
}

fn parse_ymd(s: &str) -> Option<(i32, u32, u32)> {
    // Accept "YYYY-MM-DD" or EXIF "YYYY:MM:DD".
    let norm = s.replace(':', "-");
    let mut parts = norm.split(['-', ' ', 'T']);
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next().unwrap_or("1").parse().ok()?;
    let d: u32 = parts.next().unwrap_or("1").parse().ok()?;
    if (1900..=2100).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d) {
        Some((y, m, d))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_capture_is_exact_and_confident() {
        let inputs = DateInputs {
            exif_capture: Some("1998:06:14".into()),
            ..Default::default()
        };
        let est = estimate(&inputs);
        assert_eq!(est.earliest_date, "1998-06-14");
        assert_eq!(est.latest_date, "1998-06-14");
        assert!(est.confidence > 0.9);
        assert_eq!(est.primary_source, DateSource::ExifCapture);
        assert!(describe(&est).starts_with("Taken on"));
    }

    #[test]
    fn no_evidence_gives_wide_low_confidence_range() {
        let est = estimate(&DateInputs::default());
        assert_ne!(est.earliest_date, est.latest_date);
        assert!(est.confidence < 0.2);
        assert_eq!(describe(&est), "Date uncertain");
    }

    #[test]
    fn grayscale_scan_nudges_older() {
        let est = estimate(&DateInputs {
            is_grayscale: true,
            ..Default::default()
        });
        assert!(est.earliest_date.starts_with("1930"));
        assert!(est.evidence.iter().any(|e| e.contains("black-and-white")));
    }

    #[test]
    fn user_confirmed_overrides() {
        let est = user_confirmed("1982-01-01", "1988-12-31");
        assert!(est.is_user_confirmed);
        assert_eq!(est.confidence, 1.0);
        assert!(describe(&est).contains("confirmed"));
    }

    #[test]
    fn year_extraction() {
        assert_eq!(year_from_text("Xmas_1987_scan.jpg"), Some(1987));
        assert_eq!(year_from_text("IMG_20040101.jpg"), Some(2004));
        assert_eq!(year_from_text("no-year-here.jpg"), None);
    }
}
