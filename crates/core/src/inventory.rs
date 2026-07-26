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
    tags_on_drive(conn, limit, None)
}

/// The same, narrowed to one drive.
///
/// A tag cloud built from every drive is the wrong tool once a drive is
/// unplugged: it offers subjects the connected disk cannot show. Scoping the
/// cloud to the selected drive means every chip on screen leads somewhere.
pub fn tags_on_drive(
    conn: &Connection,
    limit: usize,
    drive_number: Option<i64>,
) -> Result<Vec<TagCount>> {
    // The most photographed subjects are chosen first, then shown in
    // alphabetical order. Selecting alphabetically would cut the list off at
    // the letter D; ordering the *display* by count makes a specific subject
    // impossible to find, because its position depends on a number the owner
    // does not know. Picking by count and showing by name gives both.
    let sql = format!(
        "SELECT name, n FROM (
           SELECT t.name AS name, count(*) AS n
             FROM file_tags ft
             JOIN tags t  ON t.id = ft.tag_id
             JOIN files f ON f.id = ft.file_id
             JOIN drives d ON d.id = f.drive_id
            WHERE f.status = 'complete' AND t.tag_type <> 'person'{}
            GROUP BY t.name
            ORDER BY n DESC, t.name ASC
            LIMIT ?1
         ) ORDER BY name COLLATE NOCASE ASC",
        match drive_number {
            Some(_) => " AND d.drive_number = ?2",
            None => "",
        }
    );
    let mut stmt = conn.prepare(&sql)?;
    let row = |r: &rusqlite::Row| Ok(TagCount { tag: r.get(0)?, count: r.get(1)? });
    let out = match drive_number {
        Some(dn) => stmt
            .query_map(rusqlite::params![limit as i64, dn], row)?
            .collect::<std::result::Result<Vec<_>, _>>()?,
        None => stmt
            .query_map(rusqlite::params![limit as i64], row)?
            .collect::<std::result::Result<Vec<_>, _>>()?,
    };
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

// ---------------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------------

/// How completely a drive has actually been indexed.
///
/// This exists to answer one question at the moment it is asked: *is this drive
/// finished, and can I unplug it?* The archive is built one drive at a time,
/// left connected until it is done — which at roughly a third of a photograph
/// per second can be a night or several days.
///
/// Getting that answer wrong in the reassuring direction is the expensive
/// failure. A drive unplugged at ninety per cent sits at ninety per cent
/// forever, and months later a photograph genuinely on drive 3 simply would not
/// be found — indistinguishable from it never having existed. So the summary
/// says "safe to unplug" only when there is nothing left to do.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DriveCoverage {
    pub drive_number: i64,
    pub drive_name: Option<String>,
    /// Photographs the last real scan found on the drive.
    pub discovered: i64,
    /// Photographs fully indexed.
    pub complete: i64,
    /// Discovered but not yet indexed.
    pub outstanding: i64,
    pub failed: i64,
    /// How the last scan ended, when it recorded an outcome.
    pub last_outcome: Option<String>,
    pub last_scan_at: Option<String>,

    // The two fields below are computed, and are *fields* rather than methods
    // on purpose. When they were methods the interface could not reach them, so
    // it reimplemented the rule in TypeScript — and got the never-scanned case
    // wrong, telling the owner a drive that had never been touched was
    // "Finished — all 0 photographs indexed. Safe to unplug." The Rust test for
    // that wording passed the whole time, because it tested code the screen was
    // not using. Serialising the answer leaves nothing to reimplement.
    /// The sentence to show. Never says "safe to unplug" unless it is.
    pub summary: String,
    /// Whether this drive can be disconnected without losing work.
    pub can_unplug: bool,
}

impl DriveCoverage {
    /// Fill in the computed fields. The only place the rule is written.
    fn finish(mut self) -> Self {
        let never_scanned = self.discovered == 0 && self.complete == 0;
        self.can_unplug = !never_scanned && !self.is_incomplete();
        self.summary = if never_scanned {
            "Never indexed — start a scan from Scan activity.".to_string()
        } else if self.is_incomplete() {
            format!(
                "{} of {} indexed ({:.0}%). {} still to do — leave this drive connected.",
                self.complete,
                self.discovered,
                self.percent(),
                self.outstanding
            )
        } else {
            format!(
                "Finished — all {} photographs indexed. Safe to unplug.",
                self.complete
            )
        };
        self
    }

    pub fn percent(&self) -> f64 {
        if self.discovered <= 0 {
            return if self.complete > 0 { 100.0 } else { 0.0 };
        }
        (self.complete as f64 / self.discovered as f64 * 100.0).min(100.0)
    }

    /// True when photographs were found on the drive and never indexed.
    ///
    /// The plain question the interface needs to ask: is there work left on
    /// this drive that will be missed unless it is plugged back in?
    pub fn is_incomplete(&self) -> bool {
        self.outstanding > 0
    }

}

/// Coverage for every registered drive, least complete first.
///
/// Ordered that way deliberately: the drive most in need of being plugged back
/// in should be the one at the top of the list.
pub fn drive_coverage(conn: &Connection) -> Result<Vec<DriveCoverage>> {
    let mut stmt = conn.prepare(
        "SELECT d.drive_number, d.friendly_name, d.last_scan_at,
                (SELECT count(*) FROM files f
                  WHERE f.drive_id = d.id AND f.status = 'complete'),
                (SELECT count(*) FROM files f
                  WHERE f.drive_id = d.id AND f.status = 'failed'),
                (SELECT sr.files_discovered FROM scan_runs sr
                  WHERE sr.drive_id = d.id AND sr.mode <> 'dry-run'
                  ORDER BY sr.started_at DESC LIMIT 1),
                (SELECT sr.outcome FROM scan_runs sr
                  WHERE sr.drive_id = d.id AND sr.mode <> 'dry-run'
                  ORDER BY sr.started_at DESC LIMIT 1)
           FROM drives d
          ORDER BY d.drive_number",
    )?;
    let rows = stmt.query_map([], |r| {
        let complete: i64 = r.get(3)?;
        let discovered: i64 = r.get::<_, Option<i64>>(5)?.unwrap_or(0);
        Ok(DriveCoverage {
            drive_number: r.get(0)?,
            drive_name: r.get(1)?,
            last_scan_at: r.get(2)?,
            complete,
            failed: r.get(4)?,
            // A rescan can discover fewer files than are catalogued (photographs
            // deleted from the drive), so this must never go negative.
            outstanding: (discovered - complete).max(0),
            discovered,
            last_outcome: r.get(6)?,
            summary: String::new(),
            can_unplug: false,
        }
        .finish())
    })?;
    let mut all: Vec<DriveCoverage> = rows.collect::<std::result::Result<_, _>>()?;
    all.sort_by(|a, b| {
        b.is_incomplete()
            .cmp(&a.is_incomplete())
            .then(a.percent().partial_cmp(&b.percent()).unwrap_or(std::cmp::Ordering::Equal))
    });
    Ok(all)
}

/// How long indexing a folder will take, so the time can be planned for.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexEstimate {
    pub files: u64,
    pub files_per_second: f64,
    pub hours: f64,
    /// True when this is unlikely to finish in one night.
    pub exceeds_one_night: bool,
    pub summary: String,
}

/// Beyond this a run spans more than one night, so the estimate is given in
/// days. Not a warning — a drive is left connected until it finishes, however
/// long that is — just the more readable unit.
pub const OVERNIGHT_HOURS: f64 = 10.0;

/// Throughput assumed before this catalogue has measured its own.
///
/// Measured on a real wedding drive: 0.27–0.36 files/sec, dominated by Vision
/// analysis. The lower end is used so an estimate errs towards warning.
pub const ASSUMED_FILES_PER_SECOND: f64 = 0.30;

/// Estimate how long indexing `file_count` photographs will take.
///
/// Throughput is taken from this catalogue's own history when it has any, so
/// the estimate reflects the machine it is running on rather than the one the
/// constant was measured on.
///
/// Phrased as a duration to plan around, not as a warning. The drive stays
/// connected until it finishes; the number is there so its owner knows whether
/// that is tonight or the rest of the week.
pub fn estimate_indexing(conn: &Connection, file_count: u64) -> IndexEstimate {
    let measured: Option<f64> = conn
        .query_row(
            "SELECT CAST(sum(files_done) AS REAL) /
                    NULLIF(sum(strftime('%s', ended_at) - strftime('%s', started_at)), 0)
               FROM scan_runs
              WHERE mode <> 'dry-run' AND ended_at IS NOT NULL AND files_done > 0",
            [],
            |r| r.get(0),
        )
        .ok()
        .flatten();

    let fps = measured.filter(|f| *f > 0.0).unwrap_or(ASSUMED_FILES_PER_SECOND);
    let hours = file_count as f64 / fps / 3600.0;
    let exceeds = hours > OVERNIGHT_HOURS;

    let summary = if file_count == 0 {
        "No photographs found here.".to_string()
    } else if hours >= 24.0 {
        format!(
            "{file_count} photographs, about {:.1} days. Leave the drive connected — \
             it keeps its place if interrupted.",
            hours / 24.0
        )
    } else if exceeds {
        format!(
            "{file_count} photographs, about {hours:.0} hours — more than one night. \
             Leave the drive connected until it finishes."
        )
    } else {
        format!("{file_count} photographs, about {hours:.1} hours.")
    };

    IndexEstimate { files: file_count, files_per_second: fps, hours, exceeds_one_night: exceeds, summary }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    fn drive(conn: &Connection, id: &str, number: i64, discovered: i64, complete: i64) {
        conn.execute(
            "INSERT INTO drives (id, drive_number, friendly_name, status, first_seen_at)
             VALUES (?1, ?2, ?3, 'offline', 'now')",
            rusqlite::params![id, number, format!("Drive {number}")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO roots (id, drive_id, relative_root, created_at)
             VALUES (?1, ?2, '', 'now')",
            rusqlite::params![format!("rt-{id}"), id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scan_runs (id, drive_id, drive_number, mode, scan_root, started_at,
                                    ended_at, outcome, files_discovered, files_done, files_failed)
             VALUES (?1, ?2, ?3, 'full', '/Volumes/x', '2026-01-01T00:00:00Z',
                     '2026-01-01T09:00:00Z', 'ok', ?4, ?5, 0)",
            rusqlite::params![format!("sr-{id}"), id, number, discovered, complete],
        )
        .unwrap();
        for i in 0..complete {
            conn.execute(
                "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                    source_mtime_ns, status, analysis_version, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?1, ?1, 1, 0, 'complete', 1, 'now', 'now')",
                rusqlite::params![format!("{id}-f{i}"), id, format!("rt-{id}")],
            )
            .unwrap();
        }
    }

    /// The failure this exists to prevent: a drive unplugged part-way through
    /// the night, left at seventy per cent, with nothing ever saying so.
    #[test]
    fn a_part_indexed_drive_says_so_and_sorts_first() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        drive(&conn, "d1", 1, 500, 500);   // finished
        drive(&conn, "d2", 2, 15000, 11000); // unplugged before it finished
        drive(&conn, "d3", 3, 800, 800);   // finished

        let all = drive_coverage(&conn).unwrap();
        assert_eq!(all.len(), 3);

        // The drive needing attention is first, not buried in drive-number order.
        assert_eq!(all[0].drive_number, 2);
        assert!(all[0].is_incomplete());
        assert_eq!(all[0].outstanding, 4000);
        assert!((all[0].percent() - 73.3).abs() < 0.5, "{}", all[0].percent());
        assert!(all[0].summary.contains("leave this drive connected"), "{}", all[0].summary);
        assert!(!all[0].can_unplug);

        // And the finished ones say so plainly.
        assert!(!all[1].is_incomplete());
        // "Safe to unplug" is the phrase that must never appear on unfinished work.
        assert!(all[1].summary.contains("Safe to unplug"), "{}", all[1].summary);
        assert!(all[1].can_unplug);
        assert!(!all[0].summary.contains("Safe to unplug"), "{}", all[0].summary);
    }

    /// Photographs deleted from a drive since the last scan must not read as
    /// negative outstanding work.
    #[test]
    fn fewer_files_on_disk_than_catalogued_is_not_negative_work() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        drive(&conn, "d1", 1, 100, 120);
        let all = drive_coverage(&conn).unwrap();
        assert_eq!(all[0].outstanding, 0);
        assert!(!all[0].is_incomplete());
        assert!(all[0].percent() <= 100.0);
    }

    #[test]
    fn a_never_indexed_drive_says_so() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives (id, drive_number, status, first_seen_at)
             VALUES ('d9', 9, 'offline', 'now')",
            [],
        )
        .unwrap();
        let all = drive_coverage(&conn).unwrap();
        // The bug this replaced: a drive that had never been touched reported
        // "Finished — all 0 photographs indexed. Safe to unplug."
        assert!(all[0].summary.starts_with("Never indexed"), "{}", all[0].summary);
        assert!(!all[0].summary.contains("Safe to unplug"), "{}", all[0].summary);
        assert!(!all[0].can_unplug, "a never-scanned drive is not finished");
    }

    /// The estimate exists so the time can be planned for, not to warn.
    #[test]
    fn warns_when_a_drive_will_not_finish_overnight() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();

        let short = estimate_indexing(&conn, 5_000);
        assert!(!short.exceeds_one_night);
        assert!(short.summary.contains("4.6 hours"), "{}", short.summary);

        let long = estimate_indexing(&conn, 20_000);
        assert!(long.exceeds_one_night);
        assert!(long.summary.contains("more than one night"), "{}", long.summary);
        assert!(long.summary.contains("until it finishes"), "{}", long.summary);

        // Past a day, days are the readable unit.
        let huge = estimate_indexing(&conn, 60_000);
        assert!(huge.summary.contains("days"), "{}", huge.summary);
        assert!(huge.summary.contains("keeps its place"), "{}", huge.summary);

        assert_eq!(estimate_indexing(&conn, 0).summary, "No photographs found here.");
    }

    /// The estimate must reflect this machine once it has evidence of its own.
    #[test]
    fn measured_throughput_replaces_the_assumed_rate() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        assert_eq!(
            estimate_indexing(&conn, 1000).files_per_second,
            ASSUMED_FILES_PER_SECOND
        );

        // A past run: 3600 files in one hour = 1.0 files/sec, far faster than
        // the assumed rate.
        drive(&conn, "d1", 1, 3600, 3600);
        let e = estimate_indexing(&conn, 3600);
        assert!(
            (e.files_per_second - 0.1111).abs() < 0.01,
            "expected the measured rate, got {}",
            e.files_per_second
        );
    }
}

// ---------------------------------------------------------------------------
// Live scan statistics
// ---------------------------------------------------------------------------

/// What a scan has produced so far, for the live dashboard.
///
/// Every figure here is counted from the catalogue rather than tracked in
/// memory, so it survives the app being closed and reopened mid-run and cannot
/// drift from what was actually written. The queries are all index-backed
/// counts; this is called once a second while a scan runs.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScanStats {
    pub drive_number: i64,
    /// Photographs catalogued from this drive.
    pub files: i64,
    /// Bytes of original photograph read, for a genuine throughput figure
    /// rather than one inferred from file counts.
    pub bytes: i64,
    pub faces: i64,
    pub tags: i64,
    pub people_recognised: i64,
    /// Counts by file extension, largest first.
    pub by_extension: Vec<(String, i64)>,
    /// The most recently catalogued photographs, newest first.
    pub recent: Vec<RecentFile>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecentFile {
    pub file_id: String,
    pub filename: String,
    pub relative_path: String,
    pub size_bytes: i64,
    pub faces: i64,
    /// The strongest label Vision gave it, when it gave one — what the
    /// photograph is *of*, which is the interesting part of a live feed.
    pub top_tag: Option<String>,
}

/// Statistics for one drive's catalogue as it stands right now.
pub fn scan_stats(conn: &Connection, drive_number: i64, recent_limit: usize) -> Result<ScanStats> {
    let mut stats = ScanStats { drive_number, ..Default::default() };

    let drive_id: Option<String> = conn
        .query_row(
            "SELECT id FROM drives WHERE drive_number = ?1",
            [drive_number],
            |r| r.get(0),
        )
        .ok();
    let Some(drive_id) = drive_id else { return Ok(stats) };

    (stats.files, stats.bytes) = conn.query_row(
        "SELECT count(*), COALESCE(sum(size_bytes), 0) FROM files
          WHERE drive_id = ?1 AND status = 'complete'",
        [&drive_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    stats.faces = conn
        .query_row(
            "SELECT count(*) FROM faces fa JOIN files f ON f.id = fa.file_id
              WHERE f.drive_id = ?1",
            [&drive_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    stats.tags = conn
        .query_row(
            "SELECT count(*) FROM file_tags ft JOIN files f ON f.id = ft.file_id
              WHERE f.drive_id = ?1",
            [&drive_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    stats.people_recognised = conn
        .query_row(
            "SELECT count(DISTINCT c.person_id)
               FROM face_clusters c
               JOIN faces fa ON fa.cluster_id = c.id
               JOIN files f ON f.id = fa.file_id
              WHERE f.drive_id = ?1 AND c.person_id IS NOT NULL",
            [&drive_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    {
        let mut stmt = conn.prepare(
            "SELECT COALESCE(lower(extension), '?') AS ext, count(*) AS n
               FROM files WHERE drive_id = ?1 AND status = 'complete'
              GROUP BY ext ORDER BY n DESC",
        )?;
        let rows = stmt.query_map([&drive_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        stats.by_extension = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    }

    {
        // Ordered by rowid rather than a timestamp: several photographs land in
        // the same second, and rowid preserves the order they were actually
        // written in.
        let mut stmt = conn.prepare(
            "SELECT f.id, f.filename, f.relative_path, f.size_bytes,
                    (SELECT count(*) FROM faces fa WHERE fa.file_id = f.id),
                    (SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
                      WHERE ft.file_id = f.id
                      ORDER BY ft.confidence DESC LIMIT 1)
               FROM files f
              WHERE f.drive_id = ?1 AND f.status = 'complete'
              ORDER BY f.rowid DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![&drive_id, recent_limit as i64], |r| {
            Ok(RecentFile {
                file_id: r.get(0)?,
                filename: r.get(1)?,
                relative_path: r.get(2)?,
                size_bytes: r.get(3)?,
                faces: r.get(4)?,
                top_tag: r.get(5)?,
            })
        })?;
        stats.recent = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    }

    Ok(stats)
}

#[cfg(test)]
mod scan_stats_tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    #[test]
    fn reports_what_a_scan_has_produced() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives (id, drive_number, status, first_seen_at)
             VALUES ('d1', 3, 'online', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO roots (id, drive_id, relative_root, created_at)
             VALUES ('rt1','d1','','now')",
            [],
        )
        .unwrap();
        for (i, ext) in ["jpg", "jpg", "jpg", "psd", "png"].iter().enumerate() {
            conn.execute(
                "INSERT INTO files (id, drive_id, root_id, relative_path, filename, extension,
                                    size_bytes, source_mtime_ns, status, analysis_version,
                                    created_at, updated_at)
                 VALUES (?1,'d1','rt1',?2,?2,?3,?4,0,'complete',1,'now','now')",
                rusqlite::params![format!("f{i}"), format!("shot{i}.{ext}"), ext, 1_000_000],
            )
            .unwrap();
        }

        let stats = scan_stats(&conn, 3, 3).unwrap();
        assert_eq!(stats.files, 5);
        assert_eq!(stats.bytes, 5_000_000);
        // Largest group first, so a breakdown reads without sorting by eye.
        assert_eq!(stats.by_extension[0], ("jpg".to_string(), 3));
        // The feed shows the newest, capped at what was asked for.
        assert_eq!(stats.recent.len(), 3);
        assert_eq!(stats.recent[0].filename, "shot4.png");
    }

    /// A drive that has not been scanned must give zeroes, not an error — the
    /// dashboard asks for these before anything exists.
    #[test]
    fn an_unscanned_drive_reports_nothing_rather_than_failing() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        let stats = scan_stats(&conn, 99, 10).unwrap();
        assert_eq!(stats.files, 0);
        assert!(stats.by_extension.is_empty());
        assert!(stats.recent.is_empty());
    }
}

/// Result of looking for names in text already read from photographs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NameScan {
    /// Photographs whose text was examined.
    pub examined: i64,
    /// Photographs that gained at least one name tag.
    pub tagged: i64,
    /// Names found, most photographs first.
    pub names: Vec<TagCount>,
}

/// Find brand names in the OCR text already stored for each photograph.
///
/// This is a backfill, not a rescan. Every photograph AtlasDrive has indexed
/// already has its text stored, so brands can be recognised on a drive that is
/// sitting in a drawer — no original is opened and nothing is re-read.
///
/// Safe to run repeatedly: tags are inserted with `INSERT OR IGNORE`, so a
/// second pass over unchanged text changes nothing.
pub fn scan_for_names(conn: &Connection, drive_number: Option<i64>) -> Result<NameScan> {
    let now = crate::util::now_iso8601();
    let mut out = NameScan::default();

    let sql = format!(
        "SELECT sa.file_id, sa.ocr_text
           FROM scene_analysis sa
           JOIN files f  ON f.id = sa.file_id
           JOIN drives d ON d.id = f.drive_id
          WHERE sa.ocr_text IS NOT NULL AND sa.ocr_text <> ''
            AND f.status = 'complete'{}",
        match drive_number {
            Some(_) => " AND d.drive_number = ?1",
            None => "",
        }
    );

    let rows: Vec<(String, String)> = {
        let mut stmt = conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row| Ok((r.get(0)?, r.get(1)?));
        match drive_number {
            Some(dn) => stmt
                .query_map([dn], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            None => stmt.query_map([], map)?.collect::<std::result::Result<Vec<_>, _>>()?,
        }
    };

    let mut counts: std::collections::BTreeMap<String, i64> = Default::default();
    let tx = conn.unchecked_transaction()?;
    for (file_id, text) in rows {
        out.examined += 1;
        let hits = crate::ai::names::detect(&text);
        if hits.is_empty() {
            continue;
        }
        out.tagged += 1;
        for hit in hits {
            let tag_id = crate::util::new_uuid();
            tx.execute(
                "INSERT OR IGNORE INTO tags (id, name, tag_type, created_at)
                 VALUES (?1, ?2, 'automatic', ?3)",
                rusqlite::params![tag_id, hit.tag, now],
            )?;
            let real_id: String =
                tx.query_row("SELECT id FROM tags WHERE name = ?1", [&hit.tag], |r| r.get(0))?;
            tx.execute(
                "INSERT OR IGNORE INTO file_tags (file_id, tag_id, confidence, source, created_at)
                 VALUES (?1, ?2, 0.9, 'name', ?3)",
                rusqlite::params![file_id, real_id, now],
            )?;
            *counts.entry(hit.tag).or_default() += 1;
        }
    }
    tx.commit()?;

    out.names = counts.into_iter().map(|(tag, count)| TagCount { tag, count }).collect();
    out.names.sort_by(|a, b| b.count.cmp(&a.count).then(a.tag.cmp(&b.tag)));
    Ok(out)
}

#[cfg(test)]
mod name_scan_tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    fn catalogue_with_text(texts: &[(&str, &str)]) -> Connection {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives(id, drive_number, volume_name, status, first_seen_at)
             VALUES ('d1', 2, 'Vol', 'online', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO roots(id, drive_id, relative_root, created_at)
             VALUES ('r1','d1','','2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        for (fid, text) in texts {
            conn.execute(
                "INSERT INTO files(id, drive_id, root_id, relative_path, filename, size_bytes,
                                   source_mtime_ns, status, created_at, updated_at)
                 VALUES (?1,'d1','r1',?2,?2,1,1,'complete','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                rusqlite::params![fid, format!("{fid}.jpg")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO scene_analysis(file_id, ocr_text, created_at)
                 VALUES (?1, ?2, '2026-01-01T00:00:00Z')",
                rusqlite::params![fid, text],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn finds_brands_in_text_already_read_from_the_photographs() {
        let conn = catalogue_with_text(&[
            ("f1", "COCA-COLA and a bottle of PERONI"),
            ("f2", "Welcome to TESCO Extra"),
            ("f3", "nothing recognisable here at all"),
        ]);
        let report = scan_for_names(&conn, None).unwrap();
        assert_eq!(report.examined, 3);
        assert_eq!(report.tagged, 2, "f3 has no brand in it");
        let names: Vec<_> = report.names.iter().map(|b| b.tag.as_str()).collect();
        assert!(names.contains(&"coca-cola"));
        assert!(names.contains(&"peroni"));
        assert!(names.contains(&"tesco"));
    }

    /// The tags must actually be searchable afterwards, not merely counted in
    /// a report — the report is the thing most likely to be right while the
    /// catalogue stays empty.
    #[test]
    fn the_brands_land_in_the_catalogue_as_tags() {
        let conn = catalogue_with_text(&[("f1", "Guinness on tap")]);
        scan_for_names(&conn, None).unwrap();
        let tags = all_tags(&conn, 60).unwrap();
        assert!(tags.iter().any(|t| t.tag == "guinness" && t.count == 1), "{tags:?}");

        let source: String = conn
            .query_row(
                "SELECT ft.source FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
                  WHERE t.name = 'guinness'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source, "name", "provenance must say where the tag came from");
    }

    /// Running it twice must not double-count or duplicate tags: the owner
    /// will run this after every drive.
    #[test]
    fn running_it_again_changes_nothing() {
        let conn = catalogue_with_text(&[("f1", "ASDA car park")]);
        let first = scan_for_names(&conn, None).unwrap();
        let second = scan_for_names(&conn, None).unwrap();
        assert_eq!(first.tagged, second.tagged);

        let rows: i64 = conn
            .query_row("SELECT count(*) FROM file_tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "a second pass must not add a duplicate row");
        let tags: i64 = conn.query_row("SELECT count(*) FROM tags", [], |r| r.get(0)).unwrap();
        assert_eq!(tags, 1, "a second pass must not add a duplicate tag");
    }

    #[test]
    fn a_drive_can_be_done_on_its_own() {
        let conn = catalogue_with_text(&[("f1", "PEPSI MAX")]);
        assert_eq!(scan_for_names(&conn, Some(2)).unwrap().examined, 1);
        // A drive that holds nothing is not an error.
        assert_eq!(scan_for_names(&conn, Some(99)).unwrap().examined, 0);
    }
}
