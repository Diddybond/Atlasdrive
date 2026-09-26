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

/// Recorded on groups made by [`FaceRepo::group_ungrouped`], so they can be
/// told apart from a full rebuild's.
pub const UNGROUPED_ALGO_VERSION: &str = "ungrouped-greedy-0.1.0";
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

/// Model id of the Apple Vision face engine.
///
/// Spelled out here rather than read from `crate::ai::vision`, which only
/// exists on macOS. A model id is a stored data contract: it is written into
/// every `face_embeddings` row, so any platform must be able to read a
/// catalogue back and know which engine produced it, even where that engine
/// cannot itself run. Referring to the gated module made the whole crate
/// macOS-only and stopped it building — and therefore being tested — anywhere
/// else.
pub const VISION_MODEL_ID: &str = "apple-vision";

// The two declarations must never drift apart. On macOS, where both exist,
// this fails the build the moment they do.
#[cfg(target_os = "macos")]
const _: () = {
    let here = VISION_MODEL_ID.as_bytes();
    let engine = crate::ai::vision::MODEL_ID.as_bytes();
    assert!(here.len() == engine.len(), "vision model id out of step with the engine");
    let mut i = 0;
    while i < here.len() {
        assert!(here[i] == engine[i], "vision model id out of step with the engine");
        i += 1;
    }
};

/// Grouping threshold for the ArcFace identity model (D-102).
///
/// ArcFace scores are cosine similarities in a space trained to separate
/// people: on a test group photograph six different people scored −0.04 to
/// 0.21 against each other, and the same face, darkened and shrunk, 0.99.
/// Same-person pairs across different photographs typically land 0.45–0.85.
/// 0.50 groups conservatively (the value Immich uses as its default distance),
/// so a wrong merge is rarer than a missed one, which is the right way round:
/// a missed one costs a second naming, a wrong one costs trust.
pub const IDENTITY_CLUSTER_THRESHOLD: f32 = 0.50;

/// Suggestion threshold for the identity model. Lower than grouping because a
/// suggestion is only ever a question the owner answers.
pub const IDENTITY_MATCH_THRESHOLD: f32 = 0.42;

/// The clustering threshold to use for a given face-embedding model.
pub fn cluster_threshold_for(model_id: &str) -> f32 {
    match model_id {
        VISION_MODEL_ID => VISION_CLUSTER_THRESHOLD,
        crate::ai::identity::MODEL_ID => IDENTITY_CLUSTER_THRESHOLD,
        _ => DEFAULT_CLUSTER_THRESHOLD,
    }
}

/// The threshold above which a face is proposed as a named person, per model.
pub fn person_match_threshold_for(model_id: &str) -> f32 {
    match model_id {
        crate::ai::identity::MODEL_ID => IDENTITY_MATCH_THRESHOLD,
        _ => PERSON_MATCH_THRESHOLD,
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

/// One confirmed face of a named person, decrypted.
#[derive(Debug, Clone)]
struct Exemplar {
    person_id: String,
    display_name: String,
    model_id: String,
    model_version: String,
    vector: Vec<f32>,
}

/// The faces a new face is compared against: see [`FaceRepo::person_exemplars`].
#[derive(Debug, Clone, Default)]
pub struct PersonExemplars {
    exemplars: Vec<Exemplar>,
}

impl PersonExemplars {
    /// The closest named person at or above `threshold`, comparing only
    /// embeddings from the same model partition.
    pub fn best_match(
        &self,
        embedding: &[f32],
        model_id: &str,
        model_version: &str,
        threshold: f32,
    ) -> Option<PersonSuggestion> {
        let mut best: Option<(&Exemplar, f32)> = None;
        for e in &self.exemplars {
            if e.model_id != model_id || e.model_version != model_version {
                continue;
            }
            let score = cosine_similarity(embedding, &e.vector);
            if score >= threshold && best.is_none_or(|(_, b)| score > b) {
                best = Some((e, score));
            }
        }
        best.map(|(e, score)| PersonSuggestion {
            person_id: e.person_id.clone(),
            display_name: e.display_name.clone(),
            score,
        })
    }
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

/// Greedy grouping of unit vectors: each joins the most similar group at or
/// above `threshold`, or starts its own. Returns the members of each group, as
/// indices into `vectors`, in the order the groups were started.
///
/// Every vector is compared with every group so far, which is quadratic in the
/// worst case — a drive where no two faces match — and measured at 197 seconds
/// for 20,000 faces done one comparison at a time. So vectors are taken in
/// blocks: the comparisons against the groups that existed when a block began
/// are spread across the cores, and only the handful of groups started within
/// the block are checked in order. A group that grows during a block is
/// compared at its centroid from the start of the block, a drift of at most a
/// few members that the greedy method's own order-dependence dwarfs.
fn greedy_groups(vectors: &[&[f32]], threshold: f32) -> Vec<Vec<usize>> {
    const BLOCK: usize = 512;
    let Some(dim) = vectors.first().map(|v| v.len()) else { return Vec::new() };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let dot = |a: &[f32], b: &[f32]| -> f32 { a.iter().zip(b).map(|(x, y)| x * y).sum() };

    let mut sums: Vec<f32> = Vec::new(); // member sums, flat
    let mut units: Vec<f32> = Vec::new(); // normalised centroids, flat
    let mut members: Vec<Vec<usize>> = Vec::new();

    for (b, block) in vectors.chunks(BLOCK).enumerate() {
        let known = members.len();
        let frozen = &units[..known * dim];
        let best_known: Vec<Option<(usize, f32)>> = if known == 0 {
            vec![None; block.len()]
        } else {
            let per = block.len().div_ceil(threads).max(1);
            std::thread::scope(|s| {
                let handles: Vec<_> = block
                    .chunks(per)
                    .map(|part| {
                        s.spawn(move || {
                            part.iter()
                                .map(|v| {
                                    let mut best: Option<(usize, f32)> = None;
                                    for (g, c) in frozen.chunks_exact(dim).enumerate() {
                                        let cos = dot(c, v);
                                        if cos >= threshold && best.is_none_or(|(_, bc)| cos > bc) {
                                            best = Some((g, cos));
                                        }
                                    }
                                    best
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
            })
        };

        for (j, v) in block.iter().enumerate() {
            let i = b * BLOCK + j;
            let mut best = best_known[j];
            for g in known..members.len() {
                let cos = dot(&units[g * dim..(g + 1) * dim], v);
                if cos >= threshold && best.is_none_or(|(_, bc)| cos > bc) {
                    best = Some((g, cos));
                }
            }
            match best {
                Some((g, _)) => {
                    let sum = &mut sums[g * dim..(g + 1) * dim];
                    sum.iter_mut().zip(v.iter()).for_each(|(a, x)| *a += x);
                    let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::MIN_POSITIVE);
                    let unit = &mut units[g * dim..(g + 1) * dim];
                    unit.iter_mut().zip(sum.iter()).for_each(|(u, x)| *u = x / norm);
                    members[g].push(i);
                }
                None => {
                    sums.extend_from_slice(v);
                    units.extend_from_slice(v);
                    members.push(vec![i]);
                }
            }
        }
    }
    members
}

/// How much stricter merging whole groups is than grouping single faces.
pub const GROUP_MERGE_MARGIN: f32 = 0.03;

/// What [`FaceRepo::merge_lookalike_groups`] did.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MergeReport {
    pub groups_considered: usize,
    pub groups_merged: usize,
    pub faces_moved: usize,
}

/// What [`FaceRepo::group_ungrouped`] did.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupingReport {
    pub faces_considered: usize,
    pub groups_created: usize,
    pub faces_grouped: usize,
    /// Groups joined to another drive's group of the same person
    /// ([`FaceRepo::merge_lookalike_groups`]), when that was run too.
    #[serde(default)]
    pub groups_merged: usize,
}

/// Unnamed faces on one drive; see [`FaceRepo::unnamed_counts`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnnamedOnDrive {
    pub drive_number: i64,
    pub drive_name: Option<String>,
    pub faces: i64,
    /// Tiles to name: groups, plus faces with no group.
    pub groups: i64,
}

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

/// A face inside one photograph, where it is and who it is (when known).
///
/// The box is in normalised image coordinates, top-left origin, the same
/// convention the face crops are cut with, so the interface can draw it over
/// the preview as percentages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoFace {
    pub face_id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub person_id: Option<String>,
    pub person_name: Option<String>,
}

/// A face filed under a named person that does not look like them (D-103).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoubtfulFace {
    pub face_id: String,
    pub file_id: String,
    pub filename: String,
    pub drive_number: i64,
    pub online: bool,
    /// How much this face resembles the person's other faces, 0–1.
    pub likeness: f32,
}

/// The result of checking one person's faces.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersonCheck {
    /// Faces of this person the identity model could compare.
    pub checked: usize,
    /// All of this person's faces, compared or not yet.
    #[serde(default)]
    pub total: usize,
    /// Faces that do not look like the rest, least alike first.
    pub doubtful: Vec<DoubtfulFace>,
}

/// Below this, a face does not look like the person it is filed under.
///
/// Measured as the average of its five best matches among the person's other
/// faces, so a person photographed at many ages is still recognised as long
/// as a handful of photographs resemble each face. Different people score
/// around 0.0–0.2 on the identity model; the same person 0.45 and up.
pub const DOUBT_THRESHOLD: f32 = 0.30;
const DOUBT_NEIGHBOURS: usize = 5;
/// Fewer identity-model faces than this and there is no "rest" to compare with.
const DOUBT_MIN_FACES: usize = 4;

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

    /// Replace a face's embedding with one from another model.
    ///
    /// A face holds one embedding: the best the catalogue has. The identity
    /// upgrade (D-102) swaps the Vision feature print for an ArcFace vector.
    pub fn replace_embedding(
        &self,
        face_id: &str,
        model_id: &str,
        model_version: &str,
        embedding: &[f32],
        key: &MasterKey,
    ) -> Result<()> {
        let sealed = crypto::seal_vector(key, embedding)?;
        self.conn.execute(
            "INSERT INTO face_embeddings
             (face_id, model_id, model_version, dim, ciphertext, nonce, enc_version, key_version, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(face_id) DO UPDATE SET
                model_id=excluded.model_id, model_version=excluded.model_version, dim=excluded.dim,
                ciphertext=excluded.ciphertext, nonce=excluded.nonce,
                enc_version=excluded.enc_version, key_version=excluded.key_version,
                created_at=excluded.created_at",
            params![
                face_id, model_id, model_version, embedding.len() as i64,
                sealed.ciphertext, sealed.nonce, sealed.enc_version, sealed.key_version,
                now_iso8601()
            ],
        )?;
        Ok(())
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
        // One tile per group — its clearest face — and one per face that has
        // no group yet. Listing every face put the same person in five tiles of
        // the first two rows, each to be named separately, while naming any one
        // of a group already names all of it. Biggest groups first: naming
        // those covers the most photographs per answer.
        let on_drive = match drive_number {
            Some(n) => format!(" AND d.drive_number = {n}"),
            None => String::new(),
        };
        let sql = format!(
            "SELECT x.id, x.cluster_id, x.quality, x.file_id, p.display_name, c.status,
                    CASE WHEN x.cluster_id IS NULL THEN 1
                         ELSE (SELECT count(*) FROM faces sib WHERE sib.cluster_id = x.cluster_id) END
                      AS group_size,
                    x.drive_number, x.friendly_name
               FROM (SELECT f.id, f.cluster_id, f.quality, f.file_id, d.drive_number, d.friendly_name,
                            ROW_NUMBER() OVER (PARTITION BY coalesce(f.cluster_id, f.id)
                                               ORDER BY f.quality DESC, f.id) AS rn
                       FROM faces f
                       JOIN face_thumbnails ft ON ft.face_id = f.id
                       JOIN files fi ON fi.id = f.file_id
                       JOIN drives d ON d.id = fi.drive_id
                       LEFT JOIN face_clusters c ON c.id = f.cluster_id
                      WHERE f.is_false_detection = 0 AND f.is_ignored = 0
                        AND (c.status IS NULL OR c.status <> 'rejected')
                        -- Proposals live in the review queue, not the gallery: a
                        -- guess must never look like a name the user gave.
                        AND (c.person_id IS NULL OR c.status = 'confirmed'){on_drive}) x
               LEFT JOIN face_clusters c ON c.id = x.cluster_id
               LEFT JOIN people p       ON p.id = c.person_id
              WHERE x.rn = 1
              ORDER BY group_size DESC, x.quality DESC
              LIMIT ?1"
        );
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

    /// Faces filed under a person that do not look like the rest of them.
    ///
    /// Groups named before the identity model (D-102) were built by Apple
    /// Vision's look-alike matching, which put different people together; a
    /// whole group named at once carried them in. This finds them, so the
    /// owner can say "not them" to each. Faces the owner has already said are
    /// them are not asked about again.
    pub fn doubtful_faces(&self, person_id: &str, key: &MasterKey) -> Result<PersonCheck> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, fl.id, fl.filename, d.drive_number, d.status,
                    fe.ciphertext, fe.nonce, fe.enc_version, fe.key_version
               FROM faces f
               JOIN face_clusters c   ON c.id = f.cluster_id
               JOIN face_embeddings fe ON fe.face_id = f.id
               JOIN files fl          ON fl.id = f.file_id
               JOIN drives d          ON d.id = fl.drive_id
              WHERE c.person_id = ?1 AND c.status = 'confirmed'
                AND fe.model_id = ?2 AND fe.model_version = ?3
                AND f.is_false_detection = 0",
        )?;
        let rows = stmt.query_map(
            params![person_id, crate::ai::identity::MODEL_ID, crate::ai::identity::MODEL_VERSION],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    Sealed { ciphertext: r.get(5)?, nonce: r.get(6)?, enc_version: r.get(7)?, key_version: r.get(8)? },
                ))
            },
        )?;
        let mut faces = Vec::new();
        for row in rows {
            let (face_id, file_id, filename, drive_number, status, sealed) = row?;
            if let Ok(v) = crypto::open_vector(key, &sealed) {
                faces.push((face_id, file_id, filename, drive_number, status == "online", v));
            }
        }
        let checked = faces.len();
        let total: usize = self.conn.query_row(
            "SELECT count(*) FROM faces f JOIN face_clusters c ON c.id = f.cluster_id
              WHERE c.person_id = ?1 AND c.status = 'confirmed' AND f.is_false_detection = 0",
            [person_id],
            |r| r.get::<_, i64>(0),
        )? as usize;
        if checked < DOUBT_MIN_FACES {
            return Ok(PersonCheck { checked, total, doubtful: Vec::new() });
        }
        let kept: std::collections::HashSet<String> = self
            .conn
            .prepare(
                "SELECT face_id FROM face_person_links
                  WHERE person_id = ?1 AND source = 'kept' AND face_id IS NOT NULL",
            )?
            .query_map([person_id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;

        let k = DOUBT_NEIGHBOURS.min(checked - 1);
        let mut doubtful = Vec::new();
        for (i, (face_id, file_id, filename, drive_number, online, v)) in faces.iter().enumerate() {
            if kept.contains(face_id) {
                continue;
            }
            let mut sims: Vec<f32> = faces
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, other)| cosine_similarity(v, &other.5))
                .collect();
            sims.sort_by(|a, b| b.total_cmp(a));
            let likeness = sims.iter().take(k).sum::<f32>() / k as f32;
            if likeness < DOUBT_THRESHOLD {
                doubtful.push(DoubtfulFace {
                    face_id: face_id.clone(),
                    file_id: file_id.clone(),
                    filename: filename.clone(),
                    drive_number: *drive_number,
                    online: *online,
                    likeness: likeness.max(0.0),
                });
            }
        }
        doubtful.sort_by(|a, b| a.likeness.total_cmp(&b.likeness));
        Ok(PersonCheck { checked, total, doubtful })
    }

    /// The owner looked and said: yes, this face is them. Not asked again.
    pub fn keep_face(&self, face_id: &str, person_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO face_person_links (id, face_id, person_id, source, confidence, is_confirmed, created_at)
             VALUES (?1, ?2, ?3, 'kept', 1.0, 1, ?4)",
            params![new_uuid(), face_id, person_id, now_iso8601()],
        )?;
        Ok(())
    }

    /// The owner looked and said: this face is not them. It leaves the
    /// person and becomes an unnamed face again, free to be named or grouped.
    pub fn not_this_person(&self, face_id: &str) -> Result<()> {
        self.split_face(face_id)?;
        Ok(())
    }

    /// Every face found in one photograph, left to right.
    pub fn faces_in_file(&self, file_id: &str) -> Result<Vec<PhotoFace>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.bbox_x, f.bbox_y, f.bbox_w, f.bbox_h, p.id, p.display_name
               FROM faces f
               LEFT JOIN face_clusters c ON c.id = f.cluster_id AND c.status = 'confirmed'
               LEFT JOIN people p        ON p.id = c.person_id
              WHERE f.file_id = ?1 AND f.is_false_detection = 0
              ORDER BY f.bbox_x",
        )?;
        let out = stmt
            .query_map([file_id], |r| {
                Ok(PhotoFace {
                    face_id: r.get(0)?,
                    x: r.get(1)?,
                    y: r.get(2)?,
                    w: r.get(3)?,
                    h: r.get(4)?,
                    person_id: r.get(5)?,
                    person_name: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    /// Name exactly this face, and no other.
    ///
    /// Naming from inside a photograph is a statement about one face. If its
    /// group holds other faces, those are not assumed to be the same person —
    /// the face moves to a group of its own first. The rest are offered
    /// afterwards as suggestions, to accept or refuse. The People screen is
    /// where a whole group is named at once.
    pub fn name_one_face(&self, face_id: &str, display_name: &str) -> Result<Person> {
        let name = display_name.trim();
        if name.is_empty() {
            return Err(Error::InvalidArgs("a person needs a name".into()));
        }
        let row: Option<(Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT f.cluster_id, p.display_name
                   FROM faces f
                   LEFT JOIN face_clusters c ON c.id = f.cluster_id AND c.status = 'confirmed'
                   LEFT JOIN people p        ON p.id = c.person_id
                  WHERE f.id = ?1",
                [face_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((cluster, current)) = row else {
            return Err(Error::InvalidArgs("no such face".into()));
        };
        // Already this person: nothing to change.
        if let (Some(c), Some(cur)) = (&cluster, &current) {
            if cur.eq_ignore_ascii_case(name) {
                let person_id: String = self.conn.query_row(
                    "SELECT person_id FROM face_clusters WHERE id = ?1",
                    [c],
                    |r| r.get(0),
                )?;
                return self
                    .get_person(&person_id)?
                    .ok_or_else(|| Error::Other("person vanished".into()));
            }
        }
        let alone = match &cluster {
            None => true,
            Some(c) => {
                self.conn.query_row(
                    "SELECT count(*) FROM faces WHERE cluster_id = ?1",
                    [c],
                    |r| r.get::<_, i64>(0),
                )? <= 1
            }
        };
        let target = if alone {
            match cluster {
                Some(c) => c,
                None => self.split_face(face_id)?,
            }
        } else {
            self.split_face(face_id)?
        };
        self.tag_cluster_with_name(&target, name)
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

    /// Put faces that belong to no group into groups of look-alikes.
    ///
    /// A scan stores each face on its own; grouping happened only when someone
    /// ran `index --rebuild-faces` by hand, so after a scan the People screen
    /// offered every face as a stranger to be named one at a time. This runs at
    /// the end of every scan for that drive, and on demand for an archive
    /// indexed before it existed (D-089).
    ///
    /// It only ever *adds*: faces already in a group, named or not, are left
    /// exactly where they are, and a face with no look-alike stays ungrouped
    /// rather than becoming a group of one. Clearest faces go first, so each
    /// group is seeded by a good example. Each model partition is grouped on
    /// its own, at its own threshold ([`cluster_threshold_for`]).
    ///
    /// Greedy against running centroids, like [`Self::rebuild_clusters`], but
    /// over one drive's ungrouped faces rather than the whole archive, so it
    /// stays proportionate on a catalogue with a hundred thousand faces.
    pub fn group_ungrouped(&self, drive_id: Option<&str>, key: &MasterKey) -> Result<GroupingReport> {
        let (filter, param) = match drive_id {
            Some(id) => ("AND fi.drive_id = ?1", Some(id.to_string())),
            None => ("", None),
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT f.id, fe.model_id, fe.model_version,
                    fe.ciphertext, fe.nonce, fe.enc_version, fe.key_version
               FROM faces f
               JOIN face_embeddings fe ON fe.face_id = f.id
               JOIN files fi ON fi.id = f.file_id
              WHERE f.cluster_id IS NULL
                AND f.is_false_detection = 0 AND f.is_ignored = 0 {filter}
              ORDER BY fe.model_id, fe.model_version, f.quality DESC, f.id"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(param.iter()), |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
                Sealed {
                    ciphertext: r.get::<_, Vec<u8>>(3)?,
                    nonce: r.get::<_, Vec<u8>>(4)?,
                    enc_version: r.get::<_, i64>(5)?,
                    key_version: r.get::<_, i64>(6)?,
                },
            ))
        })?;

        let mut report = GroupingReport::default();
        // (model, [(face_id, unit vector)]) in query order.
        type Partition = ((String, String), Vec<(String, Vec<f32>)>);
        let mut partitions: Vec<Partition> = Vec::new();
        for row in rows {
            let (face_id, model, sealed) = row?;
            let Ok(mut v) = crypto::open_vector(key, &sealed) else { continue };
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if !norm.is_finite() || norm == 0.0 {
                continue;
            }
            v.iter_mut().for_each(|x| *x /= norm);
            report.faces_considered += 1;
            match partitions.last_mut() {
                Some((m, faces)) if *m == model => faces.push((face_id, v)),
                _ => partitions.push((model, vec![(face_id, v)])),
            }
        }

        let tx = crate::db::write_tx(self.conn)?;
        let now = now_iso8601();
        for ((model_id, _), faces) in &partitions {
            let dim = faces[0].1.len();
            let usable: Vec<usize> = (0..faces.len()).filter(|&i| faces[i].1.len() == dim).collect();
            let vectors: Vec<&[f32]> = usable.iter().map(|&i| faces[i].1.as_slice()).collect();
            let members: Vec<Vec<usize>> = greedy_groups(&vectors, cluster_threshold_for(model_id))
                .into_iter()
                .map(|g| g.into_iter().map(|j| usable[j]).collect())
                .collect();
            for group in members.iter().filter(|m| m.len() >= 2) {
                let cid = new_uuid();
                tx.execute(
                    "INSERT INTO face_clusters (id, status, algorithm_version, created_at, updated_at)
                     VALUES (?1,'unnamed',?2,?3,?3)",
                    params![cid, UNGROUPED_ALGO_VERSION, now],
                )?;
                for &i in group {
                    tx.execute(
                        "UPDATE faces SET cluster_id = ?2 WHERE id = ?1 AND cluster_id IS NULL",
                        params![faces[i].0, cid],
                    )?;
                }
                report.groups_created += 1;
                report.faces_grouped += group.len();
            }
        }
        tx.commit()?;
        Ok(report)
    }

    /// Merge unnamed groups that are the same person, across every drive.
    ///
    /// [`Self::group_ungrouped`] works one drive at a time, so the same guest at
    /// weddings on Drive 3 and Drive 8 ends up as two groups. This compares
    /// the groups themselves — the average of each group's faces — and merges
    /// look-alikes into the larger group. An average is steadier than any one
    /// face, which also pulls different people's averages closer together, so
    /// the bar is set higher than for single faces
    /// ([`GROUP_MERGE_MARGIN`] above the model's threshold).
    ///
    /// Only groups nobody has named, and nobody has been suggested for, are
    /// touched. A named group never moves and never absorbs anything here;
    /// naming stays a human act (D-007).
    pub fn merge_lookalike_groups(&self, key: &MasterKey) -> Result<MergeReport> {
        let mut stmt = self.conn.prepare(
            "SELECT f.cluster_id, fe.model_id, fe.model_version,
                    fe.ciphertext, fe.nonce, fe.enc_version, fe.key_version
               FROM faces f
               JOIN face_clusters c ON c.id = f.cluster_id
               JOIN face_embeddings fe ON fe.face_id = f.id
              WHERE c.status = 'unnamed' AND c.person_id IS NULL
                AND f.is_false_detection = 0 AND f.is_ignored = 0
              ORDER BY fe.model_id, fe.model_version, f.cluster_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
                Sealed {
                    ciphertext: r.get::<_, Vec<u8>>(3)?,
                    nonce: r.get::<_, Vec<u8>>(4)?,
                    enc_version: r.get::<_, i64>(5)?,
                    key_version: r.get::<_, i64>(6)?,
                },
            ))
        })?;
        // model -> cluster -> (sum of unit vectors, faces)
        type Sums = std::collections::BTreeMap<String, (Vec<f32>, usize)>;
        let mut by_model: std::collections::BTreeMap<(String, String), Sums> = Default::default();
        for row in rows {
            let (cluster, model, sealed) = row?;
            let Ok(v) = crypto::open_vector(key, &sealed) else { continue };
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if !norm.is_finite() || norm == 0.0 {
                continue;
            }
            let entry = by_model
                .entry(model)
                .or_default()
                .entry(cluster)
                .or_insert_with(|| (vec![0.0; v.len()], 0));
            if entry.0.len() != v.len() {
                continue;
            }
            entry.0.iter_mut().zip(&v).for_each(|(a, x)| *a += x / norm);
            entry.1 += 1;
        }

        let mut report = MergeReport::default();
        let tx = crate::db::write_tx(self.conn)?;
        let now = now_iso8601();
        for ((model_id, _), clusters) in by_model {
            // Biggest groups first, so each merge lands in the best-established one.
            let mut groups: Vec<(String, Vec<f32>, usize)> = clusters
                .into_iter()
                .map(|(id, (sum, n))| {
                    let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::MIN_POSITIVE);
                    (id, sum.into_iter().map(|x| x / norm).collect(), n)
                })
                .collect();
            groups.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
            report.groups_considered += groups.len();
            let dim = groups.first().map_or(0, |g| g.1.len());
            let vectors: Vec<&[f32]> = groups.iter().map(|g| g.1.as_slice()).filter(|v| v.len() == dim).collect();
            let threshold = (cluster_threshold_for(&model_id) + GROUP_MERGE_MARGIN).min(0.99);
            for merged in greedy_groups(&vectors, threshold).into_iter().filter(|m| m.len() >= 2) {
                let into = &groups[merged[0]].0;
                for &i in &merged[1..] {
                    let from = &groups[i].0;
                    let moved = tx.execute(
                        "UPDATE faces SET cluster_id = ?2 WHERE cluster_id = ?1",
                        params![from, into],
                    )?;
                    tx.execute(
                        "UPDATE face_clusters SET status = 'merged', updated_at = ?2 WHERE id = ?1",
                        params![from, now],
                    )?;
                    report.groups_merged += 1;
                    report.faces_moved += moved;
                }
            }
        }
        tx.commit()?;
        Ok(report)
    }

    /// Faces nobody has named, per drive: how many faces, and how many tiles
    /// that is once grouped. Counted in the catalogue, not from a sample —
    /// the People screen used to count within its first thousand faces.
    pub fn unnamed_counts(&self) -> Result<Vec<UnnamedOnDrive>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.drive_number, d.friendly_name, count(*),
                    count(DISTINCT coalesce(f.cluster_id, f.id))
               FROM faces f
               JOIN face_thumbnails ft ON ft.face_id = f.id
               JOIN files fi ON fi.id = f.file_id
               JOIN drives d ON d.id = fi.drive_id
               LEFT JOIN face_clusters c ON c.id = f.cluster_id
              WHERE f.is_false_detection = 0 AND f.is_ignored = 0
                AND (c.status IS NULL OR c.status <> 'rejected')
                AND c.person_id IS NULL
              GROUP BY d.drive_number, d.friendly_name
              ORDER BY d.drive_number",
        )?;
        let out = stmt
            .query_map([], |r| {
                Ok(UnnamedOnDrive {
                    drive_number: r.get(0)?,
                    drive_name: r.get(1)?,
                    faces: r.get(2)?,
                    groups: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
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

        let tx = crate::db::write_tx(self.conn)?;
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
        Ok(self.person_exemplars(key)?.best_match(embedding, model_id, model_version, threshold))
    }

    /// Every confirmed face of every named person, decrypted once.
    ///
    /// [`Self::suggest_person`] used to decrypt all of them for every new face,
    /// so the cost of recognising one face grew with everyone the owner had
    /// ever named — on the single thread that writes the catalogue, which
    /// parallel analysis (D-087) made the one place a scan waits in line. The
    /// pipeline loads these once per batch and asks them for each face.
    pub fn person_exemplars(&self, key: &MasterKey) -> Result<PersonExemplars> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.display_name, fe.model_id, fe.model_version,
                    fe.ciphertext, fe.nonce, fe.enc_version, fe.key_version
               FROM face_embeddings fe
               JOIN faces f            ON f.id = fe.face_id
               JOIN face_clusters c    ON c.id = f.cluster_id
               JOIN people p           ON p.id = c.person_id
              WHERE c.status = 'confirmed'
                AND f.is_false_detection = 0 AND f.is_ignored = 0",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                Sealed {
                    ciphertext: r.get::<_, Vec<u8>>(4)?,
                    nonce: r.get::<_, Vec<u8>>(5)?,
                    enc_version: r.get::<_, i64>(6)?,
                    key_version: r.get::<_, i64>(7)?,
                },
            ))
        })?;
        let mut exemplars = Vec::new();
        for row in rows {
            let (person_id, display_name, model_id, model_version, sealed) = row?;
            // A single undecryptable exemplar must not abort recognition.
            let Ok(vector) = crypto::open_vector(key, &sealed) else { continue };
            exemplars.push(Exemplar { person_id, display_name, model_id, model_version, vector });
        }
        Ok(PersonExemplars { exemplars })
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

    /// How a person relates to the owner: family, or anything else they type.
    ///
    /// `people.relationship` has existed since the first schema and has never
    /// been settable. It earns its place now because a wedding photographer's
    /// archive is mostly clients and guests, and the handful of people who are
    /// *family* are the ones searched for again in ten years' time. Being able
    /// to say "just my family" is the difference between a working archive and
    /// a very large folder.
    ///
    /// Free text rather than an enum: "family" is what is asked for today, and
    /// somebody will reasonably want "wedding party" or "staff" tomorrow.
    /// Stored lower-cased so "Family" and "family" are one group. `None` clears.
    pub fn set_relationship(&self, person_id: &str, relationship: Option<&str>) -> Result<()> {
        let cleaned = relationship
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(|r| r.to_lowercase());
        let affected = self.conn.execute(
            "UPDATE people SET relationship = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![person_id, cleaned, crate::util::now_iso8601()],
        )?;
        if affected == 0 {
            return Err(Error::InvalidArgs(format!("no person {person_id}")));
        }
        Ok(())
    }

    /// Every relationship in use, with how many people carry it.
    pub fn relationships(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT relationship, count(*) FROM people
              WHERE relationship IS NOT NULL AND trim(relationship) <> ''
              GROUP BY relationship ORDER BY count(*) DESC, relationship",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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
                let tx = crate::db::write_tx(self.conn)?;
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
        let tx = crate::db::write_tx(self.conn)?;
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
        let tx = crate::db::write_tx(self.conn)?;
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
        let tx = crate::db::write_tx(self.conn)?;
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
        let tx = crate::db::write_tx(self.conn)?;
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
        Ok(FaceHealth::of(embeddings.iter().map(|(_, v)| v.as_slice())))
    }

    /// [`Self::embedding_health`] for every model partition at once, over the
    /// faces whose `f.file_id` satisfies `condition` (`f` is `faces`; `param`
    /// binds its `?1`, if it has one).
    ///
    /// Each partition is judged on its own: a 768-dimension Vision print and a
    /// 65-dimension heuristic vector are both healthy, and mixing them would
    /// report every one of one kind as a dimension mismatch.
    pub fn embedding_health_where(
        &self,
        condition: &str,
        param: Option<&str>,
        key: &MasterKey,
    ) -> Result<Vec<((String, String), FaceHealth)>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT fe.model_id, fe.model_version, fe.ciphertext, fe.nonce, fe.enc_version, fe.key_version
               FROM face_embeddings fe
               JOIN faces f ON f.id = fe.face_id
              WHERE f.is_false_detection=0 AND f.is_ignored=0 AND {condition}
              ORDER BY fe.model_id, fe.model_version"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(param.iter()), |r| {
            Ok((
                (r.get::<_, String>(0)?, r.get::<_, String>(1)?),
                Sealed {
                    ciphertext: r.get::<_, Vec<u8>>(2)?,
                    nonce: r.get::<_, Vec<u8>>(3)?,
                    enc_version: r.get::<_, i64>(4)?,
                    key_version: r.get::<_, i64>(5)?,
                },
            ))
        })?;
        type Partition = ((String, String), Vec<Vec<f32>>);
        let mut partitions: Vec<Partition> = Vec::new();
        for row in rows {
            let (model, sealed) = row?;
            let v = crypto::open_vector(key, &sealed)?;
            match partitions.last_mut() {
                Some((m, vs)) if *m == model => vs.push(v),
                _ => partitions.push((model, vec![v])),
            }
        }
        Ok(partitions
            .into_iter()
            .map(|(model, vs)| (model, FaceHealth::of(vs.iter().map(|v| v.as_slice()))))
            .collect())
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

impl FaceHealth {
    /// Tally a set of embeddings from one model partition.
    fn of<'v>(embeddings: impl Iterator<Item = &'v [f32]>) -> Self {
        let mut health = FaceHealth::default();
        let mut seen: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for v in embeddings {
            if health.total == 0 {
                health.dim = v.len();
            }
            health.total += 1;
            if v.len() != health.dim {
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
        health
    }
}

#[cfg(test)]
mod relationship_tests {
    use super::*;
    use crate::db::{self, SchemaKind};

    fn person(conn: &rusqlite::Connection, id: &str, name: &str) {
        conn.execute(
            "INSERT INTO people (id, display_name, created_at, updated_at)
             VALUES (?1, ?2, 'now', 'now')",
            rusqlite::params![id, name],
        )
        .unwrap();
    }

    #[test]
    fn marks_people_as_family_and_counts_them() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        person(&conn, "p1", "Millie Myers");
        person(&conn, "p2", "Tyler Myers");
        person(&conn, "p3", "Aimee Kanovan");
        let repo = FaceRepo::new(&conn);

        repo.set_relationship("p1", Some("family")).unwrap();
        repo.set_relationship("p2", Some("Family")).unwrap();
        repo.set_relationship("p3", Some("client")).unwrap();

        // Lower-cased on the way in, so "Family" and "family" are one group
        // rather than two — nobody types a label the same way twice.
        let groups = repo.relationships().unwrap();
        assert_eq!(groups, vec![("family".to_string(), 2), ("client".to_string(), 1)]);
    }

    #[test]
    fn a_relationship_can_be_cleared() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        person(&conn, "p1", "Someone");
        let repo = FaceRepo::new(&conn);

        repo.set_relationship("p1", Some("family")).unwrap();
        assert_eq!(repo.relationships().unwrap().len(), 1);

        repo.set_relationship("p1", None).unwrap();
        assert!(repo.relationships().unwrap().is_empty());
        // Blank input clears rather than storing an empty label.
        repo.set_relationship("p1", Some("family")).unwrap();
        repo.set_relationship("p1", Some("   ")).unwrap();
        assert!(repo.relationships().unwrap().is_empty());
    }

    #[test]
    fn setting_a_relationship_on_nobody_is_an_error() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        assert!(FaceRepo::new(&conn).set_relationship("nope", Some("family")).is_err());
    }

    /// The relationship must come back on the person, or the interface cannot
    /// show who is family without a second query.
    #[test]
    fn the_relationship_is_carried_on_the_person() {
        let conn = db::open_in_memory(SchemaKind::Archive).unwrap();
        person(&conn, "p1", "Millie Myers");
        let repo = FaceRepo::new(&conn);
        repo.set_relationship("p1", Some("family")).unwrap();

        let people = repo.people().unwrap();
        let millie = people.iter().find(|p| p.id == "p1").unwrap();
        assert_eq!(millie.relationship.as_deref(), Some("family"));
    }
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

    /// Naming a face from inside a photograph names that face only; the rest
    /// of its group is left alone, and renaming moves it to the new person.
    #[test]
    fn naming_one_face_leaves_its_group_alone() {
        let (conn, _d, key) = setup();
        add_file(&conn, "f1");
        add_file(&conn, "f2");
        let repo = FaceRepo::new(&conn);
        let a = repo.insert_face("f1", (0.6, 0.1, 0.1, 0.1), 0.9, "m", "1", &[1.0, 0.0], &key).unwrap();
        let b = repo.insert_face("f1", (0.1, 0.1, 0.1, 0.1), 0.9, "m", "1", &[0.0, 1.0], &key).unwrap();
        let c = repo.insert_face("f2", (0.1, 0.1, 0.1, 0.1), 0.9, "m", "1", &[1.0, 0.0], &key).unwrap();
        // a and c share a group.
        let g = repo.split_face(&a).unwrap();
        conn.execute("UPDATE faces SET cluster_id=?1 WHERE id=?2", params![g, c]).unwrap();

        let in_photo = repo.faces_in_file("f1").unwrap();
        assert_eq!(in_photo.len(), 2);
        assert_eq!(in_photo[0].face_id, b, "left to right");
        assert!(in_photo.iter().all(|f| f.person_name.is_none()));

        let aimee = repo.name_one_face(&a, "Aimee").unwrap();
        let named = repo.faces_in_file("f1").unwrap();
        let fa = named.iter().find(|f| f.face_id == a).unwrap();
        assert_eq!(fa.person_name.as_deref(), Some("Aimee"));
        // c, grouped with a, is not assumed to be Aimee.
        assert!(repo.faces_in_file("f2").unwrap()[0].person_name.is_none());

        // Naming again with the same name changes nothing; a face with no
        // group gets one.
        assert_eq!(repo.name_one_face(&a, "aimee").unwrap().id, aimee.id);
        repo.name_one_face(&b, "Kent").unwrap();
        // A correction moves the face to the right person.
        repo.name_one_face(&a, "Daisy").unwrap();
        let after = repo.faces_in_file("f1").unwrap();
        let names: Vec<_> = after.iter().map(|f| f.person_name.clone().unwrap()).collect();
        assert_eq!(names, ["Kent", "Daisy"]);
        assert!(repo.name_one_face(&a, "  ").is_err());
    }

    /// A stranger filed under someone is found; their real faces are not;
    /// answering removes it from the list either way.
    #[test]
    fn checking_a_person_finds_the_stranger_among_them() {
        let (conn, _d, key) = setup();
        let repo = FaceRepo::new(&conn);
        let id = crate::ai::identity::MODEL_ID;
        let mut aimee = Vec::new();
        // Five faces of one person (similar), one stranger (orthogonal).
        for (n, v) in [
            [1.0, 0.1, 0.0], [0.95, 0.2, 0.0], [0.9, 0.0, 0.1], [1.0, 0.0, 0.2], [0.97, 0.1, 0.1],
            [0.0, 0.0, 1.0],
        ]
        .iter()
        .enumerate()
        {
            add_file(&conn, &format!("f{n}"));
            aimee.push(repo.insert_face(&format!("f{n}"), (0.1, 0.1, 0.2, 0.2), 0.9, id, "1", v, &key).unwrap());
        }
        let c = repo.split_face(&aimee[0]).unwrap();
        for f in &aimee {
            conn.execute("UPDATE faces SET cluster_id=?1 WHERE id=?2", params![c, f]).unwrap();
        }
        let person = repo.tag_cluster_with_name(&c, "Aimee").unwrap();

        let check = repo.doubtful_faces(&person.id, &key).unwrap();
        assert_eq!(check.checked, 6);
        let found: Vec<_> = check.doubtful.iter().map(|d| d.face_id.clone()).collect();
        assert_eq!(found, [aimee[5].clone()]);

        // "It's Aimee" — not asked again.
        repo.keep_face(&aimee[5], &person.id).unwrap();
        assert!(repo.doubtful_faces(&person.id, &key).unwrap().doubtful.is_empty());

        // "Not Aimee" — the face leaves her.
        conn.execute("DELETE FROM face_person_links WHERE source='kept'", []).unwrap();
        repo.not_this_person(&aimee[5]).unwrap();
        let after = repo.doubtful_faces(&person.id, &key).unwrap();
        assert_eq!(after.checked, 5);
        assert!(after.doubtful.is_empty());
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

#[cfg(test)]
mod grouping_tests {
    use super::*;
    use crate::db::{open_in_memory, SchemaKind};

    /// Two drives, one root each.
    fn archive() -> (Connection, MasterKey) {
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute_batch(
            "INSERT INTO drives (id, drive_number, status, first_seen_at) VALUES ('d1',1,'online','now'), ('d2',2,'online','now');
             INSERT INTO roots (id, drive_id, relative_root, created_at) VALUES ('r1','d1','','now'), ('r2','d2','','now');",
        )
        .unwrap();
        (conn, MasterKey::generate(1))
    }

    /// A face on `drive` pointing along `axis`, slightly perturbed: faces with
    /// the same axis are the same person.
    fn face(conn: &Connection, key: &MasterKey, drive: &str, axis: usize, jitter: f32, quality: f32) -> String {
        let file = new_uuid();
        let root = if drive == "d1" { "r1" } else { "r2" };
        conn.execute(
            "INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                source_mtime_ns, status, created_at, updated_at)
             VALUES (?1,?2,?3,?1,?1,10,1,'complete','now','now')",
            params![file, drive, root],
        )
        .unwrap();
        let mut v = vec![0.05f32; 24];
        v[axis] = 1.0;
        v[(axis + 1) % 24] = jitter;
        let repo = FaceRepo::new(conn);
        let id = repo
            .insert_face(&file, (0.1, 0.1, 0.2, 0.2), quality, "apple-vision", "1.0.0", &v, key)
            .unwrap();
        repo.store_thumbnail(&id, b"jpeg", 8, 8, key).unwrap();
        id
    }

    fn cluster_of(conn: &Connection, face: &str) -> Option<String> {
        conn.query_row("SELECT cluster_id FROM faces WHERE id=?1", [face], |r| r.get(0)).unwrap()
    }

    /// D-089: look-alikes are grouped, a face with no look-alike is not made
    /// into a group of one, and nothing already grouped is moved.
    #[test]
    fn ungrouped_faces_are_grouped_by_likeness_and_nothing_else_moves() {
        let (conn, key) = archive();
        let repo = FaceRepo::new(&conn);
        let a: Vec<String> = (0..4).map(|i| face(&conn, &key, "d1", 0, 0.05 * i as f32, 0.9)).collect();
        let b: Vec<String> = (0..3).map(|i| face(&conn, &key, "d1", 6, 0.05 * i as f32, 0.8)).collect();
        let loner = face(&conn, &key, "d1", 12, 0.0, 0.7);

        // A face the owner already grouped (and named) is left alone, even
        // though it looks just like group A.
        let named = face(&conn, &key, "d1", 0, 0.02, 0.95);
        let person = repo.tag_face_with_name(&named, "Aimee").unwrap();
        let named_cluster = cluster_of(&conn, &named).unwrap();

        let report = repo.group_ungrouped(Some("d1"), &key).unwrap();
        assert_eq!(report.faces_considered, 8);
        assert_eq!(report.groups_created, 2);
        assert_eq!(report.faces_grouped, 7);

        let ga = cluster_of(&conn, &a[0]).expect("group A");
        assert!(a.iter().all(|f| cluster_of(&conn, f).as_ref() == Some(&ga)));
        let gb = cluster_of(&conn, &b[0]).expect("group B");
        assert!(b.iter().all(|f| cluster_of(&conn, f).as_ref() == Some(&gb)));
        assert_ne!(ga, gb, "two people are two groups");
        assert_eq!(cluster_of(&conn, &loner), None, "no group of one");
        assert_eq!(cluster_of(&conn, &named), Some(named_cluster), "a named face stays put");
        assert_eq!(repo.get_person(&person.id).unwrap().unwrap().display_name, "Aimee");

        // Running again finds nothing new to do.
        let again = repo.group_ungrouped(Some("d1"), &key).unwrap();
        assert_eq!(again.groups_created, 0);
    }

    /// Grouping one drive does not reach into another.
    #[test]
    fn grouping_a_drive_leaves_other_drives_alone() {
        let (conn, key) = archive();
        let repo = FaceRepo::new(&conn);
        let other: Vec<String> = (0..3).map(|i| face(&conn, &key, "d2", 0, 0.05 * i as f32, 0.9)).collect();
        repo.group_ungrouped(Some("d1"), &key).unwrap();
        assert!(other.iter().all(|f| cluster_of(&conn, f).is_none()));
        repo.group_ungrouped(None, &key).unwrap();
        assert!(other.iter().all(|f| cluster_of(&conn, f).is_some()));
    }

    /// D-089: the gallery shows a group once, by its clearest face, with the
    /// group's size — biggest groups first.
    #[test]
    fn the_gallery_shows_each_group_once_biggest_first() {
        let (conn, key) = archive();
        let repo = FaceRepo::new(&conn);
        let a: Vec<String> = (0..4).map(|i| face(&conn, &key, "d1", 0, 0.05 * i as f32, 0.5 + 0.1 * i as f32)).collect();
        (0..2).for_each(|i| {
            face(&conn, &key, "d1", 6, 0.05 * i as f32, 0.99);
        });
        let loner = face(&conn, &key, "d2", 12, 0.0, 0.7);
        repo.group_ungrouped(None, &key).unwrap();

        let tiles = repo.gallery(200).unwrap();
        assert_eq!(tiles.len(), 3, "{tiles:?}");
        assert_eq!(tiles[0].group_size, 4, "biggest group first");
        assert_eq!(tiles[0].face_id, a[3], "shown by its clearest face");
        assert_eq!(tiles[1].group_size, 2);
        assert_eq!(tiles[2].face_id, loner);
        assert_eq!(tiles[2].group_size, 1);

        let on_d2 = repo.gallery_on_drive(200, Some(2)).unwrap();
        assert_eq!(on_d2.len(), 1);
        assert_eq!(on_d2[0].face_id, loner);
    }

    /// D-091: the same person grouped separately on two drives becomes one
    /// group; a different person stays apart; a named group is never touched.
    #[test]
    fn the_same_person_on_two_drives_becomes_one_group() {
        let (conn, key) = archive();
        let repo = FaceRepo::new(&conn);
        let on_1: Vec<String> = (0..3).map(|i| face(&conn, &key, "d1", 0, 0.05 * i as f32, 0.9)).collect();
        let on_2: Vec<String> = (0..3).map(|i| face(&conn, &key, "d2", 0, 0.04 * i as f32, 0.8)).collect();
        let other: Vec<String> = (0..3).map(|i| face(&conn, &key, "d1", 6, 0.05 * i as f32, 0.9)).collect();
        repo.group_ungrouped(Some("d1"), &key).unwrap();
        repo.group_ungrouped(Some("d2"), &key).unwrap();
        assert_ne!(cluster_of(&conn, &on_1[0]), cluster_of(&conn, &on_2[0]), "grouped per drive first");

        // A named group that looks just like them stays where it is.
        let named: Vec<String> = (0..2).map(|i| face(&conn, &key, "d2", 0, 0.03 * i as f32, 0.95)).collect();
        repo.tag_face_with_name(&named[0], "Aimee").unwrap();
        let named_cluster = cluster_of(&conn, &named[0]).unwrap();

        let r = repo.merge_lookalike_groups(&key).unwrap();
        assert_eq!(r.groups_merged, 1, "{r:?}");
        let one = cluster_of(&conn, &on_1[0]).unwrap();
        assert!(on_1.iter().chain(&on_2).all(|f| cluster_of(&conn, f).as_ref() == Some(&one)));
        assert!(other.iter().all(|f| cluster_of(&conn, f).as_ref() != Some(&one)), "a different person stays apart");
        assert_eq!(cluster_of(&conn, &named[0]), Some(named_cluster), "a named group is never merged");

        // And running it again changes nothing.
        assert_eq!(repo.merge_lookalike_groups(&key).unwrap().groups_merged, 0);
    }

    /// The per-drive counts come from the catalogue, not from a sample.
    #[test]
    fn unnamed_counts_are_real_counts() {
        let (conn, key) = archive();
        let repo = FaceRepo::new(&conn);
        for i in 0..5 {
            face(&conn, &key, "d1", 0, 0.05 * i as f32, 0.9);
        }
        face(&conn, &key, "d1", 12, 0.0, 0.9);
        let named = face(&conn, &key, "d2", 6, 0.0, 0.9);
        repo.tag_face_with_name(&named, "Kent").unwrap();
        repo.group_ungrouped(None, &key).unwrap();

        let counts = repo.unnamed_counts().unwrap();
        assert_eq!(counts.len(), 1, "a drive whose faces are all named has none left: {counts:?}");
        assert_eq!(counts[0].drive_number, 1);
        assert_eq!(counts[0].faces, 6);
        assert_eq!(counts[0].groups, 2, "one group of five and one loner");
    }
}
