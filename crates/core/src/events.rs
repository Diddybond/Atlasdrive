//! Grouping photographs into the events they were taken at.
//!
//! A photograph archive is event-shaped. The useful unit of recall is "that
//! wedding in May" or "the Crown Parents shoot", not a date range typed into a
//! filter. This module proposes those groups from capture times and lets a
//! human name, merge, split and correct them.
//!
//! # Proposing, not deciding
//!
//! Events are proposed and then confirmed, in the same shape as face
//! suggestions and for the same reason: the app can see that forty photographs
//! were taken across one Saturday, but only the owner knows whether that was a
//! wedding, a christening, or two unrelated things that happened to share a
//! day. A proposal that has not been accepted is marked `proposed` and must not
//! be presented as fact.
//!
//! # Where the gap threshold comes from
//!
//! Photographs cluster by shoot with long empty stretches between. The split is
//! made on a time gap rather than on calendar days, because a wedding routinely
//! runs past midnight and a calendar-day rule would cut the evening reception
//! off from the ceremony — the single most obvious way to get this wrong.
//!
//! [`DEFAULT_GAP_HOURS`] is deliberately generous. Over-merging is cheap to fix
//! (split it) and under-merging is not (the owner has to find and merge
//! fragments scattered through a list), so the bias is towards keeping a day
//! together.
//!
//! # Clients
//!
//! An event optionally carries a client name, so several shoots for the same
//! people can be gathered without forcing them into one event. It is a name the
//! owner types, not an entity with identity — this is a photograph catalogue,
//! not a CRM.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Gap between consecutive photographs that starts a new event.
///
/// Ten hours keeps a long wedding day — ceremony through to a late reception —
/// in one event, while still separating shoots on consecutive days, which are
/// normally more than ten hours apart at the boundary (an evening finish and a
/// morning start).
pub const DEFAULT_GAP_HOURS: f64 = 10.0;

/// How wide a date estimate may be and still mean "when this was taken".
///
/// This guard matters more than the gap threshold. A date estimate is a *range*:
/// a digital photograph carries an EXIF capture time where earliest and latest
/// are the same instant, but a scanned print may only be placed as "sometime
/// between 1985 and 1989". Clustering on the start of a four-year range would
/// clump hundreds of unrelated prints into one fabricated event, and would do it
/// silently — the photographs would look grouped, and the grouping would mean
/// nothing.
///
/// Two days rather than one, so a photograph dated only to a calendar day still
/// groups, while anything vaguer is set aside and reported instead of guessed at.
pub const MAX_DATE_SPAN_HOURS: f64 = 48.0;

/// Below this many photographs a cluster is not worth proposing as an event.
///
/// Stray shots — a test frame, a photograph of a parking sign — would otherwise
/// each become their own "event" and bury the real ones.
pub const MIN_EVENT_PHOTOS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub name: Option<String>,
    pub client: Option<String>,
    pub earliest_date: Option<String>,
    pub latest_date: Option<String>,
    pub status: String,
    pub photo_count: i64,
}

impl Event {
    /// What to show when the event has no name yet.
    pub fn display_name(&self) -> String {
        if let Some(n) = &self.name {
            if !n.trim().is_empty() {
                return n.clone();
            }
        }
        match (&self.earliest_date, &self.latest_date) {
            (Some(a), Some(b)) if a[..10.min(a.len())] == b[..10.min(b.len())] => {
                format!("{} photographs on {}", self.photo_count, &a[..10.min(a.len())])
            }
            (Some(a), Some(b)) => format!(
                "{} photographs, {} to {}",
                self.photo_count,
                &a[..10.min(a.len())],
                &b[..10.min(b.len())]
            ),
            _ => format!("{} photographs, date unknown", self.photo_count),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProposeReport {
    pub proposed: u64,
    pub photos_grouped: u64,
    /// Photographs left out because their cluster was too small.
    pub photos_skipped: u64,
    /// Photographs with no usable date, which cannot be clustered at all.
    pub photos_undated: u64,
    /// Photographs whose date is only known to within a wide range — a scanned
    /// print placed in a decade, say. Grouping these by time would invent
    /// events rather than find them.
    pub photos_imprecise: u64,
}

pub struct EventRepo<'a> {
    conn: &'a Connection,
}

impl<'a> EventRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Cluster undated-into-events photographs by capture time and propose the
    /// groups. Existing events are left alone; only photographs not already in
    /// an event are considered, so this is safe to re-run after a new scan.
    pub fn propose(&self, gap_hours: f64) -> Result<ProposeReport> {
        let mut report = ProposeReport::default();

        // The best date available: a correction the owner made outranks an
        // estimate, which is what `is_user_confirmed` records.
        let mut stmt = self.conn.prepare(
            "SELECT f.id, de.earliest_date, de.latest_date
               FROM files f
               JOIN date_estimates de ON de.file_id = f.id
              WHERE f.status = 'complete'
                AND de.earliest_date IS NOT NULL
                AND NOT EXISTS (SELECT 1 FROM event_files ef WHERE ef.file_id = f.id)
              ORDER BY de.earliest_date",
        )?;
        let candidates: Vec<(String, String, Option<String>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;

        // Set aside anything whose date is only known to within a wide range.
        // See MAX_DATE_SPAN_HOURS: these are reported, never guessed at.
        let max_span = (MAX_DATE_SPAN_HOURS * 3600.0) as i64;
        let mut rows: Vec<(String, String)> = Vec::with_capacity(candidates.len());
        for (file_id, earliest, latest) in candidates {
            let span = match (parse_epoch(&earliest), latest.as_deref().and_then(parse_epoch)) {
                (Some(a), Some(b)) => b - a,
                // No latest bound recorded: treat the earliest as exact, which
                // is how a plain EXIF timestamp is stored.
                (Some(_), None) => 0,
                _ => 0,
            };
            if span > max_span {
                report.photos_imprecise += 1;
                continue;
            }
            rows.push((file_id, earliest));
        }

        report.photos_undated = self.conn.query_row(
            "SELECT count(*) FROM files f
              WHERE f.status='complete'
                AND NOT EXISTS (SELECT 1 FROM event_files ef WHERE ef.file_id = f.id)
                AND NOT EXISTS (SELECT 1 FROM date_estimates de
                                 WHERE de.file_id = f.id AND de.earliest_date IS NOT NULL)",
            [],
            |r| r.get(0),
        )?;

        let gap_seconds = (gap_hours * 3600.0) as i64;
        let mut cluster: Vec<(String, String)> = Vec::new();
        let mut previous: Option<i64> = None;

        for (file_id, date) in rows {
            let ts = parse_epoch(&date);
            let split = match (previous, ts) {
                (Some(prev), Some(now)) => now - prev > gap_seconds,
                // A date that will not parse cannot be compared, so it starts a
                // new cluster rather than silently joining the previous one.
                _ => !cluster.is_empty() && ts.is_none(),
            };
            if split && !cluster.is_empty() {
                self.flush(&mut cluster, &mut report)?;
            }
            if ts.is_some() {
                previous = ts;
            }
            cluster.push((file_id, date));
        }
        self.flush(&mut cluster, &mut report)?;

        Ok(report)
    }

    fn flush(
        &self,
        cluster: &mut Vec<(String, String)>,
        report: &mut ProposeReport,
    ) -> Result<()> {
        if cluster.len() < MIN_EVENT_PHOTOS {
            report.photos_skipped += cluster.len() as u64;
            cluster.clear();
            return Ok(());
        }
        let now = crate::util::now_iso8601();
        let id = crate::util::new_uuid();
        let earliest = cluster.first().map(|(_, d)| d.clone());
        let latest = cluster.last().map(|(_, d)| d.clone());

        self.conn.execute(
            "INSERT INTO events (id, status, earliest_date, latest_date, created_at, updated_at)
             VALUES (?1, 'proposed', ?2, ?3, ?4, ?4)",
            rusqlite::params![id, earliest, latest, now],
        )?;
        for (file_id, _) in cluster.iter() {
            self.conn.execute(
                "INSERT OR IGNORE INTO event_files (event_id, file_id, confirmed, created_at)
                 VALUES (?1, ?2, 0, ?3)",
                rusqlite::params![id, file_id, now],
            )?;
        }
        report.proposed += 1;
        report.photos_grouped += cluster.len() as u64;
        cluster.clear();
        Ok(())
    }

    /// Name an event, optionally recording the client it was shot for.
    ///
    /// Naming is what turns a proposal into a fact, so it confirms the
    /// membership at the same time.
    pub fn name_event(&self, event_id: &str, name: &str, client: Option<&str>) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::InvalidArgs("an event needs a name".into()));
        }
        let affected = self.conn.execute(
            "UPDATE events SET name = ?2, client = ?3, status = 'named', updated_at = ?4
              WHERE id = ?1",
            rusqlite::params![
                event_id,
                name,
                client.map(str::trim).filter(|c| !c.is_empty()),
                crate::util::now_iso8601()
            ],
        )?;
        if affected == 0 {
            return Err(Error::InvalidArgs(format!("no event {event_id}")));
        }
        self.conn.execute(
            "UPDATE event_files SET confirmed = 1 WHERE event_id = ?1",
            [event_id],
        )?;
        Ok(())
    }

    /// Fold `from` into `into`, keeping the destination's name and client.
    pub fn merge(&self, into: &str, from: &str) -> Result<u64> {
        if into == from {
            return Err(Error::InvalidArgs("cannot merge an event into itself".into()));
        }
        let now = crate::util::now_iso8601();
        let moved = self.conn.execute(
            "INSERT OR IGNORE INTO event_files (event_id, file_id, confirmed, created_at)
             SELECT ?1, file_id, confirmed, ?3 FROM event_files WHERE event_id = ?2",
            rusqlite::params![into, from, now],
        )?;
        self.conn.execute("DELETE FROM events WHERE id = ?1", [from])?;
        self.refresh_bounds(into)?;
        Ok(moved as u64)
    }

    /// Split the photographs from `at_date` onwards into a new event.
    ///
    /// The common correction: two shoots on one day that the gap rule kept
    /// together.
    pub fn split(&self, event_id: &str, at_date: &str) -> Result<String> {
        let new_id = crate::util::new_uuid();
        let now = crate::util::now_iso8601();

        let mut stmt = self.conn.prepare(
            "SELECT ef.file_id
               FROM event_files ef
               JOIN date_estimates de ON de.file_id = ef.file_id
              WHERE ef.event_id = ?1 AND de.earliest_date >= ?2",
        )?;
        let moving: Vec<String> = stmt
            .query_map(rusqlite::params![event_id, at_date], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        drop(stmt);
        if moving.is_empty() {
            return Err(Error::InvalidArgs(
                "nothing falls on or after that date in this event".into(),
            ));
        }

        self.conn.execute(
            "INSERT INTO events (id, status, created_at, updated_at)
             VALUES (?1, 'proposed', ?2, ?2)",
            rusqlite::params![new_id, now],
        )?;
        for file_id in &moving {
            self.conn.execute(
                "UPDATE event_files SET event_id = ?1 WHERE event_id = ?2 AND file_id = ?3",
                rusqlite::params![new_id, event_id, file_id],
            )?;
        }
        self.refresh_bounds(event_id)?;
        self.refresh_bounds(&new_id)?;
        Ok(new_id)
    }

    /// Remove an event. Its photographs stay in the catalogue and become
    /// available to be proposed again.
    pub fn forget(&self, event_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM event_files WHERE event_id = ?1", [event_id])?;
        self.conn.execute("DELETE FROM events WHERE id = ?1", [event_id])?;
        Ok(())
    }

    /// Move one photograph into an event, correcting a bad grouping.
    pub fn assign(&self, event_id: &str, file_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM event_files WHERE file_id = ?1", [file_id])?;
        self.conn.execute(
            "INSERT INTO event_files (event_id, file_id, confirmed, created_at)
             VALUES (?1, ?2, 1, ?3)",
            rusqlite::params![event_id, file_id, crate::util::now_iso8601()],
        )?;
        self.refresh_bounds(event_id)?;
        Ok(())
    }

    /// Recompute an event's date bounds from its current membership.
    fn refresh_bounds(&self, event_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE events SET
                earliest_date = (SELECT min(de.earliest_date) FROM event_files ef
                                   JOIN date_estimates de ON de.file_id = ef.file_id
                                  WHERE ef.event_id = events.id),
                latest_date   = (SELECT max(de.earliest_date) FROM event_files ef
                                   JOIN date_estimates de ON de.file_id = ef.file_id
                                  WHERE ef.event_id = events.id),
                updated_at    = ?2
              WHERE id = ?1",
            rusqlite::params![event_id, crate::util::now_iso8601()],
        )?;
        Ok(())
    }

    /// Every event, newest first. `status` filters when given.
    pub fn list(&self, status: Option<&str>) -> Result<Vec<Event>> {
        let mut sql = String::from(
            "SELECT e.id, e.name, e.client, e.earliest_date, e.latest_date, e.status,
                    (SELECT count(*) FROM event_files ef WHERE ef.event_id = e.id)
               FROM events e",
        );
        if status.is_some() {
            sql.push_str(" WHERE e.status = ?1");
        }
        // Undated events last rather than first, where a NULL would sort them.
        sql.push_str(" ORDER BY e.earliest_date IS NULL, e.earliest_date DESC");

        let mut stmt = self.conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row| -> rusqlite::Result<Event> {
            Ok(Event {
                id: r.get(0)?,
                name: r.get(1)?,
                client: r.get(2)?,
                earliest_date: r.get(3)?,
                latest_date: r.get(4)?,
                status: r.get(5)?,
                photo_count: r.get(6)?,
            })
        };
        let events: Vec<Event> = match status {
            Some(s) => stmt
                .query_map([s], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            None => stmt
                .query_map([], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        };
        Ok(events)
    }

    /// Events shot for a client, newest first.
    pub fn for_client(&self, client: &str) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.name, e.client, e.earliest_date, e.latest_date, e.status,
                    (SELECT count(*) FROM event_files ef WHERE ef.event_id = e.id)
               FROM events e
              WHERE e.client = ?1 COLLATE NOCASE
              ORDER BY e.earliest_date IS NULL, e.earliest_date DESC",
        )?;
        let rows = stmt.query_map([client], |r| {
            Ok(Event {
                id: r.get(0)?,
                name: r.get(1)?,
                client: r.get(2)?,
                earliest_date: r.get(3)?,
                latest_date: r.get(4)?,
                status: r.get(5)?,
                photo_count: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every client name in use, with how many shoots each has.
    pub fn clients(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT client, count(*) FROM events
              WHERE client IS NOT NULL AND trim(client) <> ''
              GROUP BY client COLLATE NOCASE
              ORDER BY count(*) DESC, client",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The file ids in an event, in capture order.
    pub fn files(&self, event_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT ef.file_id
               FROM event_files ef
               LEFT JOIN date_estimates de ON de.file_id = ef.file_id
              WHERE ef.event_id = ?1
              ORDER BY de.earliest_date",
        )?;
        let rows = stmt.query_map([event_id], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The next proposal awaiting a decision, largest first.
    ///
    /// Largest first because a big group is both the most valuable to name and
    /// the easiest to recognise.
    pub fn next_proposal(&self) -> Result<Option<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.id, e.name, e.client, e.earliest_date, e.latest_date, e.status,
                    (SELECT count(*) FROM event_files ef WHERE ef.event_id = e.id) AS n
               FROM events e
              WHERE e.status = 'proposed'
              ORDER BY n DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], |r| {
            Ok(Event {
                id: r.get(0)?,
                name: r.get(1)?,
                client: r.get(2)?,
                earliest_date: r.get(3)?,
                latest_date: r.get(4)?,
                status: r.get(5)?,
                photo_count: r.get(6)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }
}

/// Parse an ISO date or datetime into epoch seconds.
///
/// Dates come from EXIF, from filenames and from the owner's corrections, so
/// the forms vary. A value that will not parse returns `None` and is treated as
/// a cluster boundary rather than being guessed at.
fn parse_epoch(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 10 {
        return None;
    }
    let (y, m, d) = (
        s[0..4].parse::<i64>().ok()?,
        s[5..7].parse::<i64>().ok()?,
        s[8..10].parse::<i64>().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days since a fixed civil epoch (Howard Hinnant's algorithm). Avoids
    // pulling a date library in for what is a subtraction.
    let y_adj = if m <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    let mut seconds = days * 86_400;
    // An optional time, in either "T" or space separated form.
    if s.len() >= 19 {
        let t = &s[11..19];
        if let (Ok(hh), Ok(mm), Ok(ss)) = (
            t[0..2].parse::<i64>(),
            t[3..5].parse::<i64>(),
            t[6..8].parse::<i64>(),
        ) {
            seconds += hh * 3600 + mm * 60 + ss;
        }
    }
    Some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    fn catalogue() -> Connection {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives (id, drive_number, status, first_seen_at)
             VALUES ('d1', 1, 'online', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO roots (id, drive_id, relative_root, created_at)
             VALUES ('rt1','d1','','now')",
            [],
        )
        .unwrap();
        conn
    }

    /// Add a photograph taken at `when` (an ISO datetime).
    fn photo(conn: &Connection, id: &str, when: &str) {
        conn.execute(
            "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                source_mtime_ns, status, analysis_version, created_at, updated_at)
             VALUES (?1,'d1','rt1',?1,?1,1,0,'complete',1,'now','now')",
            [id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO date_estimates (file_id, earliest_date, latest_date, confidence,
                                         method_version, evidence_json, is_user_confirmed,
                                         created_at, updated_at)
             VALUES (?1, ?2, ?2, 0.9, 1, '[]', 0, 'now', 'now')",
            rusqlite::params![id, when],
        )
        .unwrap();
        // The full-text index is populated by the pipeline rather than by a
        // trigger, so a fixture that skips it is not searchable — which is
        // exactly what caught this test out the first time.
        conn.execute(
            "INSERT INTO files_fts (file_id, filename, relative_path, tags, ocr_text, description)
             VALUES (?1, ?1, ?1, 'photograph', '', '')",
            [id],
        )
        .unwrap();
    }

    fn shoot(conn: &Connection, prefix: &str, day: &str, start_hour: u32, count: usize) {
        for i in 0..count {
            let h = start_hour + (i as u32 / 10);
            photo(
                conn,
                &format!("{prefix}{i:03}"),
                &format!("{day}T{:02}:{:02}:00", h.min(23), (i % 60)),
            );
        }
    }

    #[test]
    fn separate_days_become_separate_events() {
        let conn = catalogue();
        shoot(&conn, "a", "2026-05-30", 10, 20);
        shoot(&conn, "b", "2026-06-14", 11, 15);
        let repo = EventRepo::new(&conn);

        let report = repo.propose(DEFAULT_GAP_HOURS).unwrap();
        assert_eq!(report.proposed, 2);
        assert_eq!(report.photos_grouped, 35);

        let events = repo.list(None).unwrap();
        assert_eq!(events.len(), 2);
        // Newest first.
        assert!(events[0].earliest_date.as_ref().unwrap().starts_with("2026-06-14"));
    }

    /// The case most likely to be got wrong: a wedding that runs past midnight
    /// must stay one event, not be cut in half by the calendar.
    #[test]
    fn a_wedding_running_past_midnight_stays_one_event() {
        let conn = catalogue();
        // Ceremony and reception through the evening...
        shoot(&conn, "day", "2026-05-30", 13, 30);
        // ...and the last dances after midnight, three hours later.
        photo(&conn, "late1", "2026-05-31T00:15:00");
        photo(&conn, "late2", "2026-05-31T00:40:00");
        photo(&conn, "late3", "2026-05-31T01:05:00");
        photo(&conn, "late4", "2026-05-31T01:20:00");
        photo(&conn, "late5", "2026-05-31T01:30:00");

        let repo = EventRepo::new(&conn);
        let report = repo.propose(DEFAULT_GAP_HOURS).unwrap();

        assert_eq!(report.proposed, 1, "the night must not be split from the day");
        assert_eq!(report.photos_grouped, 35);
    }

    #[test]
    fn strays_are_not_promoted_into_events() {
        let conn = catalogue();
        shoot(&conn, "real", "2026-05-30", 10, 20);
        // Two test frames a month later.
        photo(&conn, "stray1", "2026-07-01T09:00:00");
        photo(&conn, "stray2", "2026-07-01T09:05:00");

        let repo = EventRepo::new(&conn);
        let report = repo.propose(DEFAULT_GAP_HOURS).unwrap();
        assert_eq!(report.proposed, 1);
        assert_eq!(report.photos_skipped, 2);
    }

    #[test]
    fn naming_confirms_the_grouping_and_records_the_client() {
        let conn = catalogue();
        shoot(&conn, "a", "2026-05-30", 12, 12);
        let repo = EventRepo::new(&conn);
        repo.propose(DEFAULT_GAP_HOURS).unwrap();

        let proposal = repo.next_proposal().unwrap().expect("a proposal");
        assert_eq!(proposal.status, "proposed");
        assert!(proposal.name.is_none());
        // An unnamed event still describes itself usefully.
        assert!(proposal.display_name().contains("2026-05-30"), "{}", proposal.display_name());

        repo.name_event(&proposal.id, "Aimee & Kent wedding", Some("Aimee Kanovan")).unwrap();

        let named = repo.list(Some("named")).unwrap();
        assert_eq!(named.len(), 1);
        assert_eq!(named[0].display_name(), "Aimee & Kent wedding");
        assert_eq!(named[0].client.as_deref(), Some("Aimee Kanovan"));

        // Membership is now fact, not proposal.
        let unconfirmed: i64 = conn
            .query_row("SELECT count(*) FROM event_files WHERE confirmed = 0", [], |r| r.get(0))
            .unwrap();
        assert_eq!(unconfirmed, 0);
        assert!(repo.next_proposal().unwrap().is_none());
    }

    /// The point of a client: several shoots for the same people, gathered
    /// without being forced into one event.
    #[test]
    fn several_shoots_gather_under_one_client() {
        let conn = catalogue();
        shoot(&conn, "eng", "2026-02-14", 11, 10);
        shoot(&conn, "wed", "2026-05-30", 12, 20);
        shoot(&conn, "anv", "2026-09-01", 14, 8);

        let repo = EventRepo::new(&conn);
        repo.propose(DEFAULT_GAP_HOURS).unwrap();
        let all = repo.list(None).unwrap();
        assert_eq!(all.len(), 3);

        for (event, name) in all.iter().zip(["Anniversary", "Wedding", "Engagement"]) {
            repo.name_event(&event.id, name, Some("Aimee & Kent")).unwrap();
        }

        let theirs = repo.for_client("Aimee & Kent").unwrap();
        assert_eq!(theirs.len(), 3);
        // Client lookup is case-insensitive; nobody types a name the same way twice.
        assert_eq!(repo.for_client("aimee & kent").unwrap().len(), 3);

        let clients = repo.clients().unwrap();
        assert_eq!(clients, vec![("Aimee & Kent".to_string(), 3)]);
    }

    /// Two shoots on one day is the expected correction, since the gap rule
    /// deliberately errs towards keeping a day together.
    #[test]
    fn one_event_can_be_split_in_two() {
        let conn = catalogue();
        // Morning christening and an afternoon portrait session, four hours
        // apart — inside the gap, so proposed as one.
        shoot(&conn, "am", "2026-04-12", 9, 10);
        shoot(&conn, "pm", "2026-04-12", 15, 10);

        let repo = EventRepo::new(&conn);
        repo.propose(DEFAULT_GAP_HOURS).unwrap();
        assert_eq!(repo.list(None).unwrap().len(), 1);

        let event = repo.list(None).unwrap().remove(0);
        let new_id = repo.split(&event.id, "2026-04-12T15:00:00").unwrap();

        let after = repo.list(None).unwrap();
        assert_eq!(after.len(), 2);
        assert_eq!(repo.files(&new_id).unwrap().len(), 10);
        assert_eq!(repo.files(&event.id).unwrap().len(), 10);

        // Bounds were recomputed, not left describing the old membership.
        let split_off = after.iter().find(|e| e.id == new_id).unwrap();
        assert!(split_off.earliest_date.as_ref().unwrap().contains("15:"));
    }

    #[test]
    fn two_events_can_be_merged() {
        let conn = catalogue();
        shoot(&conn, "a", "2026-03-01", 10, 8);
        shoot(&conn, "b", "2026-03-05", 10, 8);
        let repo = EventRepo::new(&conn);
        repo.propose(DEFAULT_GAP_HOURS).unwrap();

        let events = repo.list(None).unwrap();
        assert_eq!(events.len(), 2);
        let (keep, absorb) = (&events[0], &events[1]);
        repo.name_event(&keep.id, "Two-day shoot", None).unwrap();

        let moved = repo.merge(&keep.id, &absorb.id).unwrap();
        assert_eq!(moved, 8);

        let after = repo.list(None).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].photo_count, 16);
        assert_eq!(after[0].name.as_deref(), Some("Two-day shoot"));
        // Bounds now span both days.
        assert!(after[0].earliest_date.as_ref().unwrap().starts_with("2026-03-01"));
        assert!(after[0].latest_date.as_ref().unwrap().starts_with("2026-03-05"));
    }

    /// Re-running after a new drive is scanned must not disturb what is already
    /// named, and must pick up the new photographs.
    #[test]
    fn proposing_again_leaves_named_events_alone() {
        let conn = catalogue();
        shoot(&conn, "a", "2026-05-30", 12, 12);
        let repo = EventRepo::new(&conn);
        repo.propose(DEFAULT_GAP_HOURS).unwrap();
        let first = repo.list(None).unwrap().remove(0);
        repo.name_event(&first.id, "Wedding", Some("Smith")).unwrap();

        // A later scan brings in another shoot.
        shoot(&conn, "b", "2026-08-15", 10, 10);
        let report = repo.propose(DEFAULT_GAP_HOURS).unwrap();
        assert_eq!(report.proposed, 1, "only the new photographs are grouped");

        let all = repo.list(None).unwrap();
        assert_eq!(all.len(), 2);
        let kept = all.iter().find(|e| e.id == first.id).unwrap();
        assert_eq!(kept.name.as_deref(), Some("Wedding"));
        assert_eq!(kept.photo_count, 12);
    }

    #[test]
    fn forgetting_an_event_returns_its_photographs() {
        let conn = catalogue();
        shoot(&conn, "a", "2026-05-30", 12, 10);
        let repo = EventRepo::new(&conn);
        repo.propose(DEFAULT_GAP_HOURS).unwrap();
        let event = repo.list(None).unwrap().remove(0);

        repo.forget(&event.id).unwrap();
        assert!(repo.list(None).unwrap().is_empty());

        // The photographs are still catalogued and can be proposed again.
        let report = repo.propose(DEFAULT_GAP_HOURS).unwrap();
        assert_eq!(report.proposed, 1);
        assert_eq!(report.photos_grouped, 10);
    }

    #[test]
    fn a_photograph_can_be_moved_between_events() {
        let conn = catalogue();
        shoot(&conn, "a", "2026-03-01", 10, 8);
        shoot(&conn, "b", "2026-03-20", 10, 8);
        let repo = EventRepo::new(&conn);
        repo.propose(DEFAULT_GAP_HOURS).unwrap();
        let events = repo.list(None).unwrap();
        let (first, second) = (&events[0], &events[1]);

        let moving = repo.files(&second.id).unwrap().remove(0);
        repo.assign(&first.id, &moving).unwrap();

        assert_eq!(repo.files(&first.id).unwrap().len(), 9);
        assert_eq!(repo.files(&second.id).unwrap().len(), 7);
        // And it is in exactly one event, not two.
        let count: i64 = conn
            .query_row("SELECT count(*) FROM event_files WHERE file_id = ?1", [&moving], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn photographs_with_no_date_are_counted_not_guessed() {
        let conn = catalogue();
        shoot(&conn, "a", "2026-05-30", 12, 10);
        // A scanned print with no date at all.
        conn.execute(
            "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                source_mtime_ns, status, analysis_version, created_at, updated_at)
             VALUES ('undated','d1','rt1','x.jpg','x.jpg',1,0,'complete',1,'now','now')",
            [],
        )
        .unwrap();

        let repo = EventRepo::new(&conn);
        let report = repo.propose(DEFAULT_GAP_HOURS).unwrap();
        assert_eq!(report.photos_undated, 1);
        assert_eq!(report.photos_grouped, 10);
        // It was not silently swept into the event.
        let event = repo.list(None).unwrap().remove(0);
        assert!(!repo.files(&event.id).unwrap().contains(&"undated".to_string()));
    }

    #[test]
    fn an_event_must_be_given_a_real_name() {
        let conn = catalogue();
        shoot(&conn, "a", "2026-05-30", 12, 10);
        let repo = EventRepo::new(&conn);
        repo.propose(DEFAULT_GAP_HOURS).unwrap();
        let event = repo.list(None).unwrap().remove(0);

        assert!(repo.name_event(&event.id, "   ", None).is_err());
        assert!(repo.name_event("no-such-event", "Wedding", None).is_err());
        // A blank client is stored as absent rather than as an empty string.
        repo.name_event(&event.id, "Wedding", Some("  ")).unwrap();
        assert!(repo.list(None).unwrap()[0].client.is_none());
    }

    /// The failure this guard exists to prevent: a drive of scanned prints,
    /// each dated only to a span of years, silently forming one fabricated
    /// "event" that means nothing.
    #[test]
    fn prints_dated_only_to_a_decade_are_not_invented_into_an_event() {
        let conn = catalogue();
        // A real shoot, precisely dated.
        shoot(&conn, "wed", "2026-05-30", 12, 10);
        // Twenty scanned prints, each placed only within the late 1980s.
        for i in 0..20 {
            let id = format!("print{i:03}");
            conn.execute(
                "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                    source_mtime_ns, status, analysis_version, created_at, updated_at)
                 VALUES (?1,'d1','rt1',?1,?1,1,0,'complete',1,'now','now')",
                [&id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO date_estimates (file_id, earliest_date, latest_date, confidence,
                                             method_version, evidence_json, is_user_confirmed,
                                             created_at, updated_at)
                 VALUES (?1, '1985-01-01T00:00:00', '1989-12-31T00:00:00', 0.3, 1, '[]', 0,
                         'now', 'now')",
                [&id],
            )
            .unwrap();
        }

        let repo = EventRepo::new(&conn);
        let report = repo.propose(DEFAULT_GAP_HOURS).unwrap();

        assert_eq!(report.proposed, 1, "only the real shoot becomes an event");
        assert_eq!(report.photos_grouped, 10);
        assert_eq!(
            report.photos_imprecise, 20,
            "vaguely dated prints must be set aside and reported, not grouped"
        );

        // And none of them ended up in the wedding.
        let event = repo.list(None).unwrap().remove(0);
        let members = repo.files(&event.id).unwrap();
        assert_eq!(members.len(), 10);
        assert!(members.iter().all(|m| !m.starts_with("print")));
    }

    /// A photograph dated to a single day is still precise enough to group —
    /// the guard must not throw away ordinary EXIF-dated work.
    #[test]
    fn a_date_known_to_the_day_still_groups() {
        let conn = catalogue();
        for i in 0..8 {
            let id = format!("day{i:03}");
            conn.execute(
                "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                    source_mtime_ns, status, analysis_version, created_at, updated_at)
                 VALUES (?1,'d1','rt1',?1,?1,1,0,'complete',1,'now','now')",
                [&id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO date_estimates (file_id, earliest_date, latest_date, confidence,
                                             method_version, evidence_json, is_user_confirmed,
                                             created_at, updated_at)
                 VALUES (?1, '1999-07-04T00:00:00', '1999-07-04T23:59:59', 0.7, 1, '[]', 0,
                         'now', 'now')",
                [&id],
            )
            .unwrap();
        }
        let report = EventRepo::new(&conn).propose(DEFAULT_GAP_HOURS).unwrap();
        assert_eq!(report.proposed, 1);
        assert_eq!(report.photos_imprecise, 0);
    }

    /// Events are only useful if you can search inside them.
    #[test]
    fn an_event_and_a_client_can_be_searched_within() {
        use crate::search::{SearchFilters, SearchRepo};

        let conn = catalogue();
        shoot(&conn, "wed", "2026-05-30", 12, 10);
        shoot(&conn, "eng", "2026-02-14", 11, 10);
        // A third shoot for someone else entirely.
        shoot(&conn, "other", "2026-08-01", 10, 10);

        let repo = EventRepo::new(&conn);
        repo.propose(DEFAULT_GAP_HOURS).unwrap();
        let events = repo.list(None).unwrap();
        assert_eq!(events.len(), 3);

        // Two of the three are for one client.
        let wedding = events.iter().find(|e| e.earliest_date.as_deref().unwrap().starts_with("2026-05-30")).unwrap();
        let engagement = events.iter().find(|e| e.earliest_date.as_deref().unwrap().starts_with("2026-02-14")).unwrap();
        let unrelated = events.iter().find(|e| e.earliest_date.as_deref().unwrap().starts_with("2026-08-01")).unwrap();
        repo.name_event(&wedding.id, "Wedding", Some("Aimee & Kent")).unwrap();
        repo.name_event(&engagement.id, "Engagement", Some("Aimee & Kent")).unwrap();
        repo.name_event(&unrelated.id, "Corporate", Some("Someone Else")).unwrap();

        let search = SearchRepo::new(&conn);

        // One event.
        let in_wedding = search
            .text_search("photograph", &SearchFilters {
                event_id: Some(wedding.id.clone()),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(in_wedding.len(), 10);

        // Every shoot for one client, across events.
        let for_client = search
            .text_search("photograph", &SearchFilters {
                client: Some("Aimee & Kent".into()),
                limit: 100,
                ..Default::default()
            })
            .unwrap_or_default();
        // Filenames differ per shoot, so query on something both share.
        let both: Vec<_> = search
            .text_search("photograph", &SearchFilters {
                client: Some("aimee & kent".into()),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(both.len(), 20, "both shoots for the client, case-insensitively");
        assert!(
            both.iter().all(|r| !r.filename.starts_with("other")),
            "the other client's shoot must not leak in"
        );
        let _ = for_client;
    }

    /// A client name with an apostrophe must not break the query.
    #[test]
    fn a_name_with_an_apostrophe_is_handled() {
        use crate::search::{SearchFilters, SearchRepo};

        let conn = catalogue();
        shoot(&conn, "a", "2026-05-30", 12, 10);
        let repo = EventRepo::new(&conn);
        repo.propose(DEFAULT_GAP_HOURS).unwrap();
        let event = repo.list(None).unwrap().remove(0);
        repo.name_event(&event.id, "O'Brien wedding", Some("Sean O'Brien")).unwrap();

        let hits = SearchRepo::new(&conn)
            .text_search("photograph", &SearchFilters {
                client: Some("Sean O'Brien".into()),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(hits.len(), 10);
    }

    #[test]
    fn parses_the_date_forms_the_catalogue_actually_holds() {
        // Date only.
        assert!(parse_epoch("2026-05-30").is_some());
        // Date and time.
        let a = parse_epoch("2026-05-30T13:00:00").unwrap();
        let b = parse_epoch("2026-05-31T01:00:00").unwrap();
        assert_eq!(b - a, 12 * 3600, "must span midnight correctly");
        // Leap day.
        let feb29 = parse_epoch("2024-02-29T00:00:00").unwrap();
        let mar01 = parse_epoch("2024-03-01T00:00:00").unwrap();
        assert_eq!(mar01 - feb29, 86_400);
        // Nonsense is refused rather than guessed.
        assert!(parse_epoch("not a date").is_none());
        assert!(parse_epoch("2026-13-01").is_none());
        assert!(parse_epoch("").is_none());
    }
}
