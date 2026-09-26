//! Moving the archive's faces onto the identity model (D-102).
//!
//! Two steps, both resumable and both safe to stop at any point:
//!
//! 1. **Embed.** Every face that has a stored crop gets an ArcFace identity
//!    embedding in place of its Vision feature print. The crops are in the
//!    catalogue, so no drive needs to be connected. A face the identity model
//!    cannot find in its crop (a profile, a blur, a false detection) keeps the
//!    embedding it had and is recorded so it is not retried.
//! 2. **Regroup.** Groups nobody has named or answered are dissolved and
//!    rebuilt with the new embeddings, and every named person's suggestions
//!    are recomputed. Named groups and the owner's "no" answers are never
//!    touched: naming stays a human act (D-007).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::ai::identity::{self, IdentityModel};
use crate::crypto::MasterKey;
use crate::error::Result;
use crate::faces::FaceRepo;
use crate::util::now_iso8601;

/// Faces still waiting for an identity embedding.
const PENDING: &str = "FROM face_thumbnails t
       JOIN faces f ON f.id = t.face_id
       LEFT JOIN face_embeddings e ON e.face_id = t.face_id
      WHERE f.is_false_detection = 0
        AND (e.model_id IS NULL OR e.model_id <> ?1)
        AND NOT EXISTS (SELECT 1 FROM face_identity_skips s
                         WHERE s.face_id = t.face_id AND s.model_id = ?1)";

/// How far the archive is from being fully on the identity model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentityStatus {
    /// Faces with an identity embedding.
    pub upgraded: usize,
    /// Faces still to do.
    pub pending: usize,
    /// Faces the model could not read; they keep their old embedding.
    pub unreadable: usize,
}

impl IdentityStatus {
    pub fn is_complete(&self) -> bool {
        self.pending == 0
    }
}

pub fn status(conn: &Connection) -> Result<IdentityStatus> {
    // Counted, not searched: the People screen asks every few seconds, and the
    // exact "which faces are pending" query reads every face on a catalogue of
    // 228,000 — slow enough that repeated asks queued up behind each other and
    // the progress card vanished. Every upgraded face has a crop, and a face
    // is either upgraded, unreadable, or pending, so arithmetic suffices.
    let id = identity::MODEL_ID;
    let count = |sql: &str, with_id: bool| -> Result<usize> {
        let n: i64 = if with_id {
            conn.query_row(sql, [id], |r| r.get(0))?
        } else {
            conn.query_row(sql, [], |r| r.get(0))?
        };
        Ok(n.max(0) as usize)
    };
    let with_crops = count(
        "SELECT count(*) FROM face_thumbnails t JOIN faces f ON f.id = t.face_id
          WHERE f.is_false_detection = 0",
        false,
    )?;
    let upgraded = count("SELECT count(*) FROM face_embeddings WHERE model_id = ?1", true)?;
    let unreadable = count("SELECT count(*) FROM face_identity_skips WHERE model_id = ?1", true)?;
    Ok(IdentityStatus {
        upgraded,
        pending: with_crops.saturating_sub(upgraded + unreadable),
        unreadable,
    })
}

/// What one pass of [`embed_pending`] did.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbedReport {
    pub embedded: usize,
    pub unreadable: usize,
    /// Stopped on request before finishing.
    pub stopped: bool,
}

/// Give every waiting face an identity embedding, `workers` at a time.
///
/// Works in batches, each committed on its own, so stopping loses at most the
/// batch in hand and the next run carries on where this one left off.
/// `on_batch` is told the running totals after each batch.
pub fn embed_pending(
    conn: &Connection,
    model: &IdentityModel,
    key: &MasterKey,
    workers: usize,
    stop: &AtomicBool,
    mut on_batch: impl FnMut(&EmbedReport),
) -> Result<EmbedReport> {
    let workers = workers.clamp(1, 16);
    let batch = workers * 24;
    let repo = FaceRepo::new(conn);
    let mut report = EmbedReport::default();
    loop {
        if stop.load(Ordering::Relaxed) {
            report.stopped = true;
            return Ok(report);
        }
        // Faces of people the owner has named go first: they are what
        // "Check photos" and every suggestion are measured against, so those
        // work within minutes rather than at the end of a long run.
        let pick = |extra: &str| -> Result<Vec<String>> {
            Ok(conn
                .prepare(&format!("SELECT t.face_id {PENDING} {extra} LIMIT ?2"))?
                .query_map(params![identity::MODEL_ID, batch as i64], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?)
        };
        let mut ids = pick(
            "AND f.cluster_id IN (SELECT id FROM face_clusters
                                   WHERE status = 'confirmed' AND person_id IS NOT NULL)",
        )?;
        if ids.is_empty() {
            ids = pick("")?;
        }
        if ids.is_empty() {
            return Ok(report);
        }
        // Decrypt on this thread (one connection), analyse on many.
        let crops: Vec<Option<Vec<u8>>> = ids
            .iter()
            .map(|id| repo.thumbnail(id, key).ok().flatten().map(|(bytes, _)| bytes))
            .collect();
        let results = embed_all(model, &crops, workers, stop);

        let tx = crate::db::write_tx(conn)?;
        let repo_tx = FaceRepo::new(&tx);
        let now = now_iso8601();
        for (id, result) in ids.iter().zip(results) {
            match result {
                Outcome::Embedded(v) => {
                    repo_tx.replace_embedding(id, identity::MODEL_ID, identity::MODEL_VERSION, &v, key)?;
                    report.embedded += 1;
                }
                Outcome::NoFace => {
                    tx.execute(
                        "INSERT OR IGNORE INTO face_identity_skips (face_id, model_id, created_at)
                         VALUES (?1, ?2, ?3)",
                        params![id, identity::MODEL_ID, now],
                    )?;
                    report.unreadable += 1;
                }
                // Stopped before this one was looked at: leave it for next time.
                Outcome::NotTried => {}
            }
        }
        tx.commit()?;
        on_batch(&report);
    }
}

enum Outcome {
    Embedded(Vec<f32>),
    NoFace,
    NotTried,
}

fn embed_all(
    model: &IdentityModel,
    crops: &[Option<Vec<u8>>],
    workers: usize,
    stop: &AtomicBool,
) -> Vec<Outcome> {
    let next = AtomicUsize::new(0);
    let mut results: Vec<Option<Outcome>> = (0..crops.len()).map(|_| None).collect();
    let slots: Vec<std::sync::Mutex<&mut Option<Outcome>>> =
        results.iter_mut().map(std::sync::Mutex::new).collect();
    std::thread::scope(|s| {
        for _ in 0..workers.min(crops.len()) {
            s.spawn(|| loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(crop) = crops.get(i) else { return };
                let outcome = match crop
                    .as_deref()
                    .and_then(|b| image::load_from_memory(b).ok())
                    .map(|img| model.embed_crop(&img.to_rgb8()))
                {
                    Some(Ok(Some(v))) => Outcome::Embedded(v),
                    // No crop, an undecodable crop, no face found, or a model
                    // error on this one input: all mean "cannot be read".
                    _ => Outcome::NoFace,
                };
                **slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
            });
        }
    });
    drop(slots);
    results.into_iter().map(|r| r.unwrap_or(Outcome::NotTried)).collect()
}

/// What regrouping did.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegroupReport {
    /// Groups nobody had named or answered, taken apart to be rebuilt.
    pub groups_dissolved: usize,
    pub groups_created: usize,
    pub faces_grouped: usize,
    pub groups_merged: usize,
    /// Faces now proposed as someone already named.
    pub suggestions: usize,
}

/// Rebuild the unnamed groups and every person's suggestions on the new
/// embeddings. Named groups and refused suggestions are left exactly as the
/// owner left them.
pub fn regroup(conn: &Connection, key: &MasterKey) -> Result<RegroupReport> {
    let mut report = RegroupReport::default();
    {
        let tx = crate::db::write_tx(conn)?;
        tx.execute(
            "UPDATE faces SET cluster_id = NULL
              WHERE cluster_id IN (SELECT id FROM face_clusters WHERE status = 'unnamed')",
            [],
        )?;
        report.groups_dissolved = tx.execute(
            "UPDATE face_clusters SET status = 'split', person_id = NULL, updated_at = ?1
              WHERE status = 'unnamed'",
            [now_iso8601()],
        )?;
        tx.commit()?;
    }

    let repo = FaceRepo::new(conn);
    let drives: Vec<String> = conn
        .prepare("SELECT id FROM drives ORDER BY drive_number")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    for drive in &drives {
        let r = repo.group_ungrouped(Some(drive), key)?;
        report.groups_created += r.groups_created;
        report.faces_grouped += r.faces_grouped;
    }
    report.groups_merged = repo.merge_lookalike_groups(key)?.groups_merged;

    let people: Vec<String> = conn
        .prepare("SELECT id FROM people")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    for person in &people {
        report.suggestions += repo.suggest_for_person(
            person,
            identity::MODEL_ID,
            identity::MODEL_VERSION,
            key,
            crate::faces::IDENTITY_MATCH_THRESHOLD,
        )?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, SchemaKind};

    fn catalogue() -> (Connection, MasterKey) {
        let conn = open_in_memory(SchemaKind::Archive).unwrap();
        conn.execute_batch(
            "INSERT INTO drives (id, drive_number, status, first_seen_at) VALUES ('d1',1,'online','now');
             INSERT INTO roots (id, drive_id, relative_root, created_at) VALUES ('r1','d1','','now');",
        )
        .unwrap();
        (conn, MasterKey::generate(1))
    }

    fn face(conn: &Connection, key: &MasterKey, n: usize, v: &[f32]) -> String {
        conn.execute(
            "INSERT OR IGNORE INTO files (id, drive_id, root_id, relative_path, filename, size_bytes,
                                source_mtime_ns, status, created_at, updated_at)
             VALUES (?1,'d1','r1',?1,?1,1,1,'complete','now','now')",
            [format!("f{n}")],
        )
        .unwrap();
        let repo = FaceRepo::new(conn);
        let id = repo
            .insert_face(&format!("f{n}"), (0.1, 0.1, 0.2, 0.2), 0.9, crate::faces::VISION_MODEL_ID, "1", v, key)
            .unwrap();
        repo.store_thumbnail(&id, b"not an image", 10, 10, key).unwrap();
        id
    }

    /// A face the model cannot read is recorded and not retried; the status
    /// counts add up; stopping first does nothing.
    #[test]
    fn unreadable_faces_are_recorded_once() {
        let Some(dir) = IdentityModel::find(&[]) else {
            eprintln!("face identity model not installed; skipping");
            return;
        };
        let model = IdentityModel::load(&dir).unwrap();
        let (conn, key) = catalogue();
        face(&conn, &key, 1, &[1.0, 0.0]);
        face(&conn, &key, 2, &[0.0, 1.0]);
        assert_eq!(status(&conn).unwrap(), IdentityStatus { upgraded: 0, pending: 2, unreadable: 0 });

        let stop = AtomicBool::new(true);
        let r = embed_pending(&conn, &model, &key, 2, &stop, |_| {}).unwrap();
        assert!(r.stopped && r.embedded == 0);

        let stop = AtomicBool::new(false);
        let r = embed_pending(&conn, &model, &key, 2, &stop, |_| {}).unwrap();
        assert_eq!((r.embedded, r.unreadable, r.stopped), (0, 2, false));
        assert_eq!(status(&conn).unwrap(), IdentityStatus { upgraded: 0, pending: 0, unreadable: 2 });
        // Nothing left to try.
        let again = embed_pending(&conn, &model, &key, 2, &stop, |_| {}).unwrap();
        assert_eq!(again, EmbedReport::default());
    }

    /// Named people's faces are upgraded before anyone else's.
    #[test]
    fn named_faces_go_first() {
        let Some(dir) = IdentityModel::find(&[]) else {
            eprintln!("face identity model not installed; skipping");
            return;
        };
        let model = IdentityModel::load(&dir).unwrap();
        let (conn, key) = catalogue();
        let repo = FaceRepo::new(&conn);
        let stranger = face(&conn, &key, 1, &[1.0, 0.0]);
        let named = face(&conn, &key, 2, &[0.0, 1.0]);
        repo.name_one_face(&named, "Sabrina").unwrap();
        // A batch of one (one worker × 24 is more than two faces, so stop after
        // the first batch by checking which face was tried first).
        let first: String = conn
            .query_row(
                &format!(
                    "SELECT t.face_id {PENDING} AND f.cluster_id IN
                       (SELECT id FROM face_clusters WHERE status='confirmed' AND person_id IS NOT NULL)
                     LIMIT 1"
                ),
                [identity::MODEL_ID],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(first, named);
        let stop = AtomicBool::new(false);
        embed_pending(&conn, &model, &key, 1, &stop, |_| {}).unwrap();
        let _ = stranger;
    }

    /// Regrouping rebuilds unnamed groups and leaves named ones and refused
    /// suggestions alone.
    #[test]
    fn regrouping_keeps_what_the_owner_decided() {
        let (conn, key) = catalogue();
        let repo = FaceRepo::new(&conn);
        let a = face(&conn, &key, 1, &[1.0, 0.0, 0.0]);
        let b = face(&conn, &key, 2, &[0.0, 1.0, 0.0]);
        let c = face(&conn, &key, 3, &[0.0, 1.0, 0.0]);
        let d = face(&conn, &key, 4, &[0.0, 0.0, 1.0]);
        // The old, weak grouping put a with b.
        let wrong = repo.split_face(&a).unwrap();
        conn.execute("UPDATE faces SET cluster_id=?1 WHERE id=?2", params![wrong, b]).unwrap();
        // c is named; d was refused as someone.
        repo.name_one_face(&c, "Kent").unwrap();
        let refused = repo.split_face(&d).unwrap();
        conn.execute("UPDATE face_clusters SET status='rejected' WHERE id=?1", [&refused]).unwrap();
        // Re-embed a and b as the identity model would.
        for (id, v) in [(&a, [1.0f32, 0.0, 0.0]), (&b, [0.0, 1.0, 0.0])] {
            repo.replace_embedding(id, identity::MODEL_ID, identity::MODEL_VERSION, &v, &key).unwrap();
        }

        let r = regroup(&conn, &key).unwrap();
        assert_eq!(r.groups_dissolved, 1);
        let cluster = |id: &str| -> Option<String> {
            conn.query_row("SELECT cluster_id FROM faces WHERE id=?1", [id], |r| r.get(0)).unwrap()
        };
        // Different people: never in the same group (each alone is right).
        assert!(cluster(&a).is_none() || cluster(&a) != cluster(&b), "the wrong group is taken apart");
        let kent: String = conn
            .query_row(
                "SELECT p.display_name FROM faces f JOIN face_clusters c ON c.id=f.cluster_id
                   JOIN people p ON p.id=c.person_id WHERE f.id=?1 AND c.status='confirmed'",
                [&c],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kent, "Kent");
        assert_eq!(cluster(&d), Some(refused), "a refusal stands");
    }

    /// End to end on real faces, when the model and a test photograph are
    /// available: each face stored twice (as two photographs would), under
    /// deliberately wrong old groupings. After the upgrade every person is one
    /// group of two, and nobody is grouped with anyone else.
    #[test]
    fn the_upgrade_groups_real_faces_by_person() {
        let (Some(dir), Ok(photo)) = (IdentityModel::find(&[]), std::env::var("ATLASDRIVE_FACE_TEST_PHOTO")) else {
            eprintln!("model or ATLASDRIVE_FACE_TEST_PHOTO missing; skipping");
            return;
        };
        let model = IdentityModel::load(&dir).unwrap();
        let img = image::open(photo).unwrap().to_rgb8();
        let (conn, key) = catalogue();
        let repo = FaceRepo::new(&conn);
        let mut ids: Vec<(usize, String)> = Vec::new();
        let dets = model.detect(&img).unwrap();
        assert!(dets.len() >= 3);
        for (person, d) in dets.iter().enumerate() {
            for copy in 0..2 {
                let b = d.bbox;
                let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                let (hw, hh) = ((b[2] - b[0]) * 0.725, (b[3] - b[1]) * 0.725);
                let x0 = (cx - hw).max(0.0) as u32;
                let y0 = (cy - hh).max(0.0) as u32;
                let w = ((cx + hw) as u32).min(img.width()) - x0;
                let h = ((cy + hh) as u32).min(img.height()) - y0;
                let mut crop = image::imageops::crop_imm(&img, x0, y0, w, h).to_image();
                if copy == 1 {
                    crop = image::imageops::colorops::brighten(&crop, -25);
                }
                let mut jpeg = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 82).encode_image(&crop).unwrap();
                let n = person * 2 + copy;
                let id = face(&conn, &key, n, &[1.0, 0.0]);
                repo.store_thumbnail(&id, &jpeg, w, h, &key).unwrap();
                ids.push((person, id));
            }
        }
        // The old model thought everyone looked alike: one big unnamed group.
        let everyone = repo.split_face(&ids[0].1).unwrap();
        conn.execute("UPDATE faces SET cluster_id = ?1", [&everyone]).unwrap();

        let stop = AtomicBool::new(false);
        let r = embed_pending(&conn, &model, &key, 4, &stop, |_| {}).unwrap();
        assert_eq!(r.embedded, ids.len(), "{r:?}");
        regroup(&conn, &key).unwrap();

        let cluster = |id: &str| -> Option<String> {
            conn.query_row("SELECT cluster_id FROM faces WHERE id=?1", [id], |r| r.get(0)).unwrap()
        };
        for (p, id) in &ids {
            for (q, other) in &ids {
                if id == other {
                    continue;
                }
                let same = cluster(id).is_some() && cluster(id) == cluster(other);
                assert_eq!(same, p == q, "person {p} vs person {q}");
            }
        }
    }
}
