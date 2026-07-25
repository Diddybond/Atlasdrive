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

/// Read/write access to stored date estimates.
pub struct DateRepo<'a> {
    conn: &'a rusqlite::Connection,
}

impl<'a> DateRepo<'a> {
    pub fn new(conn: &'a rusqlite::Connection) -> Self {
        Self { conn }
    }

    /// Record the user's own correction for a photograph's date.
    ///
    /// This is the highest authority in `docs/09`: it replaces whatever the
    /// estimator produced, is stored with `is_user_confirmed`, and re-analysis
    /// will not overwrite it. Both bounds are validated and ordered so a
    /// reversed range cannot be stored.
    pub fn set_user_override(
        &self,
        file_id: &str,
        earliest: &str,
        latest: &str,
    ) -> crate::error::Result<DateEstimate> {
        let (earliest, latest) = normalise_range(earliest, latest)?;
        let est = user_confirmed(&earliest, &latest);
        let now = crate::util::now_iso8601();
        let changed = self.conn.execute(
            "INSERT INTO date_estimates
               (file_id, earliest_date, latest_date, confidence, method_version,
                evidence_json, is_user_confirmed, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,1,?7,?7)
             ON CONFLICT(file_id) DO UPDATE SET
                earliest_date=excluded.earliest_date, latest_date=excluded.latest_date,
                confidence=excluded.confidence, method_version=excluded.method_version,
                evidence_json=excluded.evidence_json, is_user_confirmed=1,
                updated_at=excluded.updated_at",
            rusqlite::params![
                file_id,
                est.earliest_date,
                est.latest_date,
                est.confidence,
                est.method_version,
                serde_json::to_string(&est.evidence)?,
                now,
            ],
        )?;
        if changed == 0 {
            return Err(crate::error::Error::InvalidArgs(format!(
                "no photograph with id {file_id}"
            )));
        }
        Ok(est)
    }

    /// Remove a user's correction, letting the automatic estimate stand again.
    pub fn clear_user_override(&self, file_id: &str) -> crate::error::Result<()> {
        self.conn.execute(
            "DELETE FROM date_estimates WHERE file_id=?1 AND is_user_confirmed=1",
            [file_id],
        )?;
        Ok(())
    }

    /// The stored estimate for a file, if any.
    pub fn get(&self, file_id: &str) -> crate::error::Result<Option<DateEstimate>> {
        let row = self.conn.query_row(
            "SELECT earliest_date, latest_date, confidence, method_version, evidence_json,
                    is_user_confirmed
               FROM date_estimates WHERE file_id=?1",
            [file_id],
            |r| {
                let evidence_json: String = r.get(4)?;
                let is_user_confirmed: i64 = r.get(5)?;
                Ok(DateEstimate {
                    earliest_date: r.get(0)?,
                    latest_date: r.get(1)?,
                    confidence: r.get(2)?,
                    method_version: r.get(3)?,
                    evidence: serde_json::from_str(&evidence_json).unwrap_or_default(),
                    primary_source: if is_user_confirmed == 1 {
                        DateSource::UserConfirmed
                    } else {
                        DateSource::Estimated
                    },
                    is_user_confirmed: is_user_confirmed == 1,
                })
            },
        );
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Validate a user-supplied range and put its bounds in order.
///
/// A single date is a valid range (the user is certain). A reversed range is
/// almost always a typo, so it is corrected rather than rejected — but a
/// malformed date is refused, because guessing at it would be fabricating.
fn normalise_range(earliest: &str, latest: &str) -> crate::error::Result<(String, String)> {
    let a = earliest.trim();
    let b = if latest.trim().is_empty() { a } else { latest.trim() };
    for value in [a, b] {
        if !is_iso_date(value) {
            return Err(crate::error::Error::InvalidArgs(format!(
                "date must be YYYY-MM-DD, got {value:?}"
            )));
        }
    }
    Ok(if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    })
}

#[cfg(test)]
mod override_tests {
    use super::*;

    #[test]
    fn a_reversed_range_is_corrected_not_rejected() {
        assert_eq!(
            normalise_range("1998-12-01", "1998-01-01").unwrap(),
            ("1998-01-01".to_string(), "1998-12-01".to_string())
        );
    }

    #[test]
    fn a_single_date_is_a_valid_range() {
        assert_eq!(
            normalise_range("1998-08-12", "").unwrap(),
            ("1998-08-12".to_string(), "1998-08-12".to_string())
        );
    }

    #[test]
    fn malformed_dates_are_refused_rather_than_guessed_at() {
        for bad in ["1998", "12/08/1998", "1998-13-01", "1998-08-32", "not a date"] {
            assert!(
                normalise_range(bad, bad).is_err(),
                "{bad:?} should not be accepted"
            );
        }
    }
}

/// `YYYY-MM-DD` with plausible month/day values.
fn is_iso_date(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    if !s.chars().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit()) {
        return false;
    }
    let month: u32 = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
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
