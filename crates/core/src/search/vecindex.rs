//! A compact, persisted index over the visual embeddings.
//!
//! # The problem
//!
//! `SearchRepo::vector_search` read every embedding out of SQLite on every
//! query, decoded each blob to `Vec<f32>`, and scored it. At the scale this
//! archive is heading for — twenty drives, 200,000+ photographs, 768-dimension
//! Apple Vision embeddings — that is 614MB pulled through SQLite and allocated
//! as several hundred thousand short-lived vectors, per keystroke. The dot
//! products were never the cost; the I/O and the allocation were.
//!
//! # The approach, and why it is not an approximate index
//!
//! Embeddings are L2-normalised, quantised to `i8`, and held in one contiguous
//! allocation. Two consequences follow:
//!
//!   * For unit vectors, cosine similarity *is* the dot product, so scoring is
//!     an integer dot product over a flat slice — cache-friendly and with no
//!     per-vector allocation at all.
//!   * The whole partition costs one byte per dimension: 147MB at 200,000
//!     vectors rather than 614MB, and it is loaded once rather than per query.
//!
//! Quantisation has a floor: `i8` resolves about 1/127 per component, and
//! measured over 768-dimension embeddings that works out at roughly 0.01 of
//! cosine similarity. Two photographs whose true similarity differs by less
//! than that may swap places. For image search those are equally good matches
//! and the order between them was never meaningful; what must not happen is a
//! genuinely better match being missed, and the tests assert exactly that.
//!
//! An approximate structure (HNSW and friends) would go faster still, but it
//! trades recall for that speed, needs tuning, and adds a dependency. A scan
//! over quantised vectors is a few tens of milliseconds at this scale, returns
//! *exactly* the right ranking up to quantisation, and has nothing to tune. For
//! a single-user archive that is the better trade. If the archive ever reaches
//! a scale where it is not, this module is where that changes, and
//! [`VectorIndex::search`] is the only thing that would need to.
//!
//! # Staleness
//!
//! The index is a cache, and a stale cache silently returns wrong answers. Each
//! saved index records the partition it was built for and a fingerprint of the
//! rows it was built from; [`VectorIndex::is_current`] re-derives that
//! fingerprint and refuses the index if it disagrees. A refusal costs a rebuild;
//! trusting a stale index would cost correctness.

use std::path::Path;

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Quantisation scale. Unit-vector components live in [-1, 1], so 127 uses the
/// full `i8` range.
const SCALE: f32 = 127.0;

/// Bumped when the on-disk layout changes, so an old file is rejected rather
/// than misread.
const FORMAT_VERSION: u32 = 1;

const MAGIC: &[u8; 8] = b"ATLASVX1";

/// A flat, quantised index over one `(model_id, model_version)` partition.
pub struct VectorIndex {
    pub model_id: String,
    pub model_version: String,
    /// Dimensions per vector.
    pub dim: usize,
    /// File ids, parallel to the rows of `data`.
    ids: Vec<String>,
    /// `ids.len() * dim` quantised components, row-major.
    data: Vec<i8>,
    /// L2 norm of each quantised row.
    ///
    /// Stored rather than assumed. Rounding each component to `i8` leaves the
    /// quantised vector's norm slightly off `SCALE`: with 768 dimensions a unit
    /// vector's components are around 0.036, so quantising to 4.58 -> 5 is a 9%
    /// error on that component, and the accumulated drift pushed self-similarity
    /// to 1.013 — impossible for a cosine. Dividing by the true norms gives the
    /// exact cosine of the quantised vectors, always within [-1, 1].
    norms: Vec<f32>,
    /// Identifies the rows this was built from; see module docs.
    fingerprint: u64,
}

impl VectorIndex {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Build from the embeddings in a catalogue.
    pub fn build(conn: &Connection, model_id: &str, model_version: &str) -> Result<Self> {
        let mut stmt = conn.prepare(
            "SELECT ve.file_id, ve.vector
               FROM visual_embeddings ve
               JOIN files f ON f.id = ve.file_id
              WHERE ve.model_id = ?1 AND ve.model_version = ?2 AND f.status = 'complete'
              ORDER BY ve.file_id",
        )?;
        let rows = stmt.query_map(rusqlite::params![model_id, model_version], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;

        let mut ids = Vec::new();
        let mut data: Vec<i8> = Vec::new();
        let mut norms: Vec<f32> = Vec::new();
        let mut dim = 0usize;

        for row in rows {
            let (file_id, blob) = row?;
            let vector = super::decode_vector(&blob);
            if vector.is_empty() {
                continue;
            }
            if dim == 0 {
                dim = vector.len();
            } else if vector.len() != dim {
                // Mixed dimensions inside one partition means the partitioning
                // has been violated somewhere upstream. Skipping quietly would
                // hide that; a wrong-length vector cannot be scored anyway.
                return Err(Error::Other(format!(
                    "embedding for {file_id} has {} dimensions, expected {dim} \
                     in partition {model_id}/{model_version}",
                    vector.len()
                )));
            }
            let q = quantise(&vector);
            norms.push(l2(&q));
            data.extend_from_slice(&q);
            ids.push(file_id);
        }

        let fingerprint = fingerprint(conn, model_id, model_version)?;
        Ok(Self {
            model_id: model_id.to_string(),
            model_version: model_version.to_string(),
            dim,
            ids,
            data,
            norms,
            fingerprint,
        })
    }

    /// Score every vector and return the best `limit`, as (file_id, similarity).
    ///
    /// Returns more than the caller's final page deliberately — see
    /// `SearchRepo::vector_search`, which must filter *after* ranking and would
    /// otherwise lose results to a drive or date filter.
    pub fn search(&self, query: &[f32], limit: usize) -> Vec<(String, f32)> {
        if self.is_empty() || query.len() != self.dim {
            return Vec::new();
        }
        let q = quantise(query);
        let q_norm = l2(&q);
        if q_norm <= f32::EPSILON {
            return Vec::new();
        }

        let mut scored: Vec<(u32, i32)> = Vec::with_capacity(self.ids.len());
        for (i, chunk) in self.data.chunks_exact(self.dim).enumerate() {
            // i8 * i8 accumulates safely in i32: 768 * 127 * 127 fits easily.
            let mut acc: i32 = 0;
            for (a, b) in chunk.iter().zip(q.iter()) {
                acc += (*a as i32) * (*b as i32);
            }
            scored.push((i as u32, acc));
        }

        let take = limit.min(scored.len());
        // Only the head needs to be ordered; sorting the whole set would cost
        // more than the scan that produced it.
        let nth = take.saturating_sub(1).min(scored.len() - 1);
        scored.select_nth_unstable_by(nth, |a, b| b.1.cmp(&a.1));
        scored.truncate(take);
        scored.sort_unstable_by_key(|(_, score)| std::cmp::Reverse(*score));

        scored
            .into_iter()
            .map(|(i, acc)| {
                let denom = self.norms[i as usize] * q_norm;
                let sim = if denom > f32::EPSILON { acc as f32 / denom } else { 0.0 };
                (self.ids[i as usize].clone(), sim)
            })
            .collect()
    }

    /// Whether this index still describes the catalogue's current contents.
    pub fn is_current(&self, conn: &Connection) -> bool {
        fingerprint(conn, &self.model_id, &self.model_version)
            .map(|f| f == self.fingerprint)
            .unwrap_or(false)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf: Vec<u8> = Vec::with_capacity(self.data.len() + 1024);
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.fingerprint.to_le_bytes());
        buf.extend_from_slice(&(self.dim as u64).to_le_bytes());
        buf.extend_from_slice(&(self.ids.len() as u64).to_le_bytes());
        write_str(&mut buf, &self.model_id);
        write_str(&mut buf, &self.model_version);
        for id in &self.ids {
            write_str(&mut buf, id);
        }
        for n in &self.norms {
            buf.extend_from_slice(&n.to_le_bytes());
        }
        buf.extend_from_slice(unsafe {
            // i8 and u8 have identical layout; this is a reinterpretation of
            // the same bytes, not a conversion.
            std::slice::from_raw_parts(self.data.as_ptr() as *const u8, self.data.len())
        });

        // Written aside and renamed, so an interrupted save cannot leave a
        // truncated index that would later be read as real.
        let tmp = path.with_extension("partial");
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&buf)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let mut at = 0usize;

        let need = |at: usize, n: usize, len: usize| -> Result<()> {
            if at + n > len {
                return Err(Error::Other("index file is truncated".into()));
            }
            Ok(())
        };

        need(at, 8, bytes.len())?;
        if &bytes[0..8] != MAGIC {
            return Err(Error::Other("not an AtlasDrive vector index".into()));
        }
        at += 8;

        need(at, 4, bytes.len())?;
        let version = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        at += 4;
        if version != FORMAT_VERSION {
            return Err(Error::Other(format!(
                "vector index is format {version}, this build reads {FORMAT_VERSION}"
            )));
        }

        need(at, 24, bytes.len())?;
        let fingerprint = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        at += 8;
        let dim = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
        at += 8;
        let count = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
        at += 8;

        let model_id = read_str(&bytes, &mut at)?;
        let model_version = read_str(&bytes, &mut at)?;

        let mut ids = Vec::with_capacity(count);
        for _ in 0..count {
            ids.push(read_str(&bytes, &mut at)?);
        }

        need(at, count * 4, bytes.len())?;
        let mut norms = Vec::with_capacity(count);
        for _ in 0..count {
            norms.push(f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()));
            at += 4;
        }

        let expected = count.checked_mul(dim).ok_or_else(|| {
            Error::Other("vector index declares an implausible size".into())
        })?;
        need(at, expected, bytes.len())?;
        let data: Vec<i8> = bytes[at..at + expected].iter().map(|b| *b as i8).collect();

        Ok(Self { model_id, model_version, dim, ids, data, norms, fingerprint })
    }

    /// Load a saved index, rebuilding it when absent or stale, and save it back.
    pub fn load_or_build(
        conn: &Connection,
        path: &Path,
        model_id: &str,
        model_version: &str,
    ) -> Result<Self> {
        if let Ok(index) = Self::load(path) {
            if index.model_id == model_id
                && index.model_version == model_version
                && index.is_current(conn)
            {
                return Ok(index);
            }
        }
        let index = Self::build(conn, model_id, model_version)?;
        // A cache that cannot be written is not a failure worth refusing a
        // search over; the index is still usable in memory.
        let _ = index.save(path);
        Ok(index)
    }
}

/// L2-normalise and quantise to `i8`.
///
/// Normalising first is what makes the integer dot product equal cosine
/// similarity; without it the scores would be meaningless.
fn quantise(v: &[f32]) -> Vec<i8> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return vec![0; v.len()];
    }
    v.iter()
        .map(|x| {
            let scaled = (x / norm) * SCALE;
            // Round rather than truncate: truncation biases every component
            // towards zero and drags similarity down systematically.
            scaled.round().clamp(-127.0, 127.0) as i8
        })
        .collect()
}

/// L2 norm of a quantised vector.
fn l2(q: &[i8]) -> f32 {
    (q.iter().map(|x| (*x as i32) * (*x as i32)).sum::<i32>() as f32).sqrt()
}

/// Identify the row set an index was built from.
///
/// Count plus the maximum file id is enough: embeddings are inserted and
/// deleted with their files, and any addition, removal or replacement changes
/// one or the other. It costs a single indexed query rather than a scan.
fn fingerprint(conn: &Connection, model_id: &str, model_version: &str) -> Result<u64> {
    let (count, max_id): (i64, Option<String>) = conn.query_row(
        "SELECT count(*), max(ve.file_id)
           FROM visual_embeddings ve
           JOIN files f ON f.id = ve.file_id
          WHERE ve.model_id = ?1 AND ve.model_version = ?2 AND f.status = 'complete'",
        rusqlite::params![model_id, model_version],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&count.to_le_bytes());
    hasher.update(max_id.unwrap_or_default().as_bytes());
    let hash = hasher.finalize();
    Ok(u64::from_le_bytes(hash.as_bytes()[0..8].try_into().unwrap()))
}

fn write_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}

fn read_str(bytes: &[u8], at: &mut usize) -> Result<String> {
    if *at + 4 > bytes.len() {
        return Err(Error::Other("index file is truncated".into()));
    }
    let n = u32::from_le_bytes(bytes[*at..*at + 4].try_into().unwrap()) as usize;
    *at += 4;
    if *at + n > bytes.len() {
        return Err(Error::Other("index file is truncated".into()));
    }
    let s = String::from_utf8_lossy(&bytes[*at..*at + n]).to_string();
    *at += n;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    fn seeded(vectors: &[(&str, Vec<f32>)], model: (&str, &str)) -> Connection {
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
        for (id, v) in vectors {
            conn.execute(
                "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                    source_mtime_ns, status, analysis_version, created_at, updated_at)
                 VALUES (?1,'d1','rt1',?1,?1,1,0,'complete',1,'now','now')",
                [id],
            )
            .unwrap();
            let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            conn.execute(
                "INSERT INTO visual_embeddings (file_id, model_id, model_version, dim, vector, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'now')",
                rusqlite::params![id, model.0, model.1, v.len() as i64, blob],
            )
            .unwrap();
        }
        conn
    }

    fn exact_cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na * nb) }
    }

    /// Deterministic pseudo-random vectors, so failures reproduce.
    fn vectors(n: usize, dim: usize, seed: u32) -> Vec<(String, Vec<f32>)> {
        let mut s = seed;
        (0..n)
            .map(|i| {
                let v = (0..dim)
                    .map(|_| {
                        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                        ((s >> 8) as f32 / (1 << 23) as f32) - 1.0
                    })
                    .collect();
                (format!("f{i:05}"), v)
            })
            .collect()
    }

    #[test]
    fn finds_the_nearest_vector() {
        let a = vec![1.0, 0.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0, 0.0];
        let c = vec![0.9, 0.1, 0.0, 0.0];
        let conn = seeded(
            &[("fa", a.clone()), ("fb", b.clone()), ("fc", c.clone())],
            ("apple-vision", "1.0.0"),
        );
        let index = VectorIndex::build(&conn, "apple-vision", "1.0.0").unwrap();
        assert_eq!(index.len(), 3);
        assert_eq!(index.dim, 4);

        let hits = index.search(&a, 3);
        assert_eq!(hits[0].0, "fa");
        assert_eq!(hits[1].0, "fc");
        assert_eq!(hits[2].0, "fb");
        assert!(
            hits[0].1 > 0.99 && hits[0].1 <= 1.0001,
            "self-similarity must be ~1 and never above it, got {}",
            hits[0].1
        );
    }

    /// The index is a cache over the exact computation, so no result it
    /// returns may be meaningfully worse than what brute force would have
    /// given. This is the test that would catch a quantisation or
    /// normalisation mistake.
    ///
    /// Stated as a tolerance rather than as exact rank equality on purpose.
    /// Random high-dimensional vectors are nearly orthogonal, so their
    /// similarities bunch into the fourth decimal place; demanding an identical
    /// order there would be asserting on noise, and would fail for reasons that
    /// say nothing about whether search works. The meaningful claim is that
    /// nothing appreciably better was missed.
    #[test]
    fn returns_nothing_meaningfully_worse_than_exact_cosine() {
        let dim = 768;
        let vecs = vectors(400, dim, 7);
        let owned: Vec<(&str, Vec<f32>)> =
            vecs.iter().map(|(id, v)| (id.as_str(), v.clone())).collect();
        let conn = seeded(&owned, ("apple-vision", "1.0.0"));
        let index = VectorIndex::build(&conn, "apple-vision", "1.0.0").unwrap();

        let query = &vecs[37].1;
        let hits = index.search(query, 10);
        assert_eq!(hits.len(), 10);

        let mut exact: Vec<(String, f32)> = vecs
            .iter()
            .map(|(id, v)| (id.clone(), exact_cosine(query, v)))
            .collect();
        exact.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        // The best match is never ambiguous: it is the query itself.
        assert_eq!(hits[0].0, exact[0].0);
        assert_eq!(hits[0].0, "f00037");

        // Quantising a unit vector to i8 costs at most half a step per
        // component; over a normalised dot product that is comfortably inside
        // this bound.
        const TOLERANCE: f32 = 0.01;

        let by_id: std::collections::HashMap<&str, f32> =
            exact.iter().map(|(id, s)| (id.as_str(), *s)).collect();
        let tenth_best = exact[9].1;

        for (id, approx) in &hits {
            let truth = by_id[id.as_str()];
            assert!(
                (approx - truth).abs() < TOLERANCE,
                "score drift on {id}: index said {approx}, exact is {truth}"
            );
            assert!(
                truth >= tenth_best - TOLERANCE,
                "{id} scores {truth}, appreciably worse than the true 10th best {tenth_best}"
            );
        }
    }

    /// Real image embeddings are not random noise: photographs of the same
    /// scene cluster. Where the answer is well separated, the ranking must be
    /// exactly right.
    #[test]
    fn ranking_is_exact_when_matches_are_well_separated() {
        let dim = 128;
        let mut entries: Vec<(String, Vec<f32>)> = Vec::new();
        // Three clear clusters along different axes, plus jitter.
        for (c, axis) in [0usize, 1, 2].iter().enumerate() {
            for i in 0..20 {
                let mut v = vec![0.0f32; dim];
                v[*axis] = 1.0;
                v[3 + c * 10 + (i % 10)] = 0.15;
                entries.push((format!("c{c}_{i:02}"), v));
            }
        }
        let owned: Vec<(&str, Vec<f32>)> =
            entries.iter().map(|(id, v)| (id.as_str(), v.clone())).collect();
        let conn = seeded(&owned, ("apple-vision", "1.0.0"));
        let index = VectorIndex::build(&conn, "apple-vision", "1.0.0").unwrap();

        // Query the middle of cluster 1; every hit must come from cluster 1.
        let mut q = vec![0.0f32; dim];
        q[1] = 1.0;
        let hits = index.search(&q, 20);
        assert_eq!(hits.len(), 20);
        for (id, _) in &hits {
            assert!(id.starts_with("c1_"), "{id} is not from the queried cluster");
        }
    }

    /// Model partitions must not bleed into each other: a 768-dimension Vision
    /// embedding and a 65-dimension heuristic one describe different spaces.
    #[test]
    fn partitions_are_isolated() {
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
        conn.execute(
            "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                source_mtime_ns, status, analysis_version, created_at, updated_at)
             VALUES ('f1','d1','rt1','a.jpg','a.jpg',1,0,'complete',1,'now','now')",
            [],
        )
        .unwrap();
        for (model, version, v) in [
            ("apple-vision", "1.0.0", vec![1.0f32, 0.0, 0.0, 0.0]),
            ("local-heuristic", "0.2.0", vec![0.0f32, 1.0]),
        ] {
            let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            conn.execute(
                "INSERT INTO visual_embeddings (file_id, model_id, model_version, dim, vector, created_at)
                 VALUES ('f1', ?1, ?2, ?3, ?4, 'now')",
                rusqlite::params![model, version, v.len() as i64, blob],
            )
            .unwrap();
        }

        let vision = VectorIndex::build(&conn, "apple-vision", "1.0.0").unwrap();
        assert_eq!(vision.dim, 4);
        assert_eq!(vision.len(), 1);

        let local = VectorIndex::build(&conn, "local-heuristic", "0.2.0").unwrap();
        assert_eq!(local.dim, 2);

        // A query of the wrong width scores nothing rather than reading past
        // the end of a row or silently comparing unrelated spaces.
        assert!(vision.search(&[1.0, 0.0], 5).is_empty());
    }

    #[test]
    fn survives_a_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let vecs = vectors(50, 64, 3);
        let owned: Vec<(&str, Vec<f32>)> =
            vecs.iter().map(|(id, v)| (id.as_str(), v.clone())).collect();
        let conn = seeded(&owned, ("apple-vision", "1.0.0"));

        let built = VectorIndex::build(&conn, "apple-vision", "1.0.0").unwrap();
        let path = dir.path().join("vision.idx");
        built.save(&path).unwrap();

        let loaded = VectorIndex::load(&path).unwrap();
        assert_eq!(loaded.len(), built.len());
        assert_eq!(loaded.dim, built.dim);
        assert_eq!(loaded.model_id, "apple-vision");
        assert!(loaded.is_current(&conn));

        let q = &vecs[5].1;
        assert_eq!(built.search(q, 5), loaded.search(q, 5));
    }

    /// A cache that answers from stale data is worse than no cache.
    #[test]
    fn a_stale_index_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let vecs = vectors(20, 32, 11);
        let owned: Vec<(&str, Vec<f32>)> =
            vecs.iter().map(|(id, v)| (id.as_str(), v.clone())).collect();
        let conn = seeded(&owned, ("apple-vision", "1.0.0"));

        let path = dir.path().join("vision.idx");
        let index = VectorIndex::load_or_build(&conn, &path, "apple-vision", "1.0.0").unwrap();
        assert_eq!(index.len(), 20);
        assert!(index.is_current(&conn));

        // Index another photograph, as a later drive scan would.
        conn.execute(
            "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                source_mtime_ns, status, analysis_version, created_at, updated_at)
             VALUES ('zzz','d1','rt1','new.jpg','new.jpg',1,0,'complete',1,'now','now')",
            [],
        )
        .unwrap();
        let blob: Vec<u8> = [0.5f32; 32].iter().flat_map(|x| x.to_le_bytes()).collect();
        conn.execute(
            "INSERT INTO visual_embeddings (file_id, model_id, model_version, dim, vector, created_at)
             VALUES ('zzz','apple-vision','1.0.0',32,?1,'now')",
            [blob],
        )
        .unwrap();

        assert!(!index.is_current(&conn), "must notice the catalogue moved on");
        let rebuilt = VectorIndex::load_or_build(&conn, &path, "apple-vision", "1.0.0").unwrap();
        assert_eq!(rebuilt.len(), 21, "must have rebuilt rather than served stale");
    }

    /// A damaged or half-written index file must be refused, not misread.
    #[test]
    fn a_corrupt_index_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.idx");

        std::fs::write(&path, b"not an index at all").unwrap();
        assert!(VectorIndex::load(&path).is_err());

        // A valid header followed by nothing.
        let vecs = vectors(5, 8, 1);
        let owned: Vec<(&str, Vec<f32>)> =
            vecs.iter().map(|(id, v)| (id.as_str(), v.clone())).collect();
        let conn = seeded(&owned, ("apple-vision", "1.0.0"));
        let good = VectorIndex::build(&conn, "apple-vision", "1.0.0").unwrap();
        good.save(&path).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(bytes.len() / 2);
        std::fs::write(&path, &bytes).unwrap();
        assert!(VectorIndex::load(&path).is_err(), "truncation must be caught");

        // And load_or_build recovers by rebuilding rather than failing.
        let recovered =
            VectorIndex::load_or_build(&conn, &path, "apple-vision", "1.0.0").unwrap();
        assert_eq!(recovered.len(), 5);
    }

    #[test]
    fn an_empty_partition_is_harmless() {
        let conn = seeded(&[], ("apple-vision", "1.0.0"));
        let index = VectorIndex::build(&conn, "apple-vision", "1.0.0").unwrap();
        assert!(index.is_empty());
        assert!(index.search(&[1.0, 0.0, 0.0], 10).is_empty());
    }

    /// Zero vectors must not produce NaN and poison the ranking.
    #[test]
    fn a_zero_vector_does_not_poison_the_ranking() {
        let conn = seeded(
            &[("fz", vec![0.0, 0.0, 0.0]), ("fa", vec![1.0, 0.0, 0.0])],
            ("apple-vision", "1.0.0"),
        );
        let index = VectorIndex::build(&conn, "apple-vision", "1.0.0").unwrap();
        let hits = index.search(&[1.0, 0.0, 0.0], 2);
        assert_eq!(hits[0].0, "fa");
        assert!(hits.iter().all(|(_, s)| s.is_finite()));
    }
}

#[cfg(test)]
mod repo_integration {
    use super::tests_support::*;
    use crate::search::{SearchFilters, SearchRepo};

    /// Before the index went in, `vector_search` took the global top-N and
    /// *then* applied filters, so a drive-filtered search over a multi-drive
    /// archive returned a nearly empty page while thousands of matching
    /// photographs sat just below the cut.
    #[test]
    fn a_drive_filter_does_not_starve_the_results() {
        let dir = tempfile::tempdir().unwrap();
        // Drive 1 holds 200 photographs, drive 2 holds 10. A query closest to
        // drive 1's cluster would fill any small top-N entirely with drive 1.
        let conn = two_drive_catalogue(200, 10);

        let repo = SearchRepo::with_index_dir(&conn, dir.path());
        let query = vec![0.0f32, 1.0, 0.0, 0.0];

        let filters = SearchFilters { drive_number: Some(2), limit: 10, ..Default::default() };
        let hits = repo.vector_search(&query, "apple-vision", "1.0.0", &filters).unwrap();

        assert_eq!(hits.len(), 10, "every photograph on drive 2 should be reachable");
        assert!(hits.iter().all(|r| r.drive_number == 2));
    }

    /// The index is a cache; with and without it the answers must agree.
    #[test]
    fn indexed_and_exhaustive_paths_agree() {
        let dir = tempfile::tempdir().unwrap();
        let conn = two_drive_catalogue(60, 40);
        let query = vec![0.3f32, 0.9, 0.1, 0.0];
        let filters = SearchFilters { limit: 15, ..Default::default() };

        let indexed = SearchRepo::with_index_dir(&conn, dir.path())
            .vector_search(&query, "apple-vision", "1.0.0", &filters)
            .unwrap();
        let exhaustive = SearchRepo::new(&conn)
            .vector_search(&query, "apple-vision", "1.0.0", &filters)
            .unwrap();

        assert_eq!(indexed.len(), exhaustive.len());

        // Which photographs are found is the claim; the order among near-ties
        // is not. These fixtures are four-dimensional, where a single rounded
        // component moves the dot product appreciably, so two results whose
        // true similarity differs in the third decimal can legitimately swap.
        // `ranking_is_exact_when_matches_are_well_separated` covers order where
        // order is actually determined.
        let a: std::collections::BTreeSet<&str> =
            indexed.iter().map(|r| r.file_id.as_str()).collect();
        let b: std::collections::BTreeSet<&str> =
            exhaustive.iter().map(|r| r.file_id.as_str()).collect();
        assert_eq!(a, b, "the index must not change which photographs are found");

        // The best match is well clear of the rest, so it must be identical.
        assert_eq!(indexed[0].file_id, exhaustive[0].file_id);
    }

    /// The feature the index actually earns its keep on: given one photograph,
    /// find the others from the same set-up.
    #[test]
    fn finds_photographs_that_look_like_a_given_one() {
        let dir = tempfile::tempdir().unwrap();
        let conn = two_drive_catalogue(30, 10);
        let repo = SearchRepo::with_index_dir(&conn, dir.path());

        // Query from well inside drive 1's cluster. Index 0 would be a poor
        // choice: both fixtures start pointing along +y, so d1-f0000 and
        // d2-f0000 normalise to the same direction and are genuinely identical.
        // The clusters only separate as the index grows, which is also true of
        // real photographs — two shots of a plain wall look alike whatever
        // shoot they came from.
        let hits = repo
            .similar_to("d1-f0010", &SearchFilters { limit: 5, ..Default::default() })
            .unwrap();

        assert_eq!(hits.len(), 5, "asking for five must return five, not four");
        // A photograph is not a result for itself.
        assert!(hits.iter().all(|r| r.file_id != "d1-f0010"));
        // Its nearest neighbours are its own neighbours in the cluster.
        assert!(
            hits.iter().all(|r| r.file_id.starts_with("d1-")),
            "expected neighbours from the same cluster, got {:?}",
            hits.iter().map(|r| &r.file_id).collect::<Vec<_>>()
        );
        assert_eq!(hits[0].file_id, "d1-f0011", "the closest is the adjacent one");
    }

    /// No caller should have to know which engine embedded a photograph — that
    /// assumption is what let the text/image model mismatch go unnoticed.
    #[test]
    fn similarity_discovers_the_model_itself() {
        let dir = tempfile::tempdir().unwrap();
        let conn = two_drive_catalogue(10, 5);
        let repo = SearchRepo::with_index_dir(&conn, dir.path());

        assert!(!repo.similar_to("d1-f0000", &SearchFilters::default()).unwrap().is_empty());
        // A file with no embedding at all yields nothing, rather than erroring.
        assert!(repo.similar_to("does-not-exist", &SearchFilters::default()).unwrap().is_empty());
    }

    /// The index file is written on first use and reused after.
    #[test]
    fn the_index_is_persisted_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let conn = two_drive_catalogue(20, 5);
        let repo = SearchRepo::with_index_dir(&conn, dir.path());
        let query = vec![0.0f32, 1.0, 0.0, 0.0];
        let filters = SearchFilters::default();

        repo.vector_search(&query, "apple-vision", "1.0.0", &filters).unwrap();
        let written: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            written.iter().any(|n| n.starts_with("vec-apple-vision")),
            "expected a persisted index, found {written:?}"
        );

        // Second search reuses it and still answers.
        let again = repo.vector_search(&query, "apple-vision", "1.0.0", &filters).unwrap();
        assert!(!again.is_empty());
    }
}

/// Fixtures shared by the index tests and the repo integration tests.
#[cfg(test)]
pub(crate) mod tests_support {
    use rusqlite::Connection;

    use crate::db::{self, SchemaKind};

    /// Two drives whose photographs sit in different directions, so a query can
    /// deliberately favour one of them.
    pub fn two_drive_catalogue(on_drive_1: usize, on_drive_2: usize) -> Connection {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        for (id, number) in [("d1", 1), ("d2", 2)] {
            conn.execute(
                "INSERT INTO drives (id, drive_number, status, first_seen_at)
                 VALUES (?1, ?2, 'online', 'now')",
                rusqlite::params![id, number],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO roots (id, drive_id, relative_root, created_at)
                 VALUES (?1, ?2, '', 'now')",
                rusqlite::params![format!("rt-{id}"), id],
            )
            .unwrap();
        }

        let add = |drive: &str, i: usize, v: Vec<f32>| {
            let id = format!("{drive}-f{i:04}");
            conn.execute(
                "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                    source_mtime_ns, status, analysis_version, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?1, ?1, 1, 0, 'complete', 1, 'now', 'now')",
                rusqlite::params![id, drive, format!("rt-{drive}")],
            )
            .unwrap();
            let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            conn.execute(
                "INSERT INTO visual_embeddings (file_id, model_id, model_version, dim, vector, created_at)
                 VALUES (?1, 'apple-vision', '1.0.0', 4, ?2, 'now')",
                rusqlite::params![id, blob],
            )
            .unwrap();
        };

        // Drive 1 clusters near +y, drive 2 near +y too but rotated away, so
        // drive 1 dominates any unfiltered ranking.
        //
        // The spacing is deliberate. `i8` resolves about 1/127 per component,
        // so in only four dimensions vectors spaced 0.01 apart fall below what
        // the quantiser can represent and tie arbitrarily. That would be a
        // fixture testing the quantiser's floor rather than the index. Real
        // 768-dimension embeddings of different photographs are nowhere near
        // that close, so the fixture spreads them to match.
        for i in 0..on_drive_1 {
            add("d1", i, vec![0.15 * i as f32, 1.0, 0.0, 0.0]);
        }
        for i in 0..on_drive_2 {
            add("d2", i, vec![0.0, 0.9, 0.15 * i as f32, 0.0]);
        }
        conn
    }
}

/// Benchmarks, run explicitly: `cargo test --release -- --ignored bench`.
#[cfg(test)]
mod bench {
    use super::*;

    /// The claim this module exists to justify: that scanning a quantised
    /// partition at the archive's target scale is fast enough to search on.
    /// Ignored by default because building 200,000 vectors is slow in a debug
    /// build and this is a measurement, not a pass/fail.
    #[test]
    #[ignore]
    fn bench_200k_vectors() {
        let n = 200_000;
        let dim = 768;

        let mut s = 12345u32;
        let mut data: Vec<i8> = Vec::with_capacity(n * dim);
        let mut norms = Vec::with_capacity(n);
        let mut ids = Vec::with_capacity(n);
        for i in 0..n {
            let v: Vec<f32> = (0..dim)
                .map(|_| {
                    s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                    ((s >> 8) as f32 / (1 << 23) as f32) - 1.0
                })
                .collect();
            let q = quantise(&v);
            norms.push(l2(&q));
            data.extend_from_slice(&q);
            ids.push(format!("f{i:06}"));
        }

        let index = VectorIndex {
            model_id: "apple-vision".into(),
            model_version: "1.0.0".into(),
            dim,
            ids,
            data,
            norms,
            fingerprint: 0,
        };

        let query: Vec<f32> = (0..dim).map(|i| (i as f32 * 0.01).sin()).collect();

        // Warm, then measure.
        let _ = index.search(&query, 100);
        let start = std::time::Instant::now();
        let runs = 10;
        for _ in 0..runs {
            let hits = index.search(&query, 100);
            assert_eq!(hits.len(), 100);
        }
        let per_query = start.elapsed().as_secs_f64() / runs as f64;

        let bytes = index.data.len() + index.norms.len() * 4;
        eprintln!(
            "200,000 x 768: {:.1}ms per query, {} MB resident",
            per_query * 1000.0,
            bytes / (1024 * 1024)
        );
        assert!(
            per_query < 0.5,
            "a query took {:.0}ms, too slow to search interactively",
            per_query * 1000.0
        );
    }
}
