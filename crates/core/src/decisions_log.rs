//! A guard on `docs/16_DECISIONS.md`, the record of settled decisions.
//!
//! The log is append-only prose, so nothing but habit stopped two sessions from
//! reaching for the same number. Two did: a decode timeout was filed as D-025
//! and a disconnected drive as D-026, numbers already held by the catalogue
//! answers and the face model, and both were inserted mid-file above the
//! template rather than after the last entry. Code comments cite these numbers
//! (`faces.rs` points at D-026 for the face model), so a collision does not
//! merely look untidy — it sends a reader to the wrong decision.
//!
//! These tests are the cheapest place to notice. They read the real file, so
//! they fail in the session that introduced the mistake rather than in the one
//! that later followed the citation.

use std::path::PathBuf;

/// One `## D-NNN` heading, with the line it was found on.
struct Heading {
    id: u32,
    line: usize,
    text: String,
}

fn decisions_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/16_DECISIONS.md")
}

/// Every `## D-NNN` heading in file order.
///
/// The template at the foot of the file is `## D-XXX`, which has no number and
/// is skipped on exactly that basis rather than by position.
fn headings(markdown: &str) -> Vec<Heading> {
    markdown
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let rest = line.strip_prefix("## D-")?;
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if digits.len() != 3 {
                return None;
            }
            Some(Heading {
                id: digits.parse().ok()?,
                line: i + 1,
                text: line.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log() -> String {
        std::fs::read_to_string(decisions_path()).expect("docs/16_DECISIONS.md is readable")
    }

    #[test]
    fn every_decision_number_is_used_once() {
        let markdown = log();
        let headings = headings(&markdown);
        let mut seen: Vec<&Heading> = Vec::new();
        for h in &headings {
            if let Some(first) = seen.iter().find(|s| s.id == h.id) {
                panic!(
                    "D-{:03} is used twice:\n  line {}: {}\n  line {}: {}\n\
                     Decision numbers are cited from code and from other decisions, so \
                     the second one needs a new number.",
                    h.id, first.line, first.text, h.line, h.text
                );
            }
            seen.push(h);
        }
        assert!(
            headings.len() >= 82,
            "expected the decision log to keep its entries, found {}",
            headings.len()
        );
    }

    #[test]
    fn decisions_are_numbered_from_one_with_no_gaps() {
        let markdown = log();
        let mut ids: Vec<u32> = headings(&markdown).iter().map(|h| h.id).collect();
        ids.sort_unstable();
        let expected: Vec<u32> = (1..=ids.len() as u32).collect();
        assert_eq!(
            ids, expected,
            "decision numbers should run 1..N with no gaps; a gap means an entry was \
             deleted rather than superseded, which loses the reason it existed"
        );
    }

    #[test]
    fn a_new_decision_is_appended_after_the_last_one() {
        let markdown = log();
        let headings = headings(&markdown);
        for pair in headings.windows(2) {
            let (previous, next) = (&pair[0], &pair[1]);
            assert!(
                next.id > previous.id,
                "D-{:03} (line {}) appears after D-{:03} (line {}). A new decision goes at \
                 the end of the file, so reading top to bottom is reading in order.",
                next.id,
                next.line,
                previous.id,
                previous.line
            );
        }
    }

    #[test]
    fn the_template_is_not_counted_as_a_decision() {
        // The `D-XXX` template must stay unnumbered: it is a form, not a record.
        assert!(log().contains("## D-XXX: Title"));
        assert!(headings("## D-XXX: Title").is_empty());
    }
}
