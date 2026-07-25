//! Face clustering, people and the human review boundary
//! (see `docs/08_FACE_RECOGNITION_AND_REVIEW.md`).
//!
//! The app clusters and suggests; it never names a person automatically. Face
//! embeddings are stored encrypted and only decrypted in memory for clustering.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::crypto::{self, MasterKey, Sealed};
use crate::error::Result;
use crate::util::{cosine_similarity, new_uuid, now_iso8601};

pub const CLUSTER_ALGO_VERSION: &str = "greedy-cosine-0.1.0";
/// Cosine similarity at or above which two faces join the same cluster.
pub const DEFAULT_CLUSTER_THRESHOLD: f32 = 0.92;

/// A person record (human-confirmed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    pub display_name: String,
    pub aliases: Vec<String>,
    pub relationship: Option<String>,
}

/// A candidate cluster prepared for human review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterSummary {
    pub cluster_id: String,
    pub status: String,
    pub face_count: i64,
    pub person_id: Option<String>,
    pub label: Option<String>,
}

/// Repository for face operations over `archive.db`.
pub struct FaceRepo<'a> {
    conn: &'a Connection,
}

impl<'a> FaceRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Insert a detected face and its encrypted embedding.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_face(
        &self,
        file_id: &str,
        bbox: (f32, f32, f32, f32),
        quality: f32,
        model_id: &str,
        model_version: &str,
        embedding: &[f32],
        key: &MasterKey,
    ) -> Result<String> {
        let face_id = new_uuid();
        self.conn.execute(
            "INSERT INTO faces (id, file_id, bbox_x, bbox_y, bbox_w, bbox_h, quality, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![face_id, file_id, bbox.0, bbox.1, bbox.2, bbox.3, quality, now_iso8601()],
        )?;
        let sealed = crypto::seal_vector(key, embedding)?;
        self.conn.execute(
            "INSERT INTO face_embeddings
             (face_id, model_id, model_version, dim, ciphertext, nonce, enc_version, key_version, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                face_id, model_id, model_version, embedding.len() as i64,
                sealed.ciphertext, sealed.nonce, sealed.enc_version, sealed.key_version,
                now_iso8601()
            ],
        )?;
        Ok(face_id)
    }

    /// Decrypt all face embeddings for a model partition (in memory only).
    fn load_embeddings(
        &self,
        model_id: &str,
        model_version: &str,
        key: &MasterKey,
    ) -> Result<Vec<(String, Vec<f32>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT fe.face_id, fe.ciphertext, fe.nonce, fe.enc_version, fe.key_version
             FROM face_embeddings fe
             JOIN faces f ON f.id = fe.face_id
             WHERE fe.model_id=?1 AND fe.model_version=?2
               AND f.is_false_detection=0 AND f.is_ignored=0",
        )?;
        let rows = stmt.query_map(params![model_id, model_version], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Sealed {
                    ciphertext: r.get::<_, Vec<u8>>(1)?,
                    nonce: r.get::<_, Vec<u8>>(2)?,
                    enc_version: r.get::<_, i64>(3)?,
                    key_version: r.get::<_, i64>(4)?,
                },
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, sealed) = row?;
            let v = crypto::open_vector(key, &sealed)?;
            out.push((id, v));
        }
        Ok(out)
    }

    /// Rebuild clusters via greedy cosine agglomeration.
    ///
    /// Preserves confirmed person records and manual links, records the
    /// algorithm version, and snapshots existing clusters first so the operation
    /// is reversible. Does *not* reopen original images.
    pub fn rebuild_clusters(
        &self,
        model_id: &str,
        model_version: &str,
        key: &MasterKey,
        threshold: f32,
    ) -> Result<usize> {
        // Reversible snapshot of current cluster assignments.
        self.snapshot_clusters()?;

        let embeddings = self.load_embeddings(model_id, model_version, key)?;

        let tx = self.conn.unchecked_transaction()?;
        // Clear only *unconfirmed* cluster assignments; keep confirmed links.
        tx.execute(
            "UPDATE faces SET cluster_id = NULL
             WHERE cluster_id IN (SELECT id FROM face_clusters WHERE status <> 'confirmed')",
            [],
        )?;
        tx.execute("DELETE FROM face_clusters WHERE status <> 'confirmed'", [])?;

        // Greedy: assign each face to the first cluster whose centroid is close.
        let mut centroids: Vec<(String, Vec<f32>, usize)> = Vec::new(); // (cluster_id, centroid, count)
        for (face_id, vec) in &embeddings {
            let mut best: Option<(usize, f32)> = None;
            for (i, (_cid, centroid, _n)) in centroids.iter().enumerate() {
                let sim = cosine_similarity(vec, centroid);
                if sim >= threshold && best.map(|(_, s)| sim > s).unwrap_or(true) {
                    best = Some((i, sim));
                }
            }
            let cluster_id = match best {
                Some((i, _)) => {
                    // Update running centroid.
                    let (cid, centroid, n) = &mut centroids[i];
                    for k in 0..centroid.len() {
                        centroid[k] = (centroid[k] * *n as f32 + vec[k]) / (*n as f32 + 1.0);
                    }
                    *n += 1;
                    cid.clone()
                }
                None => {
                    let cid = new_uuid();
                    tx.execute(
                        "INSERT INTO face_clusters (id, status, algorithm_version, created_at, updated_at)
                         VALUES (?1,'unnamed',?2,?3,?3)",
                        params![cid, CLUSTER_ALGO_VERSION, now_iso8601()],
                    )?;
                    centroids.push((cid.clone(), vec.clone(), 1));
                    cid
                }
            };
            tx.execute(
                "UPDATE faces SET cluster_id=?2 WHERE id=?1",
                params![face_id, cluster_id],
            )?;
        }
        tx.commit()?;
        Ok(centroids.len())
    }

    /// Snapshot current cluster assignments into a JSON report for reversal.
    fn snapshot_clusters(&self) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, cluster_id FROM faces WHERE cluster_id IS NOT NULL")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let assignments = serde_json::to_string(&rows)?;
        self.conn.execute(
            "INSERT INTO cluster_snapshots (id, taken_at, assignments) VALUES (?1,?2,?3)",
            params![new_uuid(), now_iso8601(), assignments],
        )?;
        Ok(())
    }

    /// Create a person (explicit human action).
    pub fn create_person(&self, display_name: &str, relationship: Option<&str>) -> Result<Person> {
        let id = new_uuid();
        self.conn.execute(
            "INSERT INTO people (id, display_name, aliases_json, relationship, created_at, updated_at)
             VALUES (?1,?2,'[]',?3,?4,?4)",
            params![id, display_name, relationship, now_iso8601()],
        )?;
        Ok(Person {
            id,
            display_name: display_name.to_string(),
            aliases: vec![],
            relationship: relationship.map(|s| s.to_string()),
        })
    }

    /// Name a cluster by confirming it belongs to a person (human confirmation).
    pub fn name_cluster(&self, cluster_id: &str, person_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE face_clusters SET status='confirmed', person_id=?2, updated_at=?3 WHERE id=?1",
            params![cluster_id, person_id, now_iso8601()],
        )?;
        tx.execute(
            "INSERT INTO face_person_links (id, cluster_id, person_id, source, confidence, is_confirmed, created_at)
             VALUES (?1,?2,?3,'user',1.0,1,?4)",
            params![new_uuid(), cluster_id, person_id, now_iso8601()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Merge two clusters (source folds into target).
    pub fn merge_clusters(&self, target: &str, source: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE faces SET cluster_id=?1 WHERE cluster_id=?2",
            params![target, source],
        )?;
        tx.execute(
            "UPDATE face_clusters SET status='merged', updated_at=?2 WHERE id=?1",
            params![source, now_iso8601()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Move a face out of its cluster into a new one (split).
    pub fn split_face(&self, face_id: &str) -> Result<String> {
        let new_cluster = new_uuid();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO face_clusters (id, status, algorithm_version, created_at, updated_at)
             VALUES (?1,'unnamed',?2,?3,?3)",
            params![new_cluster, CLUSTER_ALGO_VERSION, now_iso8601()],
        )?;
        tx.execute(
            "UPDATE faces SET cluster_id=?2 WHERE id=?1",
            params![face_id, new_cluster],
        )?;
        tx.commit()?;
        Ok(new_cluster)
    }

    /// Unlink an incorrect face from any cluster.
    pub fn unlink_face(&self, face_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE faces SET cluster_id=NULL WHERE id=?1",
            [face_id],
        )?;
        Ok(())
    }

    /// Mark a detection as a false face (kept for audit, ignored downstream).
    pub fn mark_false_detection(&self, face_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE faces SET is_false_detection=1, cluster_id=NULL WHERE id=?1",
            [face_id],
        )?;
        Ok(())
    }

    /// Delete a person's derived face data (privacy control).
    pub fn delete_person_face_data(&self, person_id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM face_person_links WHERE person_id=?1",
            [person_id],
        )?;
        tx.execute(
            "UPDATE face_clusters SET person_id=NULL, status='unnamed' WHERE person_id=?1",
            [person_id],
        )?;
        tx.execute("DELETE FROM people WHERE id=?1", [person_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Prepare a bounded batch of unnamed clusters for human review.
    pub fn prepare_review(&self, limit: usize) -> Result<Vec<ClusterSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.status, c.person_id, c.label, count(f.id) as n
             FROM face_clusters c
             LEFT JOIN faces f ON f.cluster_id = c.id
             WHERE c.status = 'unnamed'
             GROUP BY c.id
             ORDER BY n DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(ClusterSummary {
                cluster_id: r.get(0)?,
                status: r.get(1)?,
                person_id: r.get(2)?,
                label: r.get(3)?,
                face_count: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn cluster_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM face_clusters", [], |r| r.get(0))?)
    }

    /// Sanity stats for the verifier's face-pipeline checks.
    pub fn embedding_health(&self, model_id: &str, model_version: &str, key: &MasterKey) -> Result<FaceHealth> {
        let embeddings = self.load_embeddings(model_id, model_version, key)?;
        let mut health = FaceHealth {
            total: embeddings.len(),
            ..Default::default()
        };
        if embeddings.is_empty() {
            return Ok(health);
        }
        let dim = embeddings[0].1.len();
        health.dim = dim;
        let mut seen: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for (_, v) in &embeddings {
            if v.len() != dim {
                health.dim_mismatches += 1;
            }
            if v.iter().any(|x| !x.is_finite()) {
                health.non_finite += 1;
            }
            // Quantized fingerprint to detect suspicious exact repeats.
            let mut h = 0u64;
            for x in v {
                h = h.wrapping_mul(131).wrapping_add((*x * 1000.0) as i64 as u64);
            }
            *seen.entry(h).or_insert(0) += 1;
        }
        health.max_identical = seen.values().copied().max().unwrap_or(0);
        Ok(health)
    }

    pub fn get_person(&self, id: &str) -> Result<Option<Person>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, display_name, aliases_json, relationship FROM people WHERE id=?1",
                [id],
                |r| {
                    let aliases: Option<String> = r.get(2)?;
                    Ok(Person {
                        id: r.get(0)?,
                        display_name: r.get(1)?,
                        aliases: aliases
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or_default(),
                        relationship: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }
}

/// Face-pipeline health snapshot.
#[derive(Debug, Clone, Default)]
pub struct FaceHealth {
    pub total: usize,
    pub dim: usize,
    pub dim_mismatches: usize,
    pub non_finite: usize,
    pub max_identical: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, SchemaKind};

    fn setup() -> (Connection, String, MasterKey) {
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        // Minimal drive/root/file so face FK holds.
        conn.execute(
            "INSERT INTO drives (id, drive_number, status, first_seen_at) VALUES ('d1',1,'online','now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO roots (id, drive_id, relative_root, created_at) VALUES ('r1','d1','','now')",
            [],
        )
        .unwrap();
        let key = MasterKey::generate(1);
        (conn, "d1".to_string(), key)
    }

    fn add_file(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                source_mtime_ns, status, created_at, updated_at)
             VALUES (?1,'d1','r1',?2,?2,10,1,'complete','now','now')",
            params![id, format!("{id}.jpg")],
        )
        .unwrap();
    }

    #[test]
    fn cluster_and_name_flow() {
        let (conn, _d, key) = setup();
        add_file(&conn, "f1");
        add_file(&conn, "f2");
        add_file(&conn, "f3");
        let repo = FaceRepo::new(&conn);
        // Two near-identical embeddings + one distinct.
        let a = vec![1.0, 0.0, 0.0, 0.0];
        let a2 = vec![0.99, 0.05, 0.0, 0.0];
        let b = vec![0.0, 0.0, 1.0, 0.0];
        repo.insert_face("f1", (0.0, 0.0, 0.1, 0.1), 0.9, "m", "1", &a, &key).unwrap();
        repo.insert_face("f2", (0.0, 0.0, 0.1, 0.1), 0.9, "m", "1", &a2, &key).unwrap();
        repo.insert_face("f3", (0.0, 0.0, 0.1, 0.1), 0.9, "m", "1", &b, &key).unwrap();

        let clusters = repo.rebuild_clusters("m", "1", &key, 0.9).unwrap();
        assert_eq!(clusters, 2, "two similar faces cluster, one separate");

        let review = repo.prepare_review(10).unwrap();
        assert_eq!(review.len(), 2);
        // Largest cluster first.
        assert_eq!(review[0].face_count, 2);

        let person = repo.create_person("Grandma", Some("grandmother")).unwrap();
        repo.name_cluster(&review[0].cluster_id, &person.id).unwrap();
        // Rebuild preserves the confirmed cluster.
        let after = repo.rebuild_clusters("m", "1", &key, 0.9).unwrap();
        assert!(after >= 1);
        let p = repo.get_person(&person.id).unwrap().unwrap();
        assert_eq!(p.display_name, "Grandma");
    }

    #[test]
    fn embedding_health_flags_repeats() {
        let (conn, _d, key) = setup();
        add_file(&conn, "f1");
        add_file(&conn, "f2");
        let repo = FaceRepo::new(&conn);
        let v = vec![0.5, 0.5, 0.5, 0.5];
        repo.insert_face("f1", (0.0, 0.0, 0.1, 0.1), 0.9, "m", "1", &v, &key).unwrap();
        repo.insert_face("f2", (0.0, 0.0, 0.1, 0.1), 0.9, "m", "1", &v, &key).unwrap();
        let h = repo.embedding_health("m", "1", &key).unwrap();
        assert_eq!(h.total, 2);
        assert_eq!(h.dim, 4);
        assert_eq!(h.non_finite, 0);
        assert_eq!(h.max_identical, 2);
    }
}
