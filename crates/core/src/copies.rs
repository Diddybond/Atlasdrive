//! Which photographs exist on only one drive.
//!
//! A shelf of drives is only as safe as its least-copied photograph. When a
//! drive fails, what is lost is exactly what was on it and nowhere else — and
//! nothing in the archive said what that was. This answers it, per drive and
//! per folder, from the catalogue alone: content hashes are compared, so every
//! drive can be unplugged, and a photograph copied into a differently named
//! folder on another drive still counts as a second copy (the same reasoning as
//! [`crate::compare`]).
//!
//! Two copies on the *same* drive are not a second copy: they fail together.
//!
//! Like `compare`, this only reports. It never suggests deleting anything.

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// A folder holding photographs that exist nowhere else.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AtRiskFolder {
    pub folder: String,
    pub photographs: i64,
}

/// One drive's share of the archive's single copies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveCopies {
    pub drive_number: i64,
    pub drive_name: Option<String>,
    /// Catalogued photographs on this drive.
    pub photographs: i64,
    /// Of those, how many exist on no other drive.
    pub only_here: i64,
    pub only_here_bytes: i64,
    /// Folders with the most single copies, largest first.
    pub folders: Vec<AtRiskFolder>,
}

impl DriveCopies {
    /// The owner's question, answered in a sentence.
    pub fn summary(&self) -> String {
        if self.only_here == 0 {
            return format!(
                "Every photograph on Drive {} also exists on another drive.",
                self.drive_number
            );
        }
        format!(
            "{} of {} photographs on Drive {} ({:.1} GB) exist on no other drive. If it failed, they would be gone.",
            self.only_here,
            self.photographs,
            self.drive_number,
            self.only_here_bytes as f64 / 1e9
        )
    }
}

/// How many folders to name per drive.
const FOLDERS_PER_DRIVE: usize = 10;

/// Single copies across the whole archive, drive by drive.
pub fn single_copies(conn: &Connection) -> Result<Vec<DriveCopies>> {
    let mut stmt = conn.prepare(
        "WITH spread AS (
             SELECT content_hash, count(DISTINCT drive_id) AS drives
               FROM files
              WHERE status = 'complete' AND content_hash IS NOT NULL
              GROUP BY content_hash
         )
         SELECT d.drive_number, d.friendly_name, f.relative_path, f.size_bytes,
                coalesce(s.drives, 1) = 1
           FROM files f
           JOIN drives d ON d.id = f.drive_id
           LEFT JOIN spread s ON s.content_hash = f.content_hash
          WHERE f.status = 'complete'
          ORDER BY d.drive_number",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, bool>(4)?,
        ))
    })?;

    let mut out: Vec<DriveCopies> = Vec::new();
    let mut folders: Vec<BTreeMap<String, i64>> = Vec::new();
    for row in rows {
        let (number, name, rel, size, only_here) = row?;
        if out.last().is_none_or(|d| d.drive_number != number) {
            out.push(DriveCopies {
                drive_number: number,
                drive_name: name,
                photographs: 0,
                only_here: 0,
                only_here_bytes: 0,
                folders: Vec::new(),
            });
            folders.push(BTreeMap::new());
        }
        let d = out.last_mut().expect("pushed above");
        d.photographs += 1;
        if only_here {
            d.only_here += 1;
            d.only_here_bytes += size;
            let folder = match rel.rfind('/') {
                Some(i) => rel[..i].to_string(),
                None => String::new(),
            };
            *folders.last_mut().expect("pushed above").entry(folder).or_default() += 1;
        }
    }
    for (d, f) in out.iter_mut().zip(folders) {
        let mut ranked: Vec<AtRiskFolder> =
            f.into_iter().map(|(folder, photographs)| AtRiskFolder { folder, photographs }).collect();
        ranked.sort_by(|a, b| b.photographs.cmp(&a.photographs).then(a.folder.cmp(&b.folder)));
        ranked.truncate(FOLDERS_PER_DRIVE);
        d.folders = ranked;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, SchemaKind};

    fn archive() -> Connection {
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute_batch(
            "INSERT INTO drives (id, drive_number, friendly_name, status, first_seen_at) VALUES
               ('d1',1,'Weddings A','offline','now'), ('d2',2,'Weddings B','offline','now');
             INSERT INTO roots (id, drive_id, relative_root, created_at) VALUES ('r1','d1','','now'), ('r2','d2','','now');",
        )
        .unwrap();
        conn
    }

    fn photo(conn: &Connection, drive: &str, rel: &str, hash: &str, status: &str) {
        let root = if drive == "d1" { "r1" } else { "r2" };
        conn.execute(
            "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                source_mtime_ns, content_hash, status, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?4,1000000,1,?5,?6,'now','now')",
            rusqlite::params![format!("{drive}/{rel}"), drive, root, rel, hash, status],
        )
        .unwrap();
    }

    #[test]
    fn a_photograph_on_two_drives_is_safe_and_one_on_a_single_drive_is_not() {
        let conn = archive();
        // Copied to the other drive under a different folder: still a copy.
        photo(&conn, "d1", "2019/Aimee and Kent/001.jpg", "h1", "complete");
        photo(&conn, "d2", "backup/aimee/001.jpg", "h1", "complete");
        // Only on drive 1 — twice, which does not make it safe.
        photo(&conn, "d1", "2019/Aimee and Kent/002.jpg", "h2", "complete");
        photo(&conn, "d1", "2019/Aimee and Kent/edits/002.jpg", "h2", "complete");
        photo(&conn, "d1", "2020/Crown/003.jpg", "h3", "complete");
        // Gone from its drive: not counted as anything.
        photo(&conn, "d2", "old/004.jpg", "h4", "missing");

        let report = single_copies(&conn).unwrap();
        assert_eq!(report.len(), 2);
        let d1 = &report[0];
        assert_eq!((d1.drive_number, d1.photographs, d1.only_here), (1, 4, 3));
        assert_eq!(d1.only_here_bytes, 3_000_000);
        assert_eq!(
            d1.folders[0],
            AtRiskFolder { folder: "2019/Aimee and Kent".into(), photographs: 1 }
        );
        assert_eq!(d1.folders.len(), 3);
        assert!(d1.summary().contains("3 of 4 photographs on Drive 1"), "{}", d1.summary());

        let d2 = &report[1];
        assert_eq!((d2.photographs, d2.only_here), (1, 0));
        assert!(d2.summary().starts_with("Every photograph on Drive 2"));
    }
}
