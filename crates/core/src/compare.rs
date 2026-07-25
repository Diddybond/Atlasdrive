//! Comparing two drives by content.
//!
//! The question this answers is the one that comes up when twenty drives have
//! accumulated over years: *drive 6 and drive 7 look similar — are they the
//! same, and if not, what is on one and not the other?*
//!
//! This is deliberately **not** deduplication. Nothing here deletes, moves or
//! recommends removing anything. The archive is a record of what is on those
//! drives, and what is on them stays on them. The output is understanding: how
//! much two drives overlap, and the short list of files that make them differ.
//!
//! # Works with both drives unplugged
//!
//! The comparison reads content hashes out of the catalogue, so neither drive
//! needs to be connected. That matters: comparing two 4TB disks by re-reading
//! them would take hours and require both to be plugged in at once.
//!
//! # Why content hash rather than path
//!
//! Clones drift. A file copied to a differently named folder, or re-sorted into
//! a new structure, is the same photograph and should count as present on both.
//! Comparing by BLAKE3 content hash sees through renames and reorganisation,
//! which comparing paths would not.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A file present on one drive but not the other.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniqueFile {
    pub file_id: String,
    pub relative_path: String,
    pub filename: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comparison {
    pub a_number: i64,
    pub b_number: i64,
    pub a_name: Option<String>,
    pub b_name: Option<String>,
    /// Files (by distinct content) on both drives.
    pub shared: i64,
    /// Distinct contents held only by A, and only by B.
    pub only_a_count: i64,
    pub only_b_count: i64,
    pub a_total: i64,
    pub b_total: i64,
    /// Bytes held only by A / only by B.
    pub only_a_bytes: i64,
    pub only_b_bytes: i64,
    /// A capped sample, newest first, so the interface has something to show
    /// without loading tens of thousands of rows.
    pub only_a: Vec<UniqueFile>,
    pub only_b: Vec<UniqueFile>,
    pub truncated: bool,
}

impl Comparison {
    /// Overlap as a percentage of the *larger* drive.
    ///
    /// Measuring against the larger side is the conservative choice: if a small
    /// drive is entirely contained in a large one, calling that "100% identical"
    /// would be misleading, because the large drive holds a great deal the
    /// small one does not.
    pub fn overlap_percent(&self) -> f64 {
        let larger = self.a_total.max(self.b_total);
        if larger == 0 {
            return 0.0;
        }
        (self.shared as f64 / larger as f64) * 100.0
    }

    /// True when the drives are near-identical: heavy overlap, few strays.
    ///
    /// The threshold is deliberately high. Calling two drives clones when they
    /// are merely similar would invite treating one as redundant, and this
    /// module must never encourage that.
    pub fn is_near_identical(&self) -> bool {
        self.overlap_percent() >= 95.0 && self.shared > 0
    }

    /// A sentence describing the relationship, for people rather than machines.
    pub fn summary(&self) -> String {
        let a = self.a_number;
        let b = self.b_number;
        if self.shared == 0 {
            return format!("Drives {a} and {b} have nothing in common.");
        }
        if self.only_a_count == 0 && self.only_b_count == 0 {
            return format!("Drives {a} and {b} are identical — same {} files.", self.shared);
        }
        if self.only_b_count == 0 {
            return format!(
                "Drive {b} is contained in drive {a}: everything on {b} is also on {a}, \
                 and {a} holds {} more.",
                self.only_a_count
            );
        }
        if self.only_a_count == 0 {
            return format!(
                "Drive {a} is contained in drive {b}: everything on {a} is also on {b}, \
                 and {b} holds {} more.",
                self.only_b_count
            );
        }
        format!(
            "Drives {a} and {b} are {:.1}% identical: {} files in common, \
             {} only on {a}, {} only on {b}.",
            self.overlap_percent(),
            self.shared,
            self.only_a_count,
            self.only_b_count
        )
    }
}

/// How many unique files to list per side.
const SAMPLE_LIMIT: usize = 500;

/// Compare two registered drives. Neither needs to be connected.
pub fn compare_drives(conn: &Connection, a_number: i64, b_number: i64) -> Result<Comparison> {
    if a_number == b_number {
        return Err(Error::InvalidArgs(
            "cannot compare a drive with itself".into(),
        ));
    }
    let (a_id, a_name) = drive_ident(conn, a_number)?;
    let (b_id, b_name) = drive_ident(conn, b_number)?;

    // Distinct contents per side. A drive holding the same photograph twice
    // should count once, or the percentages misreport.
    let shared: i64 = conn.query_row(
        "SELECT count(*) FROM (
             SELECT content_hash FROM files
              WHERE drive_id = ?1 AND status='complete' AND content_hash IS NOT NULL
             INTERSECT
             SELECT content_hash FROM files
              WHERE drive_id = ?2 AND status='complete' AND content_hash IS NOT NULL)",
        [&a_id, &b_id],
        |r| r.get(0),
    )?;

    let a_total = distinct_contents(conn, &a_id)?;
    let b_total = distinct_contents(conn, &b_id)?;

    let (only_a_count, only_a_bytes) = unique_totals(conn, &a_id, &b_id)?;
    let (only_b_count, only_b_bytes) = unique_totals(conn, &b_id, &a_id)?;

    let only_a = unique_sample(conn, &a_id, &b_id)?;
    let only_b = unique_sample(conn, &b_id, &a_id)?;
    let truncated =
        only_a_count > only_a.len() as i64 || only_b_count > only_b.len() as i64;

    Ok(Comparison {
        a_number,
        b_number,
        a_name,
        b_name,
        shared,
        only_a_count,
        only_b_count,
        a_total,
        b_total,
        only_a_bytes,
        only_b_bytes,
        only_a,
        only_b,
        truncated,
    })
}

fn drive_ident(conn: &Connection, number: i64) -> Result<(String, Option<String>)> {
    conn.query_row(
        "SELECT id, friendly_name FROM drives WHERE drive_number = ?1",
        [number],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(|_| Error::InvalidArgs(format!("no drive numbered {number}")))
}

fn distinct_contents(conn: &Connection, drive_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(DISTINCT content_hash) FROM files
          WHERE drive_id = ?1 AND status='complete' AND content_hash IS NOT NULL",
        [drive_id],
        |r| r.get(0),
    )?)
}

/// Count and total size of contents on `drive_id` that are absent from `other`.
fn unique_totals(conn: &Connection, drive_id: &str, other: &str) -> Result<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT count(*), COALESCE(sum(size_bytes), 0) FROM (
             SELECT content_hash, max(size_bytes) AS size_bytes
               FROM files
              WHERE drive_id = ?1 AND status='complete' AND content_hash IS NOT NULL
                AND content_hash NOT IN (
                    SELECT content_hash FROM files
                     WHERE drive_id = ?2 AND status='complete' AND content_hash IS NOT NULL)
              GROUP BY content_hash)",
        [drive_id, other],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

fn unique_sample(conn: &Connection, drive_id: &str, other: &str) -> Result<Vec<UniqueFile>> {
    let mut stmt = conn.prepare(
        "SELECT id, relative_path, filename, size_bytes
           FROM files
          WHERE drive_id = ?1 AND status='complete' AND content_hash IS NOT NULL
            AND content_hash NOT IN (
                SELECT content_hash FROM files
                 WHERE drive_id = ?2 AND status='complete' AND content_hash IS NOT NULL)
          GROUP BY content_hash
          ORDER BY relative_path
          LIMIT ?3",
    )?;
    let rows = stmt.query_map(rusqlite::params![drive_id, other, SAMPLE_LIMIT], |r| {
        Ok(UniqueFile {
            file_id: r.get(0)?,
            relative_path: r.get(1)?,
            filename: r.get(2)?,
            size_bytes: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Find pairs of drives that look like clones of one another.
///
/// Compares every registered pair, which is fine at the scale this is for: a
/// twenty-drive archive is 190 pairs of index-backed counting queries.
pub fn find_near_identical(conn: &Connection) -> Result<Vec<Comparison>> {
    let numbers: Vec<i64> = {
        let mut stmt =
            conn.prepare("SELECT drive_number FROM drives ORDER BY drive_number")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };

    let mut found = Vec::new();
    for (i, &a) in numbers.iter().enumerate() {
        for &b in &numbers[i + 1..] {
            let c = compare_drives(conn, a, b)?;
            if c.is_near_identical() {
                found.push(c);
            }
        }
    }
    // Most alike first.
    found.sort_by(|x, y| {
        y.overlap_percent()
            .partial_cmp(&x.overlap_percent())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    fn setup() -> Connection {
        db::open_in_memory(SchemaKind::Archive).unwrap()
    }

    fn add_drive(conn: &Connection, id: &str, number: i64, name: &str) {
        conn.execute(
            "INSERT INTO drives (id, drive_number, friendly_name, status, first_seen_at)
             VALUES (?1, ?2, ?3, 'offline', 'now')",
            rusqlite::params![id, number, name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO roots (id, drive_id, relative_root, created_at)
             VALUES (?1, ?2, '', 'now')",
            rusqlite::params![format!("rt-{id}"), id],
        )
        .unwrap();
    }

    /// `hash` is what decides identity; `path` deliberately varies so the tests
    /// prove the comparison sees through reorganisation.
    fn add_file(conn: &Connection, drive_id: &str, path: &str, hash: &str, size: i64) {
        conn.execute(
            "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                source_mtime_ns, content_hash, status, analysis_version,
                                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, 0, ?6, 'complete', 1, 'now', 'now')",
            rusqlite::params![
                format!("{drive_id}-{path}"),
                drive_id,
                format!("rt-{drive_id}"),
                path,
                size,
                hash
            ],
        )
        .unwrap();
    }

    #[test]
    fn identical_drives_are_reported_as_identical() {
        let conn = setup();
        add_drive(&conn, "d6", 6, "Weddings A");
        add_drive(&conn, "d7", 7, "Weddings A copy");
        for h in ["h1", "h2", "h3"] {
            add_file(&conn, "d6", &format!("{h}.jpg"), h, 1000);
            add_file(&conn, "d7", &format!("{h}.jpg"), h, 1000);
        }

        let c = compare_drives(&conn, 6, 7).unwrap();
        assert_eq!(c.shared, 3);
        assert_eq!(c.only_a_count, 0);
        assert_eq!(c.only_b_count, 0);
        assert!((c.overlap_percent() - 100.0).abs() < 0.01);
        assert!(c.is_near_identical());
        assert!(c.summary().contains("identical"), "{}", c.summary());
    }

    /// The case that prompted this: two drives alike apart from a handful.
    #[test]
    fn near_identical_drives_list_what_differs() {
        let conn = setup();
        add_drive(&conn, "d6", 6, "Archive 6");
        add_drive(&conn, "d7", 7, "Archive 7");
        for i in 0..100 {
            let h = format!("shared{i}");
            add_file(&conn, "d6", &format!("a/{h}.jpg"), &h, 2000);
            add_file(&conn, "d7", &format!("b/{h}.jpg"), &h, 2000);
        }
        add_file(&conn, "d6", "extra1.jpg", "only6a", 5000);
        add_file(&conn, "d6", "extra2.jpg", "only6b", 5000);
        add_file(&conn, "d7", "extra3.jpg", "only7a", 7000);

        let c = compare_drives(&conn, 6, 7).unwrap();
        assert_eq!(c.shared, 100);
        assert_eq!(c.only_a_count, 2);
        assert_eq!(c.only_b_count, 1);
        assert_eq!(c.only_a_bytes, 10_000);
        assert_eq!(c.only_b_bytes, 7_000);
        assert!(c.is_near_identical(), "{:.1}%", c.overlap_percent());

        // The files that differ are named, because that is the actionable part.
        let names: Vec<_> = c.only_a.iter().map(|f| f.filename.as_str()).collect();
        assert!(names.contains(&"extra1.jpg") && names.contains(&"extra2.jpg"));
        assert_eq!(c.only_b.len(), 1);
        assert_eq!(c.only_b[0].filename, "extra3.jpg");
    }

    /// Clones drift: the same photograph filed under a different folder is
    /// still the same photograph.
    #[test]
    fn reorganised_copies_still_count_as_shared() {
        let conn = setup();
        add_drive(&conn, "d1", 1, "Original");
        add_drive(&conn, "d2", 2, "Reorganised");
        add_file(&conn, "d1", "2019/wedding/img_001.jpg", "same", 100);
        add_file(&conn, "d2", "sorted/by-client/smith/001.jpg", "same", 100);

        let c = compare_drives(&conn, 1, 2).unwrap();
        assert_eq!(c.shared, 1, "path differs but content does not");
        assert_eq!(c.only_a_count, 0);
        assert_eq!(c.only_b_count, 0);
    }

    /// A small drive wholly inside a large one is not "100% identical" — the
    /// large drive holds much the small one does not.
    #[test]
    fn a_contained_drive_is_described_as_contained_not_identical() {
        let conn = setup();
        add_drive(&conn, "big", 1, "Everything");
        add_drive(&conn, "small", 2, "Subset");
        for i in 0..100 {
            add_file(&conn, "big", &format!("f{i}.jpg"), &format!("h{i}"), 1000);
        }
        for i in 0..10 {
            add_file(&conn, "small", &format!("f{i}.jpg"), &format!("h{i}"), 1000);
        }

        let c = compare_drives(&conn, 1, 2).unwrap();
        assert_eq!(c.shared, 10);
        assert_eq!(c.only_b_count, 0);
        assert_eq!(c.only_a_count, 90);
        // 10 of 100 on the larger side.
        assert!((c.overlap_percent() - 10.0).abs() < 0.01);
        assert!(!c.is_near_identical(), "must not call this a clone");
        assert!(c.summary().contains("contained"), "{}", c.summary());
    }

    #[test]
    fn unrelated_drives_share_nothing() {
        let conn = setup();
        add_drive(&conn, "d1", 1, "Weddings");
        add_drive(&conn, "d2", 2, "Landscapes");
        add_file(&conn, "d1", "a.jpg", "h1", 10);
        add_file(&conn, "d2", "b.jpg", "h2", 10);

        let c = compare_drives(&conn, 1, 2).unwrap();
        assert_eq!(c.shared, 0);
        assert!(!c.is_near_identical());
        assert!(c.summary().contains("nothing in common"), "{}", c.summary());
    }

    /// A drive holding the same photograph twice must not inflate its own count
    /// and skew the percentages.
    #[test]
    fn duplicates_within_one_drive_are_counted_once() {
        let conn = setup();
        add_drive(&conn, "d1", 1, "A");
        add_drive(&conn, "d2", 2, "B");
        add_file(&conn, "d1", "one/img.jpg", "same", 100);
        add_file(&conn, "d1", "two/img-copy.jpg", "same", 100);
        add_file(&conn, "d2", "img.jpg", "same", 100);

        let c = compare_drives(&conn, 1, 2).unwrap();
        assert_eq!(c.a_total, 1, "two copies of one photograph is one content");
        assert_eq!(c.shared, 1);
        assert!((c.overlap_percent() - 100.0).abs() < 0.01);
    }

    #[test]
    fn finds_clone_pairs_across_the_whole_archive() {
        let conn = setup();
        add_drive(&conn, "d1", 1, "A");
        add_drive(&conn, "d2", 2, "A clone");
        add_drive(&conn, "d3", 3, "Unrelated");
        for i in 0..50 {
            let h = format!("h{i}");
            add_file(&conn, "d1", &format!("{h}.jpg"), &h, 100);
            add_file(&conn, "d2", &format!("{h}.jpg"), &h, 100);
        }
        add_file(&conn, "d3", "x.jpg", "other", 100);

        let pairs = find_near_identical(&conn).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!((pairs[0].a_number, pairs[0].b_number), (1, 2));
    }

    #[test]
    fn comparing_a_drive_with_itself_is_refused() {
        let conn = setup();
        add_drive(&conn, "d1", 1, "A");
        assert!(compare_drives(&conn, 1, 1).is_err());
        assert!(compare_drives(&conn, 1, 99).is_err());
    }
}
