//! Catalogue search: text/metadata (FTS5) and local vector similarity
//! (see `docs/07_VISUAL_SEARCH_AND_TAGGING.md`).
//!
//! All search works against the local `archive.db` and cached thumbnails, so it
//! functions with source drives offline. Every result carries its drive number
//! and connection status; offline results are never silently omitted.

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

/// Read-only search over the catalogue.
pub struct SearchRepo<'a> {
    conn: &'a Connection,
}

impl<'a> SearchRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
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
            let sim = cosine_similarity(query_vector, &vector);
            scored.push((file_id, sim));
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut out = Vec::new();
        for (file_id, sim) in scored.into_iter().take(filters.limit_or(100)) {
            if let Some(mut res) = self.load_result(&file_id)? {
                if !passes_filters(&res, filters) {
                    continue;
                }
                res.matched.push("visual".into());
                res.score = sim;
                out.push(res);
            }
        }
        Ok(out)
    }

    /// Similar-image search from an already-indexed file.
    pub fn similar_to(
        &self,
        file_id: &str,
        model_id: &str,
        model_version: &str,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT vector FROM visual_embeddings WHERE file_id=?1 AND model_id=?2 AND model_version=?3",
                params![file_id, model_id, model_version],
                |r| r.get(0),
            )
            .ok();
        let Some(blob) = blob else {
            return Ok(Vec::new());
        };
        let vector = decode_vector(&blob);
        let mut results = self.vector_search(&vector, model_id, model_version, filters)?;
        results.retain(|r| r.file_id != file_id);
        Ok(results)
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
    if filters.online_only {
        sql.push_str(" AND d.status = 'online'");
    }
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
        let v = vec![1.0f32, -2.5, 0.0, 3.14];
        let blob = encode_vector(&v);
        assert_eq!(decode_vector(&blob), v);
    }

    #[test]
    fn fts_sanitize() {
        assert_eq!(sanitize_fts("bike images"), "\"bike\" OR \"images\"");
        // Injection characters are stripped; only alphanumerics survive per term.
        assert_eq!(sanitize_fts("a\" OR 1=1 --"), "\"a\" OR \"OR\" OR \"11\"");
    }
}
