//! Face clustering, people and the human review boundary
//! (see `docs/08_FACE_RECOGNITION_AND_REVIEW.md`).
//!
//! The app clusters and suggests; it never names a person automatically. Face
//! embeddings are stored encrypted and only decrypted in memory for clustering.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::crypto::{self, MasterKey, Sealed};
use crate::error::{Error, Result};
use crate::util::{cosine_similarity, new_uuid, now_iso8601};

pub const CLUSTER_ALGO_VERSION: &str = "greedy-cosine-0.1.0";
/// Cosine similarity at or above which two faces join the same cluster,
/// for the original 32-dimension heuristic face embedding.
pub const DEFAULT_CLUSTER_THRESHOLD: f32 = 0.92;

/// Clustering threshold for Apple Vision's 768-dimension face feature prints.
///
/// Chosen by measurement, on 2,126 real faces from one wedding:
///
/// | threshold | groups | largest | faces grouped with others |
/// |---|---|---|---|
/// | 0.92 | 1,770 | 17 | 433 |
/// | 0.88 | 1,395 | 42 | 889 |
/// | 0.84 | 918 | 65 | 1,327 |
///
/// 0.92 — the value tuned for the heuristic engine's colour grid — barely
/// grouped anything, leaving 83% of faces alone and every one needing to be
/// named individually.
///
/// **What 0.84 does not achieve, stated plainly:** a wedding has perhaps 80
/// people, and this still produces 918 groups, so one person is typically split
/// across several. That is the ceiling of a general image embedding standing in
/// for a face-recognition model — pose and lighting move the vector as much as
/// identity does. Naming several groups with the same name attaches them all to
/// one person, and each confirmation makes the next scan's suggestions better,
/// so it is workable but not effortless. A real face model (D-026) is what
/// collapses this properly.
///
/// The largest group at 0.84 is 65 faces, with no sign of a runaway merge, and
/// `--threshold` lets it be tuned per archive.
pub const VISION_CLUSTER_THRESHOLD: f32 = 0.84;

/// The clustering threshold to use for a given face-embedding model.
pub fn cluster_threshold_for(model_id: &str) -> f32 {
    match model_id {
        crate::ai::vision::MODEL_ID => VISION_CLUSTER_THRESHOLD,
        _ => DEFAULT_CLUSTER_THRESHOLD,
    }
}

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

// (doc for FaceRepo moved below; the following types support recognition)
/// A proposed identity for a face, pending human confirmation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonSuggestion {
    pub person_id: String,
    pub display_name: String,
    /// Cosine similarity to the closest confirmed face of that person.
    pub score: f32,
}

/// A person the user has named, and how well established they are.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedPerson {
    pub id: String,
    pub display_name: String,
    pub relationship: Option<String>,
    /// Faces the user confirmed. These are the exemplars used for recognition.
    pub confirmed_faces: i64,
    /// Faces proposed as this person, awaiting confirmation.
    pub suggested_faces: i64,
}

/// Similarity above which a new face is worth proposing as a known person.
///
/// Derived from measurement rather than taste: across a real 758-photograph
/// wedding, unrelated face pairs sat at a median cosine of 0.53 (p95 0.75),
/// while faces of the same person scored 0.87–0.94. 0.82 sits above the noise
/// with headroom, and because a match is only ever a *suggestion*, the cost of
/// being slightly generous is a review prompt rather than a wrong name.
///
/// Note the honest limit this number encodes: those same-person scores came
/// from one event, where lighting and clothing are shared. See D-026.
///
/// Raised from 0.82 to 0.88 after seeing it in use: sweeping 2,126 real faces at
/// 0.82 proposed 637 of them across just two people, which turns "awaiting
/// confirmation" into a chore rather than a help. A missed match costs one
/// manual naming; a bad one costs the user's trust in every other suggestion.
pub const PERSON_MATCH_THRESHOLD: f32 = 0.88;

/// Longest edge of a stored face crop, in pixels.
///
/// Big enough to recognise someone at a glance in a gallery, small enough that
/// thousands of them stay a sensible size on disk.
pub const FACE_THUMBNAIL_EDGE: u32 = 200;

/// One face as shown in the gallery — a picture first, a name only if known.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryFace {
    pub face_id: String,
    pub cluster_id: Option<String>,
    pub file_id: String,
    pub quality: Option<f32>,
    /// Set only once the user has named this face's group.
    pub person_name: Option<String>,
    pub cluster_status: Option<String>,
    /// How many faces are grouped with this one.
    pub group_size: i64,
    /// Which drive this face was found on.
    ///
    /// Carried on every face because a wall of unnamed faces from twenty drives
    /// is not reviewable: "who is this?" is a much easier question when you
    /// know it came off the 2019 weddings disk.
    pub drive_number: i64,
    pub drive_name: Option<String>,
}

/// A face the app believes is a named person, awaiting a yes or no.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedFace {
    pub face_id: String,
    pub cluster_id: String,
    /// Similarity to that person's closest confirmed face, 0–1.
    pub score: f32,
    /// How many faces come with it if accepted.
    pub group_size: i64,
}

/// A folder on disk holding some of a person's photographs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonFolder {
    pub drive_number: i64,
    pub drive_name: Option<String>,
    pub online: bool,
    /// Path within the drive, e.g. `Aimee and Kent/edits`.
    pub relative_folder: String,
    /// Full path, resolvable only while the drive is connected.
    pub absolute_path: Option<String>,
    pub photo_count: i64,
}

/// A photograph containing a named person, and where to find it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonPhoto {
    pub file_id: String,
    pub filename: String,
    pub relative_path: String,
    pub drive_number: i64,
    pub drive_name: Option<String>,
    pub online: bool,
}

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

    /// Store a small encrypted crop of a face, so it can be browsed later with
    /// every drive unplugged.
    pub fn store_thumbnail(
        &self,
        face_id: &str,
        image_bytes: &[u8],
        width: u32,
        height: u32,
        key: &MasterKey,
    ) -> Result<()> {
        let sealed = crypto::seal(key, image_bytes)?;
        self.conn.execute(
            "INSERT INTO face_thumbnails
               (face_id, width, height, format, ciphertext, nonce, enc_version, key_version, created_at)
             VALUES (?1,?2,?3,'jpeg',?4,?5,?6,?7,?8)
             ON CONFLICT(face_id) DO UPDATE SET
                width=excluded.width, height=excluded.height,
                ciphertext=excluded.ciphertext, nonce=excluded.nonce,
                enc_version=excluded.enc_version, key_version=excluded.key_version",
            params![
                face_id, width, height, sealed.ciphertext, sealed.nonce,
                sealed.enc_version, sealed.key_version, now_iso8601()
            ],
        )?;
        Ok(())
    }

    /// Faces that have no stored crop yet, with the file they came from.
    ///
    /// Needed because face crops arrived after some archives were already
    /// indexed: their faces exist and are matchable, but there is no picture to
    /// browse. Backfilling re-reads the originals, so the drive must be
    /// connected — the one operation in the product that genuinely requires it.
    pub fn faces_without_thumbnails(&self, limit: usize) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.file_id
               FROM faces f
              WHERE f.is_false_detection = 0
                AND NOT EXISTS (SELECT 1 FROM face_thumbnails t WHERE t.face_id = f.id)
              LIMIT ?1",
        )?;
        let out = stmt
            .query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    /// The stored box for a face, in normalised image coordinates.
    pub fn bbox(&self, face_id: &str) -> Result<Option<(f32, f32, f32, f32)>> {
        let row = self.conn.query_row(
            "SELECT bbox_x, bbox_y, bbox_w, bbox_h FROM faces WHERE id = ?1",
            [face_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        );
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Decrypted image bytes for one face, with the format they are in.
    ///
    /// The format is read from the row rather than assumed, so crops written
    /// before the switch to JPEG still display correctly.
    pub fn thumbnail(&self, face_id: &str, key: &MasterKey) -> Result<Option<(Vec<u8>, String)>> {
        let row = self.conn.query_row(
            "SELECT ciphertext, nonce, enc_version, key_version, format
               FROM face_thumbnails WHERE face_id = ?1",
            [face_id],
            |r| {
                Ok((
                    Sealed {
                        ciphertext: r.get(0)?,
                        nonce: r.get(1)?,
                        enc_version: r.get(2)?,
                        key_version: r.get(3)?,
                    },
                    r.get::<_, String>(4)?,
                ))
            },
        );
        match row {
            Ok((sealed, format)) => Ok(Some((crypto::open(key, &sealed)?, format))),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Faces worth showing in a gallery: best-quality first, one row per face,
    /// with the group it belongs to and any name already attached.
    ///
    /// Ordering by quality matters — the clearest face of a person is the one
    /// you can actually recognise, and it is what should represent the group.
    /// Faces to browse, best first.
    ///
    /// `drive_number` restricts to one drive. Reviewing a single disk at a time
    /// is how the archive is built — one plugged in, indexed, unplugged — and
    /// it is also the only way a face wall stays comprehensible once there are
    /// thousands of faces from twenty disks in it.
    pub fn gallery(&self, limit: usize) -> Result<Vec<GalleryFace>> {
        self.gallery_on_drive(limit, None)
    }

    pub fn gallery_on_drive(
        &self,
        limit: usize,
        drive_number: Option<i64>,
    ) -> Result<Vec<GalleryFace>> {
        let mut sql = String::from(
            "SELECT f.id, f.cluster_id, f.quality, f.file_id,
                    p.display_name, c.status,
                    (SELECT count(*) FROM faces sib WHERE sib.cluster_id = f.cluster_id),
                    d.drive_number, d.friendly_name
               FROM faces f
               JOIN face_thumbnails ft ON ft.face_id = f.id
               JOIN files fi ON fi.id = f.file_id
               JOIN drives d ON d.id = fi.drive_id
               LEFT JOIN face_clusters c ON c.id = f.cluster_id
               LEFT JOIN people p       ON p.id = c.person_id
              WHERE f.is_false_detection = 0 AND f.is_ignored = 0
                AND (c.status IS NULL OR c.status <> 'rejected')
                -- Proposals live in the review queue, not the gallery: a guess
                -- must never look like a name the user gave.
                AND (c.person_id IS NULL OR c.status = 'confirmed')",
        );
        if let Some(n) = drive_number {
            sql.push_str(&format!(" AND d.drive_number = {n}"));
        }
        sql.push_str(" ORDER BY f.quality DESC LIMIT ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        let out = stmt
            .query_map([limit as i64], |r| {
                Ok(GalleryFace {
                    face_id: r.get(0)?,
                    cluster_id: r.get(1)?,
                    quality: r.get(2)?,
                    file_id: r.get(3)?,
                    person_name: r.get(4)?,
                    cluster_status: r.get(5)?,
                    group_size: r.get::<_, Option<i64>>(6)?.unwrap_or(1),
                    drive_number: r.get(7)?,
                    drive_name: r.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    /// Attach a face that has no group of its own to a new named person.
    ///
    /// Browsing the gallery means naming individual faces, not only tidy
    /// clusters, so a face with `cluster_id = NULL` needs somewhere to go.
    pub fn tag_face_with_name(&self, face_id: &str, display_name: &str) -> Result<Person> {
        let existing: Option<String> = self
            .conn
            .query_row("SELECT cluster_id FROM faces WHERE id = ?1", [face_id], |r| r.get(0))
            .optional()?
            .flatten();
        let cluster_id = match existing {
            Some(c) => c,
            None => {
                let c = new_uuid();
                self.conn.execute(
                    "INSERT INTO face_clusters (id, status, algorithm_version, created_at, updated_at)
                     VALUES (?1,'unnamed',?2,?3,?3)",
                    params![c, CLUSTER_ALGO_VERSION, now_iso8601()],
                )?;
                self.conn.execute(
                    "UPDATE faces SET cluster_id=?2 WHERE id=?1",
                    params![face_id, c],
                )?;
                c
            }
        };
        self.tag_cluster_with_name(&cluster_id, display_name)
    }

    /// The folders on disk holding a person's photographs.
    ///
    /// Grouped by folder rather than listed per file, because "where are these?"
    /// is a question about places, not about 300 individual paths. The absolute
    /// path is resolved only when the drive is connected; otherwise the drive
    /// number and relative folder are still shown, which is enough to know what
    /// to plug in and where to look.
    pub fn folders_for_person(&self, person_id: &str) -> Result<Vec<PersonFolder>> {
        use std::collections::BTreeMap;

        let photos = self.photos_of_person(person_id)?;
        let mut grouped: BTreeMap<(i64, String), PersonFolder> = BTreeMap::new();

        for p in photos {
            // The folder is the relative path minus the filename.
            let folder = std::path::Path::new(&p.relative_path)
                .parent()
                .map(|f| f.to_string_lossy().to_string())
                .filter(|f| !f.is_empty())
                .unwrap_or_else(|| ".".to_string());

            let entry = grouped
                .entry((p.drive_number, folder.clone()))
                .or_insert_with(|| PersonFolder {
                    drive_number: p.drive_number,
                    drive_name: p.drive_name.clone(),
                    online: p.online,
                    relative_folder: folder,
                    absolute_path: None,
                    photo_count: 0,
                });
            entry.photo_count += 1;

            // One resolution per folder is enough to know where it is.
            if entry.absolute_path.is_none() {
                if let Some(abs) = crate::search::resolve_original(self.conn, &p.file_id)? {
                    entry.absolute_path = abs
                        .parent()
                        .map(|d| d.to_string_lossy().to_string());
                }
            }
        }

        let mut out: Vec<PersonFolder> = grouped.into_values().collect();
        out.sort_by_key(|f| std::cmp::Reverse(f.photo_count));
        Ok(out)
    }

    /// Every photograph containing a named person, newest drive first.
    pub fn photos_of_person(&self, person_id: &str) -> Result<Vec<PersonPhoto>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT fl.id, fl.filename, fl.relative_path, d.drive_number,
                    d.friendly_name, d.status
               FROM faces f
               JOIN face_clusters c ON c.id = f.cluster_id
               JOIN files fl        ON fl.id = f.file_id
               JOIN drives d        ON d.id = fl.drive_id
              WHERE c.person_id = ?1 AND f.is_false_detection = 0
                AND fl.status = 'complete'
              ORDER BY d.drive_number, fl.relative_path",
        )?;
        let out = stmt
            .query_map([person_id], |r| {
                let status: String = r.get(5)?;
                Ok(PersonPhoto {
                    file_id: r.get(0)?,
                    filename: r.get(1)?,
                    relative_path: r.get(2)?,
                    drive_number: r.get(3)?,
                    drive_name: r.get(4)?,
                    online: status == "online",
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
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
    /// The best match for `embedding` among faces the user has already named,
    /// if any is close enough to be worth suggesting.
    ///
    /// This is what makes "recognise this person on the next scan" work: every
    /// new face is compared against the embeddings of faces already attached to
    /// a named person. It returns a *suggestion* — never a decision. Naming a
    /// person is the user's act (D-007), so a match is recorded for review and
    /// the face is left unconfirmed until a human says otherwise.
    ///
    /// Only faces the user actually confirmed are used as exemplars, so one
    /// mistaken auto-match cannot compound into a drifting cluster.
    pub fn suggest_person(
        &self,
        embedding: &[f32],
        model_id: &str,
        model_version: &str,
        key: &MasterKey,
        threshold: f32,
    ) -> Result<Option<PersonSuggestion>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.display_name, fe.ciphertext, fe.nonce, fe.enc_version, fe.key_version
               FROM face_embeddings fe
               JOIN faces f            ON f.id = fe.face_id
               JOIN face_clusters c    ON c.id = f.cluster_id
               JOIN people p           ON p.id = c.person_id
              WHERE fe.model_id = ?1 AND fe.model_version = ?2
                AND c.status = 'confirmed'
                AND f.is_false_detection = 0 AND f.is_ignored = 0",
        )?;
        let rows = stmt.query_map(params![model_id, model_version], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;

        let mut best: Option<PersonSuggestion> = None;
        for row in rows {
            let (person_id, display_name, ciphertext, nonce, enc_version, key_version) = row?;
            let sealed = Sealed { ciphertext, nonce, enc_version, key_version };
            // A single undecryptable exemplar must not abort recognition.
            let Ok(known) = crypto::open_vector(key, &sealed) else { continue };
            let score = crate::util::cosine_similarity(embedding, &known);
            if score >= threshold && best.as_ref().is_none_or(|b| score > b.score) {
                best = Some(PersonSuggestion { person_id, display_name, score });
            }
        }
        Ok(best)
    }

    /// Attach a face to a named person's cluster as an unconfirmed suggestion.
    ///
    /// The cluster keeps `status = 'unnamed'` deliberately: the person is
    /// proposed, not decided, and only confirmed faces are ever used as
    /// exemplars for future matching.
    pub fn suggest_face_is_person(&self, face_id: &str, person_id: &str, score: f32) -> Result<()> {
        let cluster_id = new_uuid();
        self.conn.execute(
            "INSERT INTO face_clusters
               (id, status, person_id, suggestion_score, algorithm_version, created_at, updated_at)
             VALUES (?1, 'unnamed', ?2, ?3, ?4, ?5, ?5)",
            params![cluster_id, person_id, score, CLUSTER_ALGO_VERSION, now_iso8601()],
        )?;
        self.conn.execute(
            "UPDATE faces SET cluster_id=?2 WHERE id=?1",
            params![face_id, cluster_id],
        )?;
        Ok(())
    }

    /// Propose a just-named person across every face not yet claimed.
    ///
    /// Naming someone should immediately answer "who else is this?" — otherwise
    /// the user names one group of 53 and is left to find the other twenty
    /// groups of the same person by eye.
    ///
    /// Each match attaches the face's group to the person with status left at
    /// `unnamed`: proposed, not decided. Confirming is still a human act
    /// (D-007), and only confirmed faces are ever used as exemplars, so a wrong
    /// proposal cannot compound.
    ///
    /// Returns how many faces were proposed.
    pub fn suggest_for_person(
        &self,
        person_id: &str,
        model_id: &str,
        model_version: &str,
        key: &MasterKey,
        threshold: f32,
    ) -> Result<usize> {
        // The person's confirmed faces are the yardstick.
        let mut exemplars: Vec<Vec<f32>> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT fe.ciphertext, fe.nonce, fe.enc_version, fe.key_version
                   FROM face_embeddings fe
                   JOIN faces f         ON f.id = fe.face_id
                   JOIN face_clusters c ON c.id = f.cluster_id
                  WHERE c.person_id = ?1 AND c.status = 'confirmed'
                    AND fe.model_id = ?2 AND fe.model_version = ?3
                    AND f.is_false_detection = 0",
            )?;
            let rows = stmt.query_map(params![person_id, model_id, model_version], |r| {
                Ok(Sealed {
                    ciphertext: r.get(0)?,
                    nonce: r.get(1)?,
                    enc_version: r.get(2)?,
                    key_version: r.get(3)?,
                })
            })?;
            for row in rows {
                if let Ok(v) = crypto::open_vector(key, &row?) {
                    exemplars.push(v);
                }
            }
        }
        if exemplars.is_empty() {
            return Ok(0);
        }

        // Candidates: faces belonging to nobody at all.
        let mut candidates: Vec<(String, Option<String>, Vec<f32>)> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT f.id, f.cluster_id, fe.ciphertext, fe.nonce, fe.enc_version, fe.key_version
                   FROM faces f
                   JOIN face_embeddings fe ON fe.face_id = f.id
                   LEFT JOIN face_clusters c ON c.id = f.cluster_id
                  WHERE fe.model_id = ?1 AND fe.model_version = ?2
                    AND f.is_false_detection = 0 AND f.is_ignored = 0
                    AND (c.person_id IS NULL)
                    AND (c.status IS NULL OR c.status <> 'rejected')",
            )?;
            let rows = stmt.query_map(params![model_id, model_version], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    Sealed {
                        ciphertext: r.get(2)?,
                        nonce: r.get(3)?,
                        enc_version: r.get(4)?,
                        key_version: r.get(5)?,
                    },
                ))
            })?;
            for row in rows {
                let (face_id, cluster_id, sealed) = row?;
                if let Ok(v) = crypto::open_vector(key, &sealed) {
                    candidates.push((face_id, cluster_id, v));
                }
            }
        }

        let mut proposed = 0usize;
        let mut touched_clusters: std::collections::BTreeSet<String> = Default::default();
        for (face_id, cluster_id, vector) in candidates {
            // Best match against any confirmed face of this person.
            let best = exemplars
                .iter()
                .map(|e| cosine_similarity(&vector, e))
                .fold(f32::MIN, f32::max);
            if best < threshold {
                continue;
            }
            match cluster_id {
                // Propose the whole group at once — its members are already
                // believed to be the same person.
                Some(c) => {
                    if touched_clusters.insert(c.clone()) {
                        self.conn.execute(
                            "UPDATE face_clusters
                                SET person_id = ?2,
                                    suggestion_score = ?3,
                                    updated_at = ?4
                              WHERE id = ?1 AND status <> 'confirmed'",
                            params![c, person_id, best, now_iso8601()],
                        )?;
                    }
                    proposed += 1;
                }
                None => {
                    self.suggest_face_is_person(&face_id, person_id, best)?;
                    proposed += 1;
                }
            }
        }
        Ok(proposed)
    }

    /// Faces proposed as a person, most confident first.
    ///
    /// Leading with the strongest matches means the obvious yeses go quickly and
    /// the user can stop the moment the guesses start looking doubtful, rather
    /// than grinding through a queue sorted by nothing.
    pub fn pending_suggestions(&self, person_id: &str, limit: usize) -> Result<Vec<SuggestedFace>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, c.id, c.suggestion_score,
                    (SELECT count(*) FROM faces sib WHERE sib.cluster_id = c.id)
               FROM faces f
               JOIN face_clusters c ON c.id = f.cluster_id
               JOIN face_thumbnails t ON t.face_id = f.id
              WHERE c.person_id = ?1 AND c.status <> 'confirmed'
                AND f.is_false_detection = 0 AND f.is_ignored = 0
              GROUP BY c.id
              ORDER BY c.suggestion_score DESC
              LIMIT ?2",
        )?;
        let out = stmt
            .query_map(params![person_id, limit as i64], |r| {
                Ok(SuggestedFace {
                    face_id: r.get(0)?,
                    cluster_id: r.get(1)?,
                    score: r.get::<_, Option<f32>>(2)?.unwrap_or(0.0),
                    group_size: r.get::<_, Option<i64>>(3)?.unwrap_or(1),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    /// Accept every outstanding proposal for a person.
    pub fn confirm_suggestions(&self, person_id: &str) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE face_clusters SET status='confirmed', label=NULL, updated_at=?2
              WHERE person_id=?1 AND status <> 'confirmed'",
            params![person_id, now_iso8601()],
        )?;
        Ok(n)
    }

    /// Reject every outstanding proposal for a person, freeing those faces.
    pub fn reject_suggestions(&self, person_id: &str) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE face_clusters SET person_id=NULL, label=NULL, updated_at=?2
              WHERE person_id=?1 AND status <> 'confirmed'",
            params![person_id, now_iso8601()],
        )?;
        Ok(n)
    }

    /// Reject one proposed group without touching the person's other proposals.
    pub fn reject_cluster_suggestion(&self, cluster_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE face_clusters SET person_id=NULL, label=NULL, updated_at=?2
              WHERE id=?1 AND status <> 'confirmed'",
            params![cluster_id, now_iso8601()],
        )?;
        Ok(())
    }

    /// Confirm one proposed group.
    pub fn confirm_cluster_suggestion(&self, cluster_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE face_clusters SET status='confirmed', label=NULL, updated_at=?2
              WHERE id=?1",
            params![cluster_id, now_iso8601()],
        )?;
        Ok(())
    }

    /// Remove a person created by mistake.
    ///
    /// The faces are kept — they are still faces, just no longer claimed by
    /// anyone — and their groups return to unnamed so they reappear for review.
    /// Deleting a typo must never delete photographs or detections.
    pub fn remove_person(&self, person_id: &str) -> Result<()> {
        self.delete_person_face_data(person_id)
    }

    /// Correct a person's name, merging into an existing person if the new name
    /// is already taken.
    ///
    /// Merging on collision is the behaviour that matches the mistake being
    /// fixed: "Kent canon" and "Kent Canovan" are one person typed twice, and
    /// renaming one to match the other should join them rather than fail.
    pub fn rename_person(&self, person_id: &str, new_name: &str) -> Result<Person> {
        let name = new_name.trim();
        if name.is_empty() {
            return Err(Error::InvalidArgs("a person needs a name".into()));
        }
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM people WHERE display_name = ?1 COLLATE NOCASE AND id <> ?2",
                params![name, person_id],
                |r| r.get(0),
            )
            .optional()?;

        match existing {
            // Name already belongs to someone else: move this person's groups
            // across and drop the duplicate record.
            Some(target) => {
                let tx = self.conn.unchecked_transaction()?;
                tx.execute(
                    "UPDATE face_clusters SET person_id=?2 WHERE person_id=?1",
                    params![person_id, target],
                )?;
                tx.execute("DELETE FROM people WHERE id=?1", [person_id])?;
                tx.commit()?;
                self.get_person(&target)?
                    .ok_or_else(|| Error::Other("person vanished".into()))
            }
            None => {
                self.conn.execute(
                    "UPDATE people SET display_name=?2, updated_at=?3 WHERE id=?1",
                    params![person_id, name, now_iso8601()],
                )?;
                self.get_person(person_id)?
                    .ok_or_else(|| Error::InvalidArgs(format!("no person with id {person_id}")))
            }
        }
    }

    /// Detach one face from whatever person it was attached to.
    ///
    /// For the case where a group is right about being a group but wrong about
    /// one member.
    pub fn untag_face(&self, face_id: &str) -> Result<()> {
        self.conn
            .execute("UPDATE faces SET cluster_id = NULL WHERE id = ?1", [face_id])?;
        Ok(())
    }

    /// Every person the user has named, with how many faces are attached.
    pub fn people(&self) -> Result<Vec<NamedPerson>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.display_name, p.relationship,
                    (SELECT count(*) FROM faces f
                       JOIN face_clusters c ON c.id = f.cluster_id
                      WHERE c.person_id = p.id AND c.status = 'confirmed') AS confirmed,
                    (SELECT count(*) FROM faces f
                       JOIN face_clusters c ON c.id = f.cluster_id
                      WHERE c.person_id = p.id AND c.status <> 'confirmed') AS suggested
               FROM people p ORDER BY p.display_name",
        )?;
        let out = stmt
            .query_map([], |r| {
                Ok(NamedPerson {
                    id: r.get(0)?,
                    display_name: r.get(1)?,
                    relationship: r.get(2)?,
                    confirmed_faces: r.get(3)?,
                    suggested_faces: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    /// Name a cluster in one step: find or create the person, then confirm.
    ///
    /// Confirming is what promotes the cluster's faces to exemplars, so from
    /// this point the person is recognised on future scans.
    pub fn tag_cluster_with_name(&self, cluster_id: &str, display_name: &str) -> Result<Person> {
        let name = display_name.trim();
        if name.is_empty() {
            return Err(Error::InvalidArgs("a person needs a name".into()));
        }
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM people WHERE display_name = ?1 COLLATE NOCASE",
                [name],
                |r| r.get(0),
            )
            .ok();
        let person = match existing {
            Some(id) => self
                .get_person(&id)?
                .ok_or_else(|| Error::Other("person vanished".into()))?,
            None => self.create_person(name, None)?,
        };
        self.name_cluster(cluster_id, &person.id)?;
        Ok(person)
    }

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

#[cfg(test)]
mod recognition_tests {
    use super::*;
    use crate::db::{open_in_memory, SchemaKind};

    /// Seed a file row so faces have something to hang off.
    fn seed_file(conn: &Connection, file_id: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO drives(id, drive_number, status, first_seen_at)
             VALUES ('d1', 1, 'online', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO roots(id, drive_id, relative_root, created_at)
             VALUES ('r1','d1','','2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files(id, drive_id, root_id, relative_path, filename, size_bytes,
                               source_mtime_ns, status, created_at, updated_at)
             VALUES (?1,'d1','r1',?1,?1,1,1,'complete','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            [file_id],
        )
        .unwrap();
    }

    /// A vector pointing mostly along `axis`, with a little noise, so two faces
    /// of the "same person" are close but not identical.
    fn face_vector(axis: usize, jitter: f32) -> Vec<f32> {
        let mut v = vec![0.05f32; 24];
        v[axis] = 1.0;
        v[(axis + 1) % 24] = jitter;
        v
    }

    #[test]
    fn a_named_person_is_recognised_in_a_later_photograph() {
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        let key = MasterKey::generate(1);
        let repo = FaceRepo::new(&conn);
        seed_file(&conn, "photo-1");
        seed_file(&conn, "photo-2");

        // A face from the first scan, which the user then names.
        let face1 = repo
            .insert_face("photo-1", (0.1, 0.1, 0.2, 0.2), 0.9, "apple-vision", "1.0.0",
                         &face_vector(3, 0.1), &key)
            .unwrap();
        let cluster = new_uuid();
        conn.execute(
            "INSERT INTO face_clusters(id, status, created_at, updated_at)
             VALUES (?1,'unnamed','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            [&cluster],
        )
        .unwrap();
        conn.execute("UPDATE faces SET cluster_id=?2 WHERE id=?1", params![face1, cluster])
            .unwrap();

        // Before naming, nothing is recognised — there are no exemplars.
        assert!(repo
            .suggest_person(&face_vector(3, 0.12), "apple-vision", "1.0.0", &key, PERSON_MATCH_THRESHOLD)
            .unwrap()
            .is_none());

        // The user tags the cluster.
        let person = repo.tag_cluster_with_name(&cluster, "Aimee").unwrap();
        assert_eq!(person.display_name, "Aimee");

        // A similar face from a later scan is now recognised.
        let hit = repo
            .suggest_person(&face_vector(3, 0.12), "apple-vision", "1.0.0", &key, PERSON_MATCH_THRESHOLD)
            .unwrap()
            .expect("the named person should be recognised");
        assert_eq!(hit.display_name, "Aimee");
        assert!(hit.score >= PERSON_MATCH_THRESHOLD);

        // A different person is not.
        assert!(repo
            .suggest_person(&face_vector(11, 0.1), "apple-vision", "1.0.0", &key, PERSON_MATCH_THRESHOLD)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_suggestion_is_never_treated_as_a_confirmed_naming() {
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        let key = MasterKey::generate(1);
        let repo = FaceRepo::new(&conn);
        seed_file(&conn, "photo-1");
        seed_file(&conn, "photo-2");

        let face1 = repo
            .insert_face("photo-1", (0., 0., 0.2, 0.2), 0.9, "apple-vision", "1.0.0",
                         &face_vector(5, 0.1), &key)
            .unwrap();
        let cluster = new_uuid();
        conn.execute(
            "INSERT INTO face_clusters(id, status, created_at, updated_at)
             VALUES (?1,'unnamed','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            [&cluster],
        )
        .unwrap();
        conn.execute("UPDATE faces SET cluster_id=?2 WHERE id=?1", params![face1, cluster])
            .unwrap();
        let person = repo.tag_cluster_with_name(&cluster, "Kent").unwrap();

        // A later face is suggested as Kent.
        let face2 = repo
            .insert_face("photo-2", (0., 0., 0.2, 0.2), 0.9, "apple-vision", "1.0.0",
                         &face_vector(5, 0.12), &key)
            .unwrap();
        repo.suggest_face_is_person(&face2, &person.id, 0.91).unwrap();

        let people = repo.people().unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].confirmed_faces, 1, "only the user-tagged face is confirmed");
        assert_eq!(people[0].suggested_faces, 1, "the new face is proposed, not decided");

        // And the suggested face must not itself become an exemplar — otherwise
        // one bad match would compound across future scans.
        let before = repo.people().unwrap()[0].confirmed_faces;
        let _ = repo
            .suggest_person(&face_vector(5, 0.13), "apple-vision", "1.0.0", &key, PERSON_MATCH_THRESHOLD)
            .unwrap();
        assert_eq!(repo.people().unwrap()[0].confirmed_faces, before);
    }

    #[test]
    fn naming_two_clusters_the_same_name_reuses_the_person() {
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        let repo = FaceRepo::new(&conn);
        for id in ["c1", "c2"] {
            conn.execute(
                "INSERT INTO face_clusters(id, status, created_at, updated_at)
                 VALUES (?1,'unnamed','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
                [id],
            )
            .unwrap();
        }
        let a = repo.tag_cluster_with_name("c1", "Aimee").unwrap();
        // Different capitalisation and spacing must not create a second person.
        let b = repo.tag_cluster_with_name("c2", "  aimee ").unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(repo.people().unwrap().len(), 1);
    }

    #[test]
    fn a_person_needs_an_actual_name() {
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        let repo = FaceRepo::new(&conn);
        conn.execute(
            "INSERT INTO face_clusters(id, status, created_at, updated_at)
             VALUES ('c1','unnamed','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        assert!(repo.tag_cluster_with_name("c1", "   ").is_err());
    }
}
