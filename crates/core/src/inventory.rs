//! What is on each drive, answerable with every drive unplugged.
//!
//! This is the product's central promise (D-003): the catalogue on this Mac is
//! the authority, so "what is on Drive 5?" and "which drive holds the bicycle
//! photographs?" are answered from `archive.db` alone. Nothing here touches a
//! volume, so every function works identically whether a drive is connected,
//! sitting in a drawer, or on a shelf in another house.
//!
//! Two questions, two shapes:
//!
//!   * [`drive_contents`] — an inventory. What is on this drive, roughly when it
//!     is from, and what it mostly shows.
//!   * [`drives_matching`] — a search rolled up by drive, so the answer to
//!     "where are my bike photos?" is "Drives 1, 5 and 6" rather than a list of
//!     files the user has to read drive numbers off.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// How many subject tags to summarise a drive with. Enough to characterise it
/// ("beach, wedding, dog"), few enough to read at a glance.
const SUMMARY_TAG_COUNT: usize = 8;

/// A tag and how many photographs on the drive carry it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCount {
    pub tag: String,
    pub count: i64,
}

/// An inventory of one drive, readable with the drive disconnected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveContents {
    pub drive_number: i64,
    pub drive_name: Option<String>,
    pub status: String,
    pub online: bool,
    /// Where the user said the physical disk lives.
    pub physical_location: Option<String>,
    pub categories: Vec<String>,
    pub last_scan_at: Option<String>,
    pub photo_count: i64,
    /// Catalogued files whose original was not found on the last scan.
    pub missing_count: i64,
    /// Earliest and latest estimated dates across the drive, when known.
    pub earliest_date: Option<String>,
    pub latest_date: Option<String>,
    /// What the drive mostly shows, most common first.
    pub top_tags: Vec<TagCount>,
    /// Photographs with readable text in them.
    pub with_text_count: i64,
    pub people_count: i64,
}

impl DriveContents {
    /// One line a person can act on, e.g.
    /// "Drive 5 — Holidays: 8,891 photographs, 1998–2011. Mostly beach, wedding,
    /// dog. Disconnected — in Drawer 2."
    pub fn summary(&self) -> String {
        let name = self
            .drive_name
            .clone()
            .map(|n| format!(" — {n}"))
            .unwrap_or_default();
        let mut s = format!(
            "Drive {}{}: {} photograph{}",
            self.drive_number,
            name,
            self.photo_count,
            if self.photo_count == 1 { "" } else { "s" }
        );
        if let (Some(a), Some(b)) = (&self.earliest_date, &self.latest_date) {
            let (ya, yb) = (&a[..4.min(a.len())], &b[..4.min(b.len())]);
            s.push_str(&if ya == yb {
                format!(", {ya}")
            } else {
                format!(", {ya}–{yb}")
            });
        }
        s.push('.');
        if !self.top_tags.is_empty() {
            let names: Vec<&str> = self
                .top_tags
                .iter()
                .take(3)
                .map(|t| t.tag.as_str())
                .collect();
            s.push_str(&format!(" Mostly {}.", names.join(", ")));
        }
        s.push_str(if self.online {
            " Connected."
        } else {
            " Disconnected."
        });
        if let Some(loc) = &self.physical_location {
            s.push_str(&format!(" Kept in {loc}."));
        }
        s
    }
}

/// Inventory every registered drive, or just one when `drive_number` is given.
pub fn drive_contents(conn: &Connection, drive_number: Option<i64>) -> Result<Vec<DriveContents>> {
    let repo = crate::drive::DriveRepo::new(conn);
    let drives = match drive_number {
        Some(n) => repo.get_by_number(n)?.into_iter().collect::<Vec<_>>(),
        None => repo.list()?,
    };

    let mut out = Vec::new();
    for d in drives {
        let photo_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM files WHERE drive_id=?1 AND status='complete'",
                [&d.id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let missing_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM files WHERE drive_id=?1 AND status='missing'",
                [&d.id],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let (earliest_date, latest_date): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT min(de.earliest_date), max(de.latest_date)
                   FROM date_estimates de JOIN files f ON f.id = de.file_id
                  WHERE f.drive_id=?1 AND f.status='complete'",
                [&d.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((None, None));

        // What the drive mostly shows. Person tags are excluded: naming people is
        // the user's business, and a drive summary is not the place to surface it.
        let mut stmt = conn.prepare(
            "SELECT t.name, count(*) AS n
               FROM file_tags ft
               JOIN tags t ON t.id = ft.tag_id
               JOIN files f ON f.id = ft.file_id
              WHERE f.drive_id = ?1 AND f.status='complete' AND t.tag_type <> 'person'
              GROUP BY t.name
              ORDER BY n DESC, t.name ASC
              LIMIT ?2",
        )?;
        let top_tags: Vec<TagCount> = stmt
            .query_map(params![d.id, SUMMARY_TAG_COUNT as i64], |r| {
                Ok(TagCount { tag: r.get(0)?, count: r.get(1)? })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let with_text_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM scene_analysis s JOIN files f ON f.id = s.file_id
                  WHERE f.drive_id=?1 AND f.status='complete'
                    AND s.ocr_text IS NOT NULL AND length(trim(s.ocr_text)) > 0",
                [&d.id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let people_count: i64 = conn
            .query_row(
                "SELECT COALESCE(sum(s.people_count), 0) FROM scene_analysis s
                   JOIN files f ON f.id = s.file_id
                  WHERE f.drive_id=?1 AND f.status='complete'",
                [&d.id],
                |r| r.get(0),
            )
            .unwrap_or(0);

        out.push(DriveContents {
            drive_number: d.drive_number,
            drive_name: d.friendly_name,
            online: d.status == "online",
            status: d.status,
            physical_location: d.physical_location,
            categories: d.categories,
            last_scan_at: d.last_scan_at,
            photo_count,
            missing_count,
            earliest_date,
            latest_date,
            top_tags,
            with_text_count,
            people_count,
        });
    }
    out.sort_by_key(|c| c.drive_number);
    Ok(out)
}

/// Every subject the catalogue has recognised, most common first.
///
/// This is what makes the archive browsable rather than interrogable: without
/// it, finding anything depends on guessing a word that happens to be in there.
/// Person tags are excluded — people belong on the People screen, where naming
/// them is a deliberate act (D-007), not mixed into a subject list.
pub fn all_tags(conn: &Connection, limit: usize) -> Result<Vec<TagCount>> {
    let mut stmt = conn.prepare(
        "SELECT t.name, count(*) AS n
           FROM file_tags ft
           JOIN tags t  ON t.id = ft.tag_id
           JOIN files f ON f.id = ft.file_id
          WHERE f.status = 'complete' AND t.tag_type <> 'person'
          GROUP BY t.name
          ORDER BY n DESC, t.name ASC
          LIMIT ?1",
    )?;
    let out = stmt
        .query_map([limit as i64], |r| {
            Ok(TagCount { tag: r.get(0)?, count: r.get(1)? })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(out)
}

/// Which drives hold photographs matching a search, and how many each holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveMatch {
    pub drive_number: i64,
    pub drive_name: Option<String>,
    pub online: bool,
    pub physical_location: Option<String>,
    /// Matching photographs on this drive.
    pub match_count: i64,
    /// A few filenames, so the user can see the answer is plausible.
    pub examples: Vec<String>,
}

/// Roll a set of search results up by drive.
///
/// Takes results rather than re-querying so the grouping always agrees with the
/// list the user is looking at — including the visual leg of a natural-language
/// search, which no SQL query here could reproduce.
pub fn drives_matching(results: &[crate::search::SearchResult]) -> Vec<DriveMatch> {
    let mut by_drive: std::collections::BTreeMap<i64, DriveMatch> = std::collections::BTreeMap::new();
    for r in results {
        let entry = by_drive.entry(r.drive_number).or_insert_with(|| DriveMatch {
            drive_number: r.drive_number,
            drive_name: r.drive_name.clone(),
            online: r.online,
            physical_location: None,
            match_count: 0,
            examples: Vec::new(),
        });
        entry.match_count += 1;
        if entry.examples.len() < 3 {
            entry.examples.push(r.filename.clone());
        }
    }
    let mut out: Vec<DriveMatch> = by_drive.into_values().collect();
    // Most matches first: the drive most likely to be the one worth fetching.
    out.sort_by(|a, b| b.match_count.cmp(&a.match_count).then(a.drive_number.cmp(&b.drive_number)));
    out
}

/// Fill in where each matched drive physically lives, so the answer to "which
/// drive do I need?" also says where to find it.
pub fn locate_matches(conn: &Connection, matches: &mut [DriveMatch]) -> Result<()> {
    let repo = crate::drive::DriveRepo::new(conn);
    for m in matches.iter_mut() {
        if let Some(d) = repo.get_by_number(m.drive_number)? {
            m.physical_location = d.physical_location;
            m.online = d.status == "online";
            if m.drive_name.is_none() {
                m.drive_name = d.friendly_name;
            }
        }
    }
    Ok(())
}

/// One line telling the user which drives to connect, e.g.
/// "Found on Drives 1, 5 and 6. Drive 5 has the most (9) — kept in Drawer 2."
pub fn where_to_look(matches: &[DriveMatch]) -> String {
    match matches.len() {
        0 => "Not found on any indexed drive.".to_string(),
        _ => {
            let numbers: Vec<String> = {
                let mut ns: Vec<i64> = matches.iter().map(|m| m.drive_number).collect();
                ns.sort_unstable();
                ns.iter().map(|n| n.to_string()).collect()
            };
            let list = match numbers.len() {
                1 => format!("Drive {}", numbers[0]),
                _ => format!(
                    "Drives {} and {}",
                    numbers[..numbers.len() - 1].join(", "),
                    numbers[numbers.len() - 1]
                ),
            };
            let best = &matches[0];
            let mut s = format!("Found on {list}.");
            if matches.len() > 1 {
                s.push_str(&format!(
                    " Drive {} has the most ({}).",
                    best.drive_number, best.match_count
                ));
            }
            let offline: Vec<String> = matches
                .iter()
                .filter(|m| !m.online)
                .map(|m| {
                    match &m.physical_location {
                        Some(loc) => format!("Drive {} ({loc})", m.drive_number),
                        None => format!("Drive {}", m.drive_number),
                    }
                })
                .collect();
            if !offline.is_empty() {
                s.push_str(&format!(" Connect {} to open the originals.", offline.join(", ")));
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchResult;

    fn result(drive: i64, filename: &str, online: bool) -> SearchResult {
        SearchResult {
            file_id: format!("{drive}-{filename}"),
            filename: filename.into(),
            relative_path: filename.into(),
            drive_number: drive,
            drive_name: Some(format!("Drive {drive}")),
            drive_status: if online { "online".into() } else { "offline".into() },
            online,
            thumbnail_rel_path: None,
            date_range: None,
            date_label: None,
            matched: vec!["text".into()],
            score: 1.0,
        }
    }

    #[test]
    fn groups_results_by_drive_most_matches_first() {
        let results = vec![
            result(1, "a.jpg", true),
            result(5, "b.jpg", false),
            result(5, "c.jpg", false),
            result(6, "d.jpg", false),
            result(5, "e.jpg", false),
        ];
        let grouped = drives_matching(&results);
        assert_eq!(grouped.len(), 3);
        assert_eq!(grouped[0].drive_number, 5, "the drive with the most matches leads");
        assert_eq!(grouped[0].match_count, 3);
        assert_eq!(grouped[0].examples.len(), 3);
    }

    #[test]
    fn tells_the_user_which_drives_to_connect() {
        let mut grouped = drives_matching(&[
            result(1, "a.jpg", true),
            result(5, "b.jpg", false),
            result(5, "c.jpg", false),
            result(6, "d.jpg", false),
        ]);
        grouped[0].physical_location = Some("Drawer 2".into());

        let line = where_to_look(&grouped);
        assert!(line.contains("Drives 1, 5 and 6"), "got {line}");
        assert!(line.contains("Drive 5 has the most (2)"), "got {line}");
        // The connected drive is not something to go and fetch.
        assert!(!line.contains("Connect Drive 1"), "got {line}");
        assert!(line.contains("Drive 5 (Drawer 2)"), "got {line}");
    }

    #[test]
    fn a_single_drive_reads_naturally() {
        let grouped = drives_matching(&[result(3, "a.jpg", true)]);
        assert_eq!(where_to_look(&grouped), "Found on Drive 3.");
    }

    #[test]
    fn nothing_found_says_so_plainly() {
        assert_eq!(where_to_look(&[]), "Not found on any indexed drive.");
    }

    #[test]
    fn drive_summary_reads_as_a_sentence() {
        let c = DriveContents {
            drive_number: 5,
            drive_name: Some("Holidays".into()),
            status: "offline".into(),
            online: false,
            physical_location: Some("Drawer 2".into()),
            categories: vec![],
            last_scan_at: None,
            photo_count: 8891,
            missing_count: 0,
            earliest_date: Some("1998-01-01".into()),
            latest_date: Some("2011-12-31".into()),
            top_tags: vec![
                TagCount { tag: "beach".into(), count: 900 },
                TagCount { tag: "wedding".into(), count: 120 },
                TagCount { tag: "dog".into(), count: 80 },
            ],
            with_text_count: 4,
            people_count: 120,
        };
        let s = c.summary();
        assert!(s.contains("Drive 5 — Holidays"), "got {s}");
        assert!(s.contains("8891 photographs"), "got {s}");
        assert!(s.contains("1998–2011"), "got {s}");
        assert!(s.contains("Mostly beach, wedding, dog."), "got {s}");
        assert!(s.contains("Disconnected."), "got {s}");
        assert!(s.contains("Kept in Drawer 2."), "got {s}");
    }
}
