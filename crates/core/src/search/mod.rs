//! Catalogue search: text/metadata (FTS5) and local vector similarity
//! (see `docs/07_VISUAL_SEARCH_AND_TAGGING.md`).
//!
//! All search works against the local `archive.db` and cached thumbnails, so it
//! functions with source drives offline. Every result carries its drive number
//! and connection status; offline results are never silently omitted.

pub mod vecindex;

/// How many extra candidates to rank before applying filters.
///
/// A drive-filtered search over a twenty-drive archive keeps roughly one
/// twentieth of what it ranks, so ranking only the caller's page size would
/// return almost nothing. Twenty is the smallest factor that covers that case
/// without ranking the whole catalogue.
const OVERFETCH: usize = 20;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::util::cosine_similarity;

/// A single search result card (mirrors `docs/07` result-card requirements).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub file_id: String,
    pub filename: String,
    pub relative_path: String,
    pub drive_number: i64,
    pub drive_name: Option<String>,
    pub drive_status: String,
    pub online: bool,
    pub thumbnail_rel_path: Option<String>,
    pub date_range: Option<(String, String)>,
    pub date_label: Option<String>,
    pub matched: Vec<String>,
    /// Match strength in [0,1]; probabilistic, never presented as certainty.
    pub score: f32,
}

/// Filters applicable to any search.
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    pub drive_number: Option<i64>,
    pub online_only: bool,
    pub include_offline: bool,
    pub person_id: Option<String>,
    /// Restrict to one event, or to every event shot for one client.
    pub event_id: Option<String>,
    pub client: Option<String>,
    /// Subjects every result must carry, not merely one of.
    ///
    /// Picking "wedding" and "child" should mean children at weddings, not
    /// every wedding plus every child. Each tag narrows; typing the words into
    /// the query box would have widened.
    pub tags: Vec<String>,
    pub scanned_only: bool,
    pub limit: usize,
}

impl SearchFilters {
    pub fn limit_or(&self, default: usize) -> usize {
        if self.limit == 0 {
            default
        } else {
            self.limit
        }
    }
}

/// Weight a confirmed text/metadata hit carries in a fused ranking. Text
/// matches are evidence about the file; visual similarity is a guess about its
/// content, so text leads.
const TEXT_WEIGHT: f32 = 0.6;
/// Maximum weight visual similarity can add, before scaling by how much of the
/// query the text encoder actually understood.
const VISUAL_WEIGHT: f32 = 0.4;

/// A natural-language query already embedded into the visual space.
#[derive(Debug, Clone, Copy)]
pub struct VisualQuery<'v> {
    pub vector: &'v [f32],
    /// Model partition the vector belongs to; must match the indexed images.
    pub model_id: &'v str,
    pub model_version: &'v str,
    /// Encoder confidence that it understood the query, in [0,1]. Zero disables
    /// the visual leg entirely.
    pub coverage: f32,
}

/// Read-only search over the catalogue.
pub struct SearchRepo<'a> {
    conn: &'a Connection,
    /// Where the persisted vector index lives, when one is available.
    ///
    /// Optional so that tests and the verifier can construct a repo from a
    /// bare connection. Without it, searching still works — it just reads
    /// every embedding out of SQLite, which is fine for a few thousand files
    /// and is not fine for two hundred thousand.
    index_dir: Option<std::path::PathBuf>,
}

impl<'a> SearchRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn, index_dir: None }
    }

    /// A repo that will use (and maintain) a persisted vector index.
    pub fn with_index_dir(conn: &'a Connection, dir: impl Into<std::path::PathBuf>) -> Self {
        Self { conn, index_dir: Some(dir.into()) }
    }

    fn index_path(&self, model_id: &str, model_version: &str) -> Option<std::path::PathBuf> {
        // One file per partition: a 768-dimension Vision index and a 65-
        // dimension heuristic one are different spaces and must not share.
        let safe = |s: &str| s.replace(['/', '\\', '.'], "-");
        self.index_dir.as_ref().map(|d| {
            d.join(format!("vec-{}-{}.idx", safe(model_id), safe(model_version)))
        })
    }

    /// Full-text search over filename, path, tags, OCR text and scene text.
    pub fn text_search(&self, query: &str, filters: &SearchFilters) -> Result<Vec<SearchResult>> {
        let match_query = sanitize_fts(query);
        if match_query.is_empty() {
            return Ok(Vec::new());
        }
        let mut sql = String::from(
            "SELECT f.id
             FROM files_fts fts
             JOIN files f ON f.id = fts.file_id
             JOIN drives d ON d.id = f.drive_id
             WHERE files_fts MATCH ?1 AND f.status = 'complete'",
        );
        push_filter_sql(&mut sql, filters);
        sql.push_str(" ORDER BY bm25(files_fts) LIMIT ?2");

        let mut stmt = self.conn.prepare(&sql)?;
        let ids: Vec<String> = stmt
            .query_map(params![match_query, filters.limit_or(100) as i64], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut out = Vec::new();
        for id in ids {
            if let Some(mut res) = self.load_result(&id)? {
                res.matched.push("text".into());
                res.score = 1.0;
                out.push(res);
            }
        }
        Ok(out)
    }

    /// Vector similarity search against stored visual embeddings for a given
    /// model partition. Brute-force cosine is appropriate for a local,
    /// single-user catalogue and keeps model-version partitions clean.
    pub fn vector_search(
        &self,
        query_vector: &[f32],
        model_id: &str,
        model_version: &str,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>> {
        let limit = filters.limit_or(100);

        // Rank far more candidates than the caller will show. Filters are
        // applied *after* ranking — a drive or date filter that removed most of
        // a top-100 would otherwise return a nearly empty page while thousands
        // of matching photographs sat just below the cut.
        let candidates = (limit * OVERFETCH).max(limit);

        let scored: Vec<(String, f32)> = match self.index_path(model_id, model_version) {
            Some(path) => {
                match vecindex::VectorIndex::load_or_build(
                    self.conn, &path, model_id, model_version,
                ) {
                    Ok(index) => index.search(query_vector, candidates),
                    // An index that cannot be built must not take search down
                    // with it; the exhaustive path still answers correctly.
                    Err(_) => self.scan_all_embeddings(query_vector, model_id, model_version)?,
                }
            }
            None => self.scan_all_embeddings(query_vector, model_id, model_version)?,
        };

        let mut out = Vec::new();
        for (file_id, sim) in scored {
            if out.len() >= limit {
                break;
            }
            if let Some(mut res) = self.load_result(&file_id)? {
                if !passes_filters(&res, filters) {
                    continue;
                }
                // The visual path comes from the vector index, not from SQL, so
                // the tag conditions have to be applied to each candidate here
                // too — otherwise selecting a subject would silently stop
                // narrowing as soon as a query had visual meaning.
                if !filters.tags.is_empty()
                    && !file_has_all_tags(self.conn, &file_id, &filters.tags)?
                {
                    continue;
                }
                res.matched.push("visual".into());
                res.score = sim;
                out.push(res);
            }
        }
        Ok(out)
    }

    /// Photographs that look like this one.
    ///
    /// # Why this exists, and why text search does not use the vector index
    ///
    /// Apple's Vision framework produces image feature prints and has no text
    /// encoder, so there is no way to place a typed query in the same space as
    /// the photographs. The only local text encoder available renders a query
    /// into a small grid of colour and brightness features — matching "wedding
    /// dress" against that returns photographs containing white regions, which
    /// is colour matching wearing the costume of semantics. Wiring it in would
    /// have injected noise into rankings that Vision's classification labels
    /// already answer well, so text search deliberately goes through those
    /// labels and OCR instead (see D-040).
    ///
    /// Image-to-image similarity is the thing feature prints *are* for, and it
    /// is what this index earns its keep on: given one photograph, find the
    /// others from the same set-up, the same pose, the same light.
    pub fn similar_to(
        &self,
        file_id: &str,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>> {
        // Whichever model actually embedded this photograph; no assumption that
        // it is Vision, so this keeps working if the engine changes.
        let row: Option<(String, String, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT model_id, model_version, vector
                   FROM visual_embeddings WHERE file_id = ?1
                  ORDER BY dim DESC LIMIT 1",
                [file_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        let Some((model_id, model_version, blob)) = row else {
            return Ok(Vec::new());
        };
        let vector = decode_vector(&blob);
        if vector.is_empty() {
            return Ok(Vec::new());
        }

        // A photograph is always its own nearest neighbour, and that is not a
        // result anybody asked for. Ask for one extra and drop it, or a request
        // for five similar photographs quietly returns four.
        let limit = filters.limit_or(100);
        let widened = SearchFilters { limit: limit + 1, ..filters.clone() };
        let mut hits = self.vector_search(&vector, &model_id, &model_version, &widened)?;
        hits.retain(|r| r.file_id != file_id);
        hits.truncate(limit);
        Ok(hits)
    }

    /// The exhaustive path: score every embedding straight from SQLite.
    ///
    /// Kept as the fallback rather than deleted, because it is the definition
    /// the index is a cache of, and because a repo built without an index
    /// directory still has to work.
    fn scan_all_embeddings(
        &self,
        query_vector: &[f32],
        model_id: &str,
        model_version: &str,
    ) -> Result<Vec<(String, f32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT ve.file_id, ve.vector
             FROM visual_embeddings ve
             JOIN files f ON f.id = ve.file_id
             WHERE ve.model_id = ?1 AND ve.model_version = ?2 AND f.status='complete'",
        )?;
        let rows = stmt.query_map(params![model_id, model_version], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;

        let mut scored: Vec<(String, f32)> = Vec::new();
        for row in rows {
            let (file_id, blob) = row?;
            let vector = decode_vector(&blob);
            scored.push((file_id, cosine_similarity(query_vector, &vector)));
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored)
    }

    /// Natural-language search: the text/metadata index and the visual
    /// embedding space, fused into one ranking.
    ///
    /// `visual` is the query embedded by an engine advertising
    /// [`crate::ai::Capability::TextEmbedding`]. It is optional, and is ignored
    /// when its `coverage` is zero — a query the encoder did not understand must
    /// not be allowed to reorder results by what would be noise. In that case
    /// this degrades to exactly [`SearchRepo::text_search`].
    pub fn natural_language_search(
        &self,
        query: &str,
        visual: Option<VisualQuery<'_>>,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>> {
        let mut merged: Vec<SearchResult> = self.text_search(query, filters)?;
        for r in &mut merged {
            r.score = TEXT_WEIGHT;
        }

        let Some(visual) = visual.filter(|v| v.coverage > 0.0) else {
            return Ok(merged);
        };

        let visual_hits =
            self.vector_search(visual.vector, visual.model_id, visual.model_version, filters)?;
        let n = visual_hits.len() as f32;
        for (i, hit) in visual_hits.into_iter().enumerate() {
            // Rank position, not raw cosine: these vectors are non-negative, so
            // cosine values sit in a narrow high band and read as misleadingly
            // confident. Position within the candidate set is the honest signal.
            let rank_score = if n <= 1.0 { 1.0 } else { 1.0 - (i as f32) / n };
            let contribution = VISUAL_WEIGHT * visual.coverage * rank_score;
            match merged.iter_mut().find(|r| r.file_id == hit.file_id) {
                Some(existing) => {
                    existing.matched.push("visual".into());
                    existing.score += contribution;
                }
                None => {
                    let mut hit = hit;
                    hit.score = contribution;
                    merged.push(hit);
                }
            }
        }

        for r in &mut merged {
            r.score = r.score.clamp(0.0, 1.0);
        }
        merged.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        merged.truncate(filters.limit_or(100));
        Ok(merged)
    }

    fn load_result(&self, file_id: &str) -> Result<Option<SearchResult>> {
        let row = self.conn.query_row(
            "SELECT f.id, f.filename, f.relative_path, d.drive_number, d.friendly_name, d.status,
                    t.rel_path, de.earliest_date, de.latest_date
             FROM files f
             JOIN drives d ON d.id = f.drive_id
             LEFT JOIN thumbnails t ON t.file_id = f.id
             LEFT JOIN date_estimates de ON de.file_id = f.id
             WHERE f.id = ?1",
            [file_id],
            |r| {
                let status: String = r.get(5)?;
                let earliest: Option<String> = r.get(7)?;
                let latest: Option<String> = r.get(8)?;
                Ok(SearchResult {
                    file_id: r.get(0)?,
                    filename: r.get(1)?,
                    relative_path: r.get(2)?,
                    drive_number: r.get(3)?,
                    drive_name: r.get(4)?,
                    online: status == "online",
                    drive_status: status,
                    thumbnail_rel_path: r.get(6)?,
                    date_range: match (earliest, latest) {
                        (Some(e), Some(l)) => Some((e, l)),
                        _ => None,
                    },
                    date_label: None,
                    matched: Vec::new(),
                    score: 0.0,
                })
            },
        );
        match row {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Resolve a catalogued file back to its original on disk, if the drive is
/// currently connected.
///
/// Returns `Ok(None)` when the drive is offline or the original is simply not
/// there — that is an ordinary state for this product, not an error. Uses the
/// same resolution order as the integrity verifier (recorded scan root first,
/// then a conventional `/Volumes/<name>` mount), so "the verifier can see it"
/// and "Reveal in Finder works" never disagree.
pub fn resolve_original(conn: &Connection, file_id: &str) -> Result<Option<std::path::PathBuf>> {
    let row = conn.query_row(
        "SELECT f.relative_path, d.volume_name,
                (SELECT sr.scan_root FROM scan_runs sr
                  WHERE sr.drive_id = f.drive_id AND sr.mode <> 'dry-run'
                  ORDER BY sr.started_at DESC LIMIT 1)
           FROM files f JOIN drives d ON d.id = f.drive_id
          WHERE f.id = ?1",
        [file_id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        },
    );
    let (rel, volume_name, scan_root) = match row {
        Ok(v) => v,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let candidates = [
        scan_root.map(|root| std::path::Path::new(&root).join(&rel)),
        volume_name.map(|vol| std::path::Path::new("/Volumes").join(&vol).join(&rel)),
    ];
    Ok(candidates.into_iter().flatten().find(|p| p.exists()))
}

/// True when the file carries every one of `tags`.
fn file_has_all_tags(conn: &Connection, file_id: &str, tags: &[String]) -> Result<bool> {
    let held: i64 = conn.query_row(
        "SELECT count(DISTINCT t.name) FROM file_tags ft
           JOIN tags t ON t.id = ft.tag_id
          WHERE ft.file_id = ?1 AND t.name IN (SELECT value FROM json_each(?2))",
        rusqlite::params![file_id, serde_json::to_string(tags).unwrap_or_default()],
        |r| r.get(0),
    )?;
    Ok(held as usize == tags.len())
}

fn passes_filters(res: &SearchResult, filters: &SearchFilters) -> bool {
    if let Some(dn) = filters.drive_number {
        if res.drive_number != dn {
            return false;
        }
    }
    if filters.online_only && !res.online {
        return false;
    }
    true
}

fn push_filter_sql(sql: &mut String, filters: &SearchFilters) {
    if let Some(dn) = filters.drive_number {
        sql.push_str(&format!(" AND d.drive_number = {dn}"));
    }
    // One EXISTS per tag, so the conditions intersect rather than union.
    for tag in &filters.tags {
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
                           WHERE ft.file_id = f.id AND t.name = '{}')",
            escape_sql(tag)
        ));
    }
    if filters.online_only {
        sql.push_str(" AND d.status = 'online'");
    }
    // Event and client identifiers come from the catalogue rather than from
    // typed input, but they still reach here as strings, so they are escaped
    // rather than interpolated raw.
    if let Some(ev) = &filters.event_id {
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM event_files ef WHERE ef.file_id = f.id
                           AND ef.event_id = '{}')",
            escape_sql(ev)
        ));
    }
    if let Some(client) = &filters.client {
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM event_files ef
                            JOIN events e ON e.id = ef.event_id
                           WHERE ef.file_id = f.id
                             AND e.client = '{}' COLLATE NOCASE)",
            escape_sql(client)
        ));
    }
}

/// Single-quote escaping for values interpolated into the filter clauses.
fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}

/// Escape an FTS query into a safe phrase match to avoid syntax injection.
fn sanitize_fts(query: &str) -> String {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| t.chars().any(|c| c.is_alphanumeric()))
        .map(|t| {
            let cleaned: String = t.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
            format!("\"{cleaned}\"")
        })
        .collect();
    terms.join(" OR ")
}

/// Decode a little-endian f32 blob back into a vector.
pub fn decode_vector(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Encode an f32 vector to a little-endian blob for storage.
pub fn encode_vector(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vector.len() * 4);
    for v in vector {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_roundtrip() {
        let v = vec![1.0f32, -2.5, 0.0, 3.25];
        let blob = encode_vector(&v);
        assert_eq!(decode_vector(&blob), v);
    }

    /// Reveal-in-Finder must resolve a real original when the drive is
    /// connected, and must resolve to nothing once it is not.
    #[test]
    fn resolve_original_follows_the_drive_online_and_offline() {
        use crate::db::{self, SchemaKind};

        let dir = tempfile::tempdir().unwrap();
        let volume = dir.path().join("Volumes/TestVol");
        std::fs::create_dir_all(volume.join("holiday")).unwrap();
        let original = volume.join("holiday/beach.png");
        std::fs::write(&original, b"not really a png").unwrap();

        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute(
            "INSERT INTO drives(id, drive_number, volume_name, status, first_seen_at)
             VALUES ('d1', 14, 'TestVol', 'online', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO roots(id, drive_id, relative_root, created_at)
             VALUES ('r1','d1','','2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files(id, drive_id, root_id, relative_path, filename, size_bytes,
                               source_mtime_ns, status, created_at, updated_at)
             VALUES ('f1','d1','r1','holiday/beach.png','beach.png',1,1,'complete',
                     '2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scan_runs(id, drive_id, drive_number, scan_root, mode, started_at)
             VALUES ('run1','d1',14,?1,'initial','2026-01-01T00:00:00Z')",
            [volume.to_string_lossy()],
        )
        .unwrap();

        let found = resolve_original(&conn, "f1").unwrap();
        assert_eq!(found.as_deref(), Some(original.as_path()));

        // Drive disconnected: the volume is gone, so there is nothing to reveal.
        std::fs::remove_dir_all(dir.path().join("Volumes")).unwrap();
        assert!(resolve_original(&conn, "f1").unwrap().is_none());

        // An unknown file is not an error either.
        assert!(resolve_original(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn fts_sanitize() {
        assert_eq!(sanitize_fts("bike images"), "\"bike\" OR \"images\"");
        // Injection characters are stripped; only alphanumerics survive per term.
        assert_eq!(sanitize_fts("a\" OR 1=1 --"), "\"a\" OR \"OR\" OR \"11\"");
    }
}

#[cfg(test)]
mod tag_filter_tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    /// Two drives, four photographs, overlapping subjects.
    fn catalogue() -> rusqlite::Connection {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        for (id, n, vol) in [("d1", 1i64, "Late25A"), ("d2", 2, "NewVolume")] {
            conn.execute(
                "INSERT INTO drives(id, drive_number, volume_name, status, first_seen_at)
                 VALUES (?1,?2,?3,'online','2026-01-01T00:00:00Z')",
                rusqlite::params![id, n, vol],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO roots(id, drive_id, relative_root, created_at)
                 VALUES (?1,?2,'','2026-01-01T00:00:00Z')",
                rusqlite::params![format!("r{id}"), id],
            )
            .unwrap();
        }
        for (fid, drive) in
            [("f1", "d1"), ("f2", "d1"), ("f3", "d2"), ("f4", "d2")]
        {
            conn.execute(
                "INSERT INTO files(id, drive_id, root_id, relative_path, filename, size_bytes,
                                   source_mtime_ns, status, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?4,1,1,'complete','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                rusqlite::params![fid, drive, format!("r{drive}"), format!("{fid}.jpg")],
            )
            .unwrap();
        }
        for (tid, name) in [("t1", "wedding"), ("t2", "child"), ("t3", "landscape")] {
            conn.execute(
                "INSERT INTO tags(id, name, tag_type, created_at)
                 VALUES (?1,?2,'automatic','2026-01-01T00:00:00Z')",
                rusqlite::params![tid, name],
            )
            .unwrap();
        }
        // f1: wedding + child   f2: wedding   f3: wedding + child   f4: landscape
        for (f, t) in [("f1", "t1"), ("f1", "t2"), ("f2", "t1"), ("f3", "t1"), ("f3", "t2"), ("f4", "t3")] {
            conn.execute(
                "INSERT INTO file_tags(file_id, tag_id, confidence, source, created_at)
                 VALUES (?1,?2,0.9,'vision','2026-01-01T00:00:00Z')",
                rusqlite::params![f, t],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn selecting_two_subjects_narrows_rather_than_widens() {
        let conn = catalogue();
        assert!(file_has_all_tags(&conn, "f1", &["wedding".into(), "child".into()]).unwrap());
        // f2 is a wedding but has no child in it: picking both must exclude it.
        assert!(!file_has_all_tags(&conn, "f2", &["wedding".into(), "child".into()]).unwrap());
        assert!(!file_has_all_tags(&conn, "f4", &["wedding".into()]).unwrap());
    }

    #[test]
    fn an_empty_selection_excludes_nothing() {
        let conn = catalogue();
        for f in ["f1", "f2", "f3", "f4"] {
            assert!(file_has_all_tags(&conn, f, &[]).unwrap(), "{f} should pass");
        }
    }

    /// A subject that does not exist must match nothing, not everything —
    /// the failure mode where a filter quietly stops filtering.
    #[test]
    fn an_unknown_subject_matches_nothing() {
        let conn = catalogue();
        for f in ["f1", "f2", "f3", "f4"] {
            assert!(!file_has_all_tags(&conn, f, &["unicorn".into()]).unwrap());
        }
    }

    /// Tags are scoped to the drive being browsed, so every chip on screen
    /// leads to photographs on the disk that is actually connected.
    /// The list is chosen by how many photographs each subject has, but shown
    /// in alphabetical order — a subject you are looking for has to be findable
    /// without knowing its rank.
    #[test]
    fn subjects_are_listed_alphabetically() {
        let conn = catalogue();
        let tags = crate::inventory::tags_on_drive(&conn, 60, None).unwrap();
        let names: Vec<_> = tags.iter().map(|t| t.tag.clone()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "subjects must read alphabetically");
        // Counts are still real, so a chip still says whether it is worth a click.
        assert_eq!(tags.iter().find(|t| t.tag == "wedding").unwrap().count, 3);
    }

    /// The limit must still take the *biggest* subjects, not the first ones in
    /// the alphabet — otherwise a busy catalogue would only ever offer A to D.
    #[test]
    fn the_limit_keeps_the_largest_subjects_not_the_earliest_letters() {
        let conn = catalogue();
        let two = crate::inventory::tags_on_drive(&conn, 2, None).unwrap();
        let names: Vec<_> = two.iter().map(|t| t.tag.as_str()).collect();
        // wedding (3) and child (2) are the two largest; landscape (1) is not,
        // despite sorting before "wedding".
        assert!(names.contains(&"wedding"), "got {names:?}");
        assert!(names.contains(&"child"), "got {names:?}");
        assert!(!names.contains(&"landscape"), "got {names:?}");
        // And what survives is still shown alphabetically.
        assert_eq!(names, vec!["child", "wedding"]);
    }

    #[test]
    fn the_subject_list_follows_the_selected_drive() {
        let conn = catalogue();
        let everywhere = crate::inventory::tags_on_drive(&conn, 60, None).unwrap();
        let names: Vec<_> = everywhere.iter().map(|t| t.tag.as_str()).collect();
        assert!(names.contains(&"wedding") && names.contains(&"landscape"));

        let drive1 = crate::inventory::tags_on_drive(&conn, 60, Some(1)).unwrap();
        let d1: Vec<_> = drive1.iter().map(|t| t.tag.as_str()).collect();
        assert!(d1.contains(&"wedding"));
        assert!(!d1.contains(&"landscape"), "landscape is only on Drive 2");

        // And the counts are the drive's, not the catalogue's.
        let wedding = drive1.iter().find(|t| t.tag == "wedding").unwrap();
        assert_eq!(wedding.count, 2, "f1 and f2, not f3 on the other drive");
    }

    /// A quoted name must not be able to change the meaning of the query.
    #[test]
    fn a_subject_containing_a_quote_is_escaped() {
        let mut sql = String::new();
        let filters = SearchFilters {
            tags: vec!["o'brien' OR 1=1 --".into()],
            ..Default::default()
        };
        push_filter_sql(&mut sql, &filters);
        assert!(sql.contains("o''brien"), "quote must be doubled: {sql}");

        // And it is still valid SQL that matches nothing.
        let conn = catalogue();
        let q = format!(
            "SELECT count(*) FROM files f JOIN drives d ON d.id=f.drive_id WHERE 1=1{sql}"
        );
        let n: i64 = conn.query_row(&q, [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }
}
