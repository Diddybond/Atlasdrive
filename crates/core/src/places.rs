//! Where a photograph was taken, in words (D-106).
//!
//! Cameras and phones record a GPS position; nobody searches for
//! "50.2660, -5.0527". This turns a position into the place names a person
//! would type — town, county, region, country — entirely offline, from a
//! bundled copy of GeoNames' list of every place with at least 1,000 people
//! (CC BY 4.0, <https://www.geonames.org>).
//!
//! The names are stored as `place` tags, so they are searchable by typing,
//! browsable as chips, and found by the same machinery as every other tag.
//! Positions are read from the EXIF the catalogue already holds, so photographs
//! scanned before this existed get their places without their drives.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::sync::OnceLock;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::util::{new_uuid, now_iso8601};

/// The tag type place names are stored under.
pub const PLACE_TAG: &str = "place";

/// Further than this from any town and the photograph gets no place: open sea,
/// wilderness, or a bad fix. A wrong town is worse than none.
const MAX_KM: f64 = 30.0;

static PLACES_GZ: &[u8] = include_bytes!("../data/places.tsv.gz");

#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub lat: f64,
    pub lon: f64,
    pub name: String,
    /// County or district ("Cornwall", "City of York").
    pub county: String,
    /// Region or state ("England", "Ontario").
    pub region: String,
    /// ISO 3166 country code.
    pub country_code: String,
}

impl Place {
    /// The names this place is known by, most specific first, without repeats
    /// ("Manchester, Manchester, England" is "Manchester, England").
    pub fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for n in [
            self.name.as_str(),
            self.county.as_str(),
            self.region.as_str(),
            country_name(&self.country_code).unwrap_or(""),
        ] {
            let n = n.trim();
            if !n.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
                out.push(n.to_string());
            }
        }
        out
    }
}

struct Gazetteer {
    places: Vec<Place>,
    /// Places by whole-degree cell, for nearest-place lookups.
    grid: HashMap<(i32, i32), Vec<u32>>,
}

fn gazetteer() -> &'static Gazetteer {
    static G: OnceLock<Gazetteer> = OnceLock::new();
    G.get_or_init(|| {
        let mut text = String::new();
        flate2::read::GzDecoder::new(PLACES_GZ)
            .read_to_string(&mut text)
            .expect("bundled place list is valid gzip");
        let mut places = Vec::with_capacity(150_000);
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 6 {
                continue;
            }
            let (Ok(lat), Ok(lon)) = (f[0].parse::<f64>(), f[1].parse::<f64>()) else { continue };
            grid.entry(cell(lat, lon)).or_default().push(places.len() as u32);
            places.push(Place {
                lat,
                lon,
                name: f[2].to_string(),
                county: f[3].to_string(),
                region: f[4].to_string(),
                country_code: f[5].to_string(),
            });
        }
        Gazetteer { places, grid }
    })
}

fn cell(lat: f64, lon: f64) -> (i32, i32) {
    (lat.floor() as i32, lon.floor() as i32)
}

fn km_between(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}

/// The nearest town to a position, if one is within [`MAX_KM`].
pub fn nearest(lat: f64, lon: f64) -> Option<&'static Place> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    let g = gazetteer();
    let (cy, cx) = cell(lat, lon);
    let mut best: Option<(&Place, f64)> = None;
    for dy in -1..=1 {
        for dx in -1..=1 {
            // Wrap longitude across the antimeridian.
            let x = (cx + dx + 180).rem_euclid(360) - 180;
            let Some(ids) = g.grid.get(&(cy + dy, x)) else { continue };
            for &i in ids {
                let p = &g.places[i as usize];
                let d = km_between((lat, lon), (p.lat, p.lon));
                if best.is_none_or(|(_, b)| d < b) {
                    best = Some((p, d));
                }
            }
        }
    }
    best.filter(|(_, d)| *d <= MAX_KM).map(|(p, _)| p)
}

/// The GPS position recorded in a photograph's EXIF, as the catalogue keeps
/// it (`"53 deg 28 min 12.3 sec"` plus an `N`/`S`/`E`/`W` reference).
pub fn position_from_exif(raw: &BTreeMap<String, String>) -> Option<(f64, f64)> {
    let coord = |value: &str, reference: Option<&String>, negative: char| -> Option<f64> {
        let nums: Vec<f64> = value
            .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter_map(|s| s.parse::<f64>().ok())
            .collect();
        let v = match nums.as_slice() {
            [d, m, s, ..] => d + m / 60.0 + s / 3600.0,
            [d] => *d,
            _ => return None,
        };
        if !v.is_finite() {
            return None;
        }
        let neg = reference.is_some_and(|r| r.trim().starts_with(negative));
        Some(if neg { -v.abs() } else { v })
    };
    let lat = coord(raw.get("GPSLatitude")?, raw.get("GPSLatitudeRef"), 'S')?;
    let lon = coord(raw.get("GPSLongitude")?, raw.get("GPSLongitudeRef"), 'W')?;
    // 0,0 is what a camera writes when it had no fix.
    if lat.abs() < 1e-6 && lon.abs() < 1e-6 {
        return None;
    }
    Some((lat, lon))
}

/// Tag one photograph with the place it was taken, from its EXIF. Returns the
/// place, or `None` when the photograph has no usable position.
pub fn tag_file(conn: &Connection, file_id: &str, raw: &BTreeMap<String, String>) -> Result<Option<&'static Place>> {
    let Some((lat, lon)) = position_from_exif(raw) else { return Ok(None) };
    let Some(place) = nearest(lat, lon) else { return Ok(None) };
    let now = now_iso8601();
    for name in place.names() {
        let tag_id: String = match conn.query_row(
            "SELECT id FROM tags WHERE name = ?1 AND tag_type = ?2",
            params![name, PLACE_TAG],
            |r| r.get(0),
        ) {
            Ok(id) => id,
            Err(_) => {
                let id = new_uuid();
                conn.execute(
                    "INSERT INTO tags (id, name, tag_type, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![id, name, PLACE_TAG, now],
                )?;
                id
            }
        };
        conn.execute(
            "INSERT OR IGNORE INTO file_tags (file_id, tag_id, confidence, source, created_at)
             VALUES (?1, ?2, 1.0, 'automatic', ?3)",
            params![file_id, tag_id, now],
        )?;
    }
    Ok(Some(place))
}

/// What a places pass found.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlacesReport {
    /// Photographs with a GPS position that were looked at.
    pub with_position: usize,
    /// Of those, the ones now tagged with a place.
    pub placed: usize,
}

/// Give every catalogued photograph with a GPS position its place tags,
/// from the EXIF already stored. Photographs already placed are skipped, so
/// running it again only does new work. No drive is read.
pub fn backfill(conn: &Connection) -> Result<PlacesReport> {
    let rows: Vec<(String, String)> = conn
        .prepare(
            "SELECT m.file_id, m.raw_json FROM metadata m
              WHERE m.raw_json LIKE '%GPSLatitude%'
                AND NOT EXISTS (SELECT 1 FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
                                 WHERE ft.file_id = m.file_id AND t.tag_type = 'place')",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut report = PlacesReport::default();
    for chunk in rows.chunks(2000) {
        let tx = conn.unchecked_transaction()?;
        for (file_id, raw_json) in chunk {
            let Ok(raw) = serde_json::from_str::<BTreeMap<String, String>>(raw_json) else { continue };
            if position_from_exif(&raw).is_none() {
                continue;
            }
            report.with_position += 1;
            if tag_file(&tx, file_id, &raw)?.is_some() {
                report.placed += 1;
                refresh_search_text(&tx, file_id)?;
            }
        }
        tx.commit()?;
    }
    Ok(report)
}

/// Rebuild one photograph's searchable tag text after its tags changed.
pub fn refresh_search_text(conn: &Connection, file_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE files_fts
            SET tags = (SELECT coalesce(group_concat(name, ' '), '')
                          FROM (SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
                                 WHERE ft.file_id = ?1 ORDER BY t.name))
          WHERE file_id = ?1",
        [file_id],
    )?;
    Ok(())
}

/// A place and how many photographs were taken there.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaceCount {
    pub name: String,
    pub photographs: i64,
}

/// The places with the most photographs, for chips on the search screen.
pub fn top_places(conn: &Connection, limit: usize) -> Result<Vec<PlaceCount>> {
    let out = conn
        .prepare(
            "SELECT t.name, count(*) AS n
               FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
               JOIN files f ON f.id = ft.file_id AND f.status = 'complete'
              WHERE t.tag_type = 'place'
              GROUP BY t.id ORDER BY n DESC, t.name LIMIT ?1",
        )?
        .query_map([limit as i64], |r| Ok(PlaceCount { name: r.get(0)?, photographs: r.get(1)? }))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(out)
}

/// English names for the countries a UK archive is likeliest to hold. Anywhere
/// else is still named by its town, county and region.
fn country_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "GB" => "United Kingdom",
        "IE" => "Ireland",
        "FR" => "France",
        "ES" => "Spain",
        "PT" => "Portugal",
        "IT" => "Italy",
        "DE" => "Germany",
        "NL" => "Netherlands",
        "BE" => "Belgium",
        "LU" => "Luxembourg",
        "CH" => "Switzerland",
        "AT" => "Austria",
        "DK" => "Denmark",
        "NO" => "Norway",
        "SE" => "Sweden",
        "FI" => "Finland",
        "IS" => "Iceland",
        "PL" => "Poland",
        "CZ" => "Czechia",
        "HU" => "Hungary",
        "HR" => "Croatia",
        "GR" => "Greece",
        "CY" => "Cyprus",
        "MT" => "Malta",
        "TR" => "Turkey",
        "US" => "United States",
        "CA" => "Canada",
        "MX" => "Mexico",
        "AU" => "Australia",
        "NZ" => "New Zealand",
        "AE" => "United Arab Emirates",
        "EG" => "Egypt",
        "MA" => "Morocco",
        "TN" => "Tunisia",
        "ZA" => "South Africa",
        "TH" => "Thailand",
        "JP" => "Japan",
        "CN" => "China",
        "IN" => "India",
        "SG" => "Singapore",
        "BR" => "Brazil",
        "AR" => "Argentina",
        "JM" => "Jamaica",
        "BB" => "Barbados",
        "DO" => "Dominican Republic",
        "CU" => "Cuba",
        "GI" => "Gibraltar",
        "IM" => "Isle of Man",
        "JE" => "Jersey",
        "GG" => "Guernsey",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(lat: &str, lat_ref: &str, lon: &str, lon_ref: &str) -> BTreeMap<String, String> {
        [
            ("GPSLatitude", lat),
            ("GPSLatitudeRef", lat_ref),
            ("GPSLongitude", lon),
            ("GPSLongitudeRef", lon_ref),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn reads_the_position_as_the_catalogue_stores_it() {
        let r = raw("53 deg 28 min 51.4 sec", "N", "2 deg 14 min 38.7 sec", "W");
        let (lat, lon) = position_from_exif(&r).unwrap();
        assert!((lat - 53.4809).abs() < 1e-3 && (lon + 2.2441).abs() < 1e-3, "{lat} {lon}");
        assert!(position_from_exif(&raw("0 deg 0 min 0 sec", "N", "0 deg 0 min 0 sec", "E")).is_none());
        assert!(position_from_exif(&BTreeMap::new()).is_none());
    }

    #[test]
    fn names_the_town_county_region_and_country() {
        // Central Manchester.
        let p = nearest(53.4809, -2.2441).unwrap();
        assert_eq!(p.names(), ["Manchester", "England", "United Kingdom"]);
        // Near Werrington, on the Cornwall–Devon border.
        let names = nearest(50.667, -4.37).unwrap().names();
        assert!(names.contains(&"Cornwall".to_string()), "{names:?}");
        // The middle of the Atlantic is nowhere.
        assert!(nearest(45.0, -30.0).is_none());
    }

    #[test]
    fn backfills_from_stored_exif_and_is_searchable() {
        let conn = crate::db::open_in_memory(crate::db::SchemaKind::Archive).unwrap();
        conn.execute_batch(
            "INSERT INTO drives (id, drive_number, status, first_seen_at) VALUES ('d1',1,'online','now');
             INSERT INTO roots (id, drive_id, relative_root, created_at) VALUES ('r1','d1','','now');
             INSERT INTO files (id, drive_id, root_id, relative_path, filename, size_bytes, source_mtime_ns, status, created_at, updated_at)
               VALUES ('f1','d1','r1','a.jpg','a.jpg',1,1,'complete','now','now'),
                      ('f2','d1','r1','b.jpg','b.jpg',1,1,'complete','now','now');
             INSERT INTO files_fts (file_id, filename, relative_path, tags, ocr_text, description)
               VALUES ('f1','a.jpg','a.jpg','','',''), ('f2','b.jpg','b.jpg','','','');",
        )
        .unwrap();
        let manchester = serde_json::to_string(&raw("53 deg 28 min 51.4 sec", "N", "2 deg 14 min 38.7 sec", "W")).unwrap();
        conn.execute(
            "INSERT INTO metadata (file_id, raw_json) VALUES ('f1', ?1), ('f2', '{}')",
            [manchester],
        )
        .unwrap();

        let r = backfill(&conn).unwrap();
        assert_eq!(r, PlacesReport { with_position: 1, placed: 1 });
        // Typing a place finds the photograph.
        let hit: String = conn
            .query_row("SELECT file_id FROM files_fts WHERE files_fts MATCH 'manchester'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(hit, "f1");
        let top = top_places(&conn, 10).unwrap();
        assert!(top.iter().any(|p| p.name == "England" && p.photographs == 1));
        // A second pass has nothing new to do.
        assert_eq!(backfill(&conn).unwrap(), PlacesReport::default());
    }
}
