//! A plain-language guess at what each folder on a drive contains.
//!
//! The owner's ask, verbatim in spirit: "This folder appears to contain an
//! event for Crown Paints. This folder appears to contain product photography
//! for plastic packaging. This folder appears to contain a family photo
//! shoot." Twenty drives deep, folder names alone stop being enough.
//!
//! Everything here is derived from what indexing already recorded — subject
//! tags, face counts, names read off things in the pictures, dates. No drive
//! needs to be connected and nothing is re-read. Because these are guesses
//! assembled from evidence, every sentence hedges ("looks like") and carries
//! its evidence (counts, dates, names) so the owner can judge it.

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// One folder, one guess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderSummary {
    /// The shoot folder as it appears on the drive.
    pub folder: String,
    pub photo_count: i64,
    /// "Looks like a wedding — 758 photos from August 2017."
    pub description: String,
    pub earliest: Option<String>,
    pub latest: Option<String>,
}

/// Folder components that are workflow, not identity.
///
/// A photographer's shoot folder often ends in `raws`, `edits`, `waynes
/// edits`, `exports` — the same shoot split by processing stage. Those levels
/// are stripped so the summary lands on the shoot, not the stage.
const GENERIC_COMPONENTS: &[&str] = &[
    "raw", "raws", "jpeg", "jpegs", "jpg", "jpgs", "tif", "tiff", "tiffs", "psd",
    "edit", "edits", "edited", "export", "exports", "select", "selects", "final",
    "finals", "highres", "lowres", "web", "print", "prints", "images", "pics",
    "photos", "waynesedits", "wayneedits", "stax", "backup",
];

/// Top-level components that are a computer's furniture, not a folder the
/// owner made: a drive cloned from a Mac starts at `Desktop` or `Pictures`.
const FURNITURE: &[&str] = &["desktop", "documents", "pictures", "downloads", "users", "volumes"];

fn is_generic(component: &str) -> bool {
    let plain: String = component
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    GENERIC_COMPONENTS.contains(&plain.as_str())
}

/// The shoot folder a photograph belongs to, from its path on the drive.
pub fn folder_key(relative_path: &str) -> String {
    let mut parts: Vec<&str> = relative_path.split('/').collect();
    parts.pop(); // the filename
    // Peel window furniture off the front...
    while let Some(first) = parts.first() {
        if FURNITURE.contains(&first.to_lowercase().as_str()) {
            parts.remove(0);
        } else {
            break;
        }
    }
    // ...and workflow stages off the back.
    while let Some(last) = parts.last() {
        if is_generic(last) && parts.len() > 1 {
            parts.pop();
        } else {
            break;
        }
    }
    match parts.first() {
        Some(first) if !is_generic(first) => first.to_string(),
        Some(first) => first.to_string(),
        None => "(top of the drive)".to_string(),
    }
}

/// What the classifier knows about one folder.
#[derive(Debug, Default)]
struct Evidence {
    photos: i64,
    /// tag name → photographs carrying it.
    tags: BTreeMap<String, i64>,
    /// Photographs with at least one face.
    with_faces: i64,
    faces_total: i64,
    /// name-tags (read off objects in the pictures) → count.
    names: BTreeMap<String, i64>,
    earliest: Option<String>,
    latest: Option<String>,
}

impl Evidence {
    fn tag_share(&self, tag: &str) -> f64 {
        if self.photos == 0 {
            return 0.0;
        }
        *self.tags.get(tag).unwrap_or(&0) as f64 / self.photos as f64
    }
    fn any_share(&self, tags: &[&str]) -> f64 {
        tags.iter().map(|t| self.tag_share(t)).fold(0.0, f64::max)
    }
    fn face_share(&self) -> f64 {
        if self.photos == 0 {
            return 0.0;
        }
        self.with_faces as f64 / self.photos as f64
    }
    /// The most-read name in the folder, if it was read often enough to mean
    /// something — three photographs, not one stray label.
    fn dominant_name(&self) -> Option<(&str, i64)> {
        self.names
            .iter()
            .max_by_key(|(_, n)| **n)
            .filter(|(_, n)| **n >= 3)
            .map(|(name, n)| (name.as_str(), *n))
    }
    /// Object tags worth naming in a product summary, biggest first.
    fn top_object_tags(&self) -> Vec<&str> {
        const OBJECTS: &[&str] = &[
            "container", "carton", "bottle", "tableware", "food", "drinking_glass",
            "consumer_electronics", "tool", "utensil", "furniture", "textile",
            "jewelry", "footwear", "vegetable", "tomato", "lettuce", "meat", "plate",
            "bowl", "machine", "carton", "document", "cord", "optical_equipment",
        ];
        let mut hits: Vec<(&str, i64)> = OBJECTS
            .iter()
            .filter_map(|t| self.tags.get(*t).map(|n| (*t, *n)))
            .filter(|(_, n)| *n * 5 >= self.photos.max(1)) // at least a fifth
            .collect();
        hits.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        hits.into_iter().take(2).map(|(t, _)| t).collect()
    }
}

/// When the photographs were taken, said the way a person would.
fn when(earliest: &Option<String>, latest: &Option<String>) -> String {
    let month_name = |m: &str| -> Option<&'static str> {
        Some(match m {
            "01" => "January", "02" => "February", "03" => "March", "04" => "April",
            "05" => "May", "06" => "June", "07" => "July", "08" => "August",
            "09" => "September", "10" => "October", "11" => "November", "12" => "December",
            _ => return None,
        })
    };
    match (earliest, latest) {
        (Some(a), Some(b)) if a.len() >= 7 && b.len() >= 7 => {
            let (ya, ma) = (&a[..4], &a[5..7]);
            let (yb, mb) = (&b[..4], &b[5..7]);
            if ya == yb && ma == mb {
                match month_name(ma) {
                    Some(m) => format!("from {m} {ya}"),
                    None => format!("from {ya}"),
                }
            } else if ya == yb {
                format!("from {ya}")
            } else {
                format!("from {ya} to {yb}")
            }
        }
        _ => "dates unknown".to_string(),
    }
}

/// Turn evidence into one honest sentence.
fn describe(folder: &str, ev: &Evidence) -> String {
    let n = ev.photos;
    let dates = when(&ev.earliest, &ev.latest);
    let name_note = ev
        .dominant_name()
        .map(|(name, count)| format!(" The name \u{201c}{name}\u{201d} shows up in {count} photos."))
        .unwrap_or_default();

    // Ordered from the most specific evidence to the least. Each rule states
    // what it saw, so a wrong guess is at least a checkable one.
    let what = if ev.tag_share("likely-scan") >= 0.4 {
        "old prints that were scanned in".to_string()
    } else if ev.any_share(&["bride", "wedding_dress", "groom"]) >= 0.08
        || ev.tag_share("wedding") >= 0.15
    {
        "a wedding".to_string()
    } else if ev.face_share() >= 0.3 && ev.tag_share("child") >= 0.2 {
        "a family shoot".to_string()
    } else if ev.face_share() >= 0.6 && ev.faces_total * 2 <= ev.with_faces * 3 {
        // One face per photo, photo after photo, is a portrait session — and
        // it must be judged before the event rule, because headshots are shot
        // in suits and "suit" is exactly what the event rule looks for. A real
        // folder of 36 headshots was called "an event" until this moved up.
        "portrait shots".to_string()
    } else if ev.face_share() < 0.2 && !ev.top_object_tags().is_empty() {
        let objects = ev
            .top_object_tags()
            .iter()
            .map(|t| t.replace('_', " "))
            .collect::<Vec<_>>()
            .join(" and ");
        format!("product photography \u{2014} mostly {objects}")
    } else if ev.face_share() >= 0.5
        && ev.any_share(&["celebration", "crowd", "suit", "ceremony"]) >= 0.15
    {
        "an event".to_string()
    } else if ev.face_share() >= 0.5 {
        "photos of people".to_string()
    } else {
        // Say what was seen rather than pretending to know.
        let mut top: Vec<(&String, &i64)> = ev
            .tags
            .iter()
            .filter(|(t, _)| !matches!(t.as_str(), "adult" | "people" | "clothing" | "material" | "structure"))
            .collect();
        top.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        let words: Vec<String> = top
            .iter()
            .take(2)
            .map(|(t, _)| t.replace('_', " "))
            .collect();
        if words.is_empty() {
            format!("a mix of photos ({folder})")
        } else {
            format!("a mix of photos \u{2014} mostly {}", words.join(" and "))
        }
    };

    format!("Looks like {what}. {n} photos, {dates}.{name_note}")
}

/// Summarise every shoot folder on a drive, largest first.
pub fn folder_summaries(conn: &Connection, drive_number: i64) -> Result<Vec<FolderSummary>> {
    let mut folders: BTreeMap<String, Evidence> = BTreeMap::new();

    // One pass for files and dates, one for tags, one for faces and names —
    // grouped in Rust because the folder key is derived, not stored.
    {
        let mut stmt = conn.prepare(
            "SELECT f.relative_path,
                    (SELECT de.earliest_date FROM date_estimates de WHERE de.file_id = f.id),
                    (SELECT count(*) FROM faces fa
                      WHERE fa.file_id = f.id AND fa.is_false_detection = 0)
               FROM files f JOIN drives d ON d.id = f.drive_id
              WHERE d.drive_number = ?1 AND f.status = 'complete'",
        )?;
        let rows = stmt.query_map([drive_number], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (rel, date, faces) = row?;
            let ev = folders.entry(folder_key(&rel)).or_default();
            ev.photos += 1;
            if faces > 0 {
                ev.with_faces += 1;
            }
            ev.faces_total += faces;
            if let Some(dt) = date {
                if ev.earliest.as_ref().is_none_or(|e| dt < *e) {
                    ev.earliest = Some(dt.clone());
                }
                if ev.latest.as_ref().is_none_or(|l| dt > *l) {
                    ev.latest = Some(dt);
                }
            }
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT f.relative_path, t.name, ft.source
               FROM file_tags ft
               JOIN tags t  ON t.id = ft.tag_id
               JOIN files f ON f.id = ft.file_id
               JOIN drives d ON d.id = f.drive_id
              WHERE d.drive_number = ?1 AND f.status = 'complete'
                AND t.tag_type <> 'person'",
        )?;
        let rows = stmt.query_map([drive_number], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (rel, tag, source) = row?;
            let ev = folders.entry(folder_key(&rel)).or_default();
            if source == "name" || source == "brand" {
                *ev.names.entry(tag).or_default() += 1;
            } else {
                *ev.tags.entry(tag).or_default() += 1;
            }
        }
    }

    let mut out: Vec<FolderSummary> = folders
        .into_iter()
        .map(|(folder, ev)| FolderSummary {
            description: describe(&folder, &ev),
            photo_count: ev.photos,
            earliest: ev.earliest,
            latest: ev.latest,
            folder,
        })
        .collect();
    out.sort_by_key(|f| std::cmp::Reverse(f.photo_count));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real shapes from the owner's drives, verbatim.
    #[test]
    fn the_shoot_folder_is_found_through_the_workflow_layers() {
        assert_eq!(
            folder_key("Desktop/05August17Wedding backup/waynes edits/_8104515.jpg"),
            "05August17Wedding backup"
        );
        assert_eq!(folder_key("Ashberry recruitment/x.jpg"), "Ashberry recruitment");
        assert_eq!(
            folder_key("Desktop/CinCin16Aug17/raws/_8109013-Edit.tif"),
            "CinCin16Aug17"
        );
        assert_eq!(folder_key("IMG_0001.jpg"), "(top of the drive)");
    }

    fn ev(photos: i64) -> Evidence {
        Evidence { photos, ..Default::default() }
    }
    fn tag(e: &mut Evidence, name: &str, n: i64) {
        e.tags.insert(name.into(), n);
    }

    /// The owner's first example: "an event for Crown Paints".
    #[test]
    fn an_event_with_a_readable_client_name_says_so() {
        let mut e = ev(132);
        e.with_faces = 110;
        e.faces_total = 700;
        tag(&mut e, "celebration", 40);
        tag(&mut e, "crowd", 35);
        e.names.insert("crown-paints".into(), 24);
        e.earliest = Some("2025-07-06".into());
        e.latest = Some("2025-07-06".into());

        let s = describe("Crown 6th July 2025", &e);
        assert!(s.contains("an event"), "{s}");
        assert!(s.contains("crown-paints"), "{s}");
        assert!(s.contains("24 photos"), "{s}");
        assert!(s.contains("July 2025"), "{s}");
    }

    /// The second example: "product photography for plastic packaging".
    #[test]
    fn product_photography_names_what_is_on_the_table() {
        let mut e = ev(306);
        e.with_faces = 12; // hands in a few frames, not a people shoot
        tag(&mut e, "container", 210);
        tag(&mut e, "carton", 120);
        e.earliest = Some("2026-03-01".into());
        e.latest = Some("2026-03-02".into());

        let s = describe("JWP", &e);
        assert!(s.contains("product photography"), "{s}");
        assert!(s.contains("container"), "{s}");
        assert!(s.contains("306 photos"), "{s}");
    }

    /// The third example: "a family photo shoot".
    #[test]
    fn a_family_shoot_is_recognised_by_its_children() {
        let mut e = ev(85);
        e.with_faces = 70;
        e.faces_total = 240;
        tag(&mut e, "child", 40);
        let s = describe("Cake smash 20th june 2026", &e);
        assert!(s.contains("a family shoot"), "{s}");
    }

    #[test]
    fn a_wedding_outranks_a_generic_event() {
        let mut e = ev(758);
        e.with_faces = 700;
        tag(&mut e, "bride", 90);
        tag(&mut e, "wedding_dress", 104);
        tag(&mut e, "celebration", 228);
        let s = describe("Aimee and Kent ", &e);
        assert!(s.contains("a wedding"), "{s}");
    }

    #[test]
    fn scanned_prints_outrank_everything() {
        let mut e = ev(200);
        e.with_faces = 150;
        tag(&mut e, "likely-scan", 120);
        tag(&mut e, "bride", 30);
        let s = describe("Old family albums", &e);
        assert!(s.contains("scanned in"), "{s}");
    }

    /// A folder with nothing distinctive must say what it saw, not invent a
    /// confident story.
    #[test]
    fn an_unclear_folder_admits_it_is_a_mix() {
        let mut e = ev(50);
        e.with_faces = 10;
        tag(&mut e, "sky", 20);
        tag(&mut e, "grass", 15);
        let s = describe("Karls jobs", &e);
        assert!(s.contains("a mix of photos"), "{s}");
        assert!(s.contains("sky"), "{s}");
    }

    /// One stray label must not become "for Crown Paints" — a client claim
    /// needs the name read several times.
    #[test]
    fn a_single_stray_name_is_not_promoted_to_a_client() {
        let mut e = ev(100);
        e.with_faces = 80;
        tag(&mut e, "celebration", 30);
        e.names.insert("kelloggs".into(), 1);
        let s = describe("party", &e);
        assert!(!s.contains("kelloggs"), "{s}");
    }
}

#[cfg(test)]
mod portrait_tests {
    use super::*;

    /// The real folder that exposed the ordering: 36 headshots, suits on,
    /// one face per photo. That is a portrait session, not an event.
    #[test]
    fn headshots_in_suits_are_portraits_not_an_event() {
        let mut e = Evidence { photos: 36, with_faces: 34, faces_total: 36, ..Default::default() };
        e.tags.insert("suit".into(), 30);
        e.tags.insert("celebration".into(), 6);
        let s = describe("RonHutchinsonHeadshots26thOct18", &e);
        assert!(s.contains("portrait shots"), "{s}");
    }

    /// A room full of people stays an event: many faces per photo.
    #[test]
    fn a_crowd_is_still_an_event() {
        let mut e = Evidence { photos: 118, with_faces: 100, faces_total: 620, ..Default::default() };
        e.tags.insert("crowd".into(), 40);
        e.tags.insert("celebration".into(), 30);
        let s = describe("BlackburnYouthZonePatrons25thOct18", &e);
        assert!(s.contains("an event"), "{s}");
    }
}
