//! AtlasDrive desktop backend (Tauri v2).
//!
//! Thin command layer over `family-archive-core`. The GUI and CLI call the same
//! service layer, so all safety guarantees live in core, not here. Long-running
//! indexing is executed on a background thread so the UI stays responsive.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{Manager, State};

use family_archive_core::ai::{CancelToken, Capability, EngineRegistry};
use family_archive_core::config::{AppPaths, Config};
use family_archive_core::crypto::keystore;
use family_archive_core::drive::{manifest::DriveManifest, DriveRepo, RegisterParams};
use family_archive_core::logging::Logger;
use family_archive_core::pipeline::{IndexMode, IndexOptions, Pipeline};
use family_archive_core::progress::Progress;
use family_archive_core::search::{SearchFilters, SearchResult, VisualQuery};
use family_archive_core::verifier::{self, Check};
use family_archive_core::{db, faces};

/// Shared application state.
struct AppState {
    paths: Mutex<AppPaths>,
    /// Cancel token for the in-flight index run, if any.
    running: Arc<Mutex<Option<CancelToken>>>,
}

fn map_err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn open_archive(paths: &AppPaths) -> Result<rusqlite::Connection, String> {
    db::open(&paths.archive_db(), db::SchemaKind::Archive).map_err(map_err)
}
fn open_queue(paths: &AppPaths) -> Result<rusqlite::Connection, String> {
    db::open(&paths.queue_db(), db::SchemaKind::Queue).map_err(map_err)
}

/// Drive shape the UI consumes (includes a live image count).
#[derive(Serialize)]
struct DriveDto {
    id: String,
    drive_number: i64,
    friendly_name: Option<String>,
    status: String,
    physical_location: Option<String>,
    categories: Vec<String>,
    last_scan_at: Option<String>,
    image_count: i64,
}

#[tauri::command]
fn list_drives(state: State<AppState>) -> Result<Vec<DriveDto>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let repo = DriveRepo::new(&archive);
    let drives = repo.list().map_err(map_err)?;
    let mut out = Vec::new();
    for d in drives {
        let image_count: i64 = archive
            .query_row(
                "SELECT count(*) FROM files WHERE drive_id=?1 AND status='complete'",
                [&d.id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        out.push(DriveDto {
            id: d.id,
            drive_number: d.drive_number,
            friendly_name: d.friendly_name,
            status: d.status,
            physical_location: d.physical_location,
            categories: d.categories,
            last_scan_at: d.last_scan_at,
            image_count,
        });
    }
    Ok(out)
}

#[tauri::command]
fn register_drive(
    state: State<AppState>,
    number: i64,
    path: String,
    name: Option<String>,
    write_manifest: bool,
) -> Result<DriveDto, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let repo = DriveRepo::new(&archive);
    let vol = PathBuf::from(&path);
    let drive = repo
        .register(&RegisterParams {
            drive_number: number,
            friendly_name: name.clone(),
            volume_name: vol.file_name().map(|s| s.to_string_lossy().to_string()),
            ..Default::default()
        })
        .map_err(map_err)?;
    if write_manifest {
        let m = DriveManifest::new(&drive.id, drive.drive_number, name);
        m.write_to_volume(&vol).map_err(map_err)?;
        repo.audit(&drive.id, "manifest_written", None).map_err(map_err)?;
    }
    Ok(DriveDto {
        id: drive.id,
        drive_number: drive.drive_number,
        friendly_name: drive.friendly_name,
        status: drive.status,
        physical_location: drive.physical_location,
        categories: drive.categories,
        last_scan_at: drive.last_scan_at,
        image_count: 0,
    })
}

/// Search results plus a plain-language note about how the query was handled.
#[derive(Serialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
    /// Lexicon terms the local text encoder recognised, for explaining a match.
    understood: Vec<String>,
    /// True when the query carried no visual meaning and only text was searched.
    text_only: bool,
    /// Which drives hold the matches, most matches first.
    drives: Vec<family_archive_core::inventory::DriveMatch>,
    /// One line answering "which drive do I need to connect?".
    where_to_look: String,
}

/// Tag a face group with a person's name.
///
/// This is the only way a name is ever attached to a face. Confirming promotes
/// the group's faces to exemplars, so the person is recognised on later scans.
#[tauri::command]
fn tag_face_cluster(
    state: State<AppState>,
    cluster_id: String,
    name: String,
) -> Result<faces::Person, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    faces::FaceRepo::new(&archive)
        .tag_cluster_with_name(&cluster_id, &name)
        .map_err(map_err)
}

/// Faces to browse, newest and clearest first. No names required.
#[tauri::command]
fn face_gallery(
    state: State<AppState>,
    limit: Option<usize>,
) -> Result<Vec<faces::GalleryFace>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    faces::FaceRepo::new(&archive)
        .gallery(limit.unwrap_or(200))
        .map_err(map_err)
}

/// One face crop, as a data URL the webview can render directly.
///
/// The crop is decrypted here and never written to disk in the clear; the CSP
/// permits `data:` images, so nothing needs to be served from a file path.
#[tauri::command]
fn face_thumbnail(state: State<AppState>, face_id: String) -> Result<Option<String>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let key = keystore::default_keystore(paths.keys_dir())
        .get_or_create()
        .map_err(map_err)?;
    let crop = faces::FaceRepo::new(&archive)
        .thumbnail(&face_id, &key)
        .map_err(map_err)?;
    Ok(crop.map(|(bytes, format)| format!("data:image/{format};base64,{}", b64(&bytes))))
}

/// Minimal base64, to avoid a dependency for one call site.
fn b64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Name a single face from the gallery, creating its group if it has none.
#[derive(Serialize)]
struct TagResult {
    person: faces::Person,
    /// Other faces now proposed as this person, awaiting confirmation.
    suggested: usize,
}

#[tauri::command]
fn tag_face(state: State<AppState>, face_id: String, name: String) -> Result<TagResult, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let repo = faces::FaceRepo::new(&archive);
    let person = repo.tag_face_with_name(&face_id, &name).map_err(map_err)?;

    // Immediately answer "who else is this?" rather than leaving the user to
    // find the same person's other groups by eye.
    let key = keystore::default_keystore(paths.keys_dir())
        .get_or_create()
        .map_err(map_err)?;
    let (model_id, model_version) = face_model_partition(&archive);
    let suggested = repo
        .suggest_for_person(
            &person.id,
            &model_id,
            &model_version,
            &key,
            faces::PERSON_MATCH_THRESHOLD,
        )
        .map_err(map_err)?;

    Ok(TagResult { person, suggested })
}

/// The model partition most of this archive's faces were written under.
fn face_model_partition(archive: &rusqlite::Connection) -> (String, String) {
    archive
        .query_row(
            "SELECT model_id, model_version FROM face_embeddings
              GROUP BY model_id, model_version ORDER BY count(*) DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or_else(|_| {
            (
                family_archive_core::ai::local::MODEL_ID.to_string(),
                family_archive_core::ai::local::MODEL_VERSION.to_string(),
            )
        })
}

/// Faces awaiting a yes/no for a person, most confident first.
#[tauri::command]
fn pending_suggestions(
    state: State<AppState>,
    person_id: String,
    limit: Option<usize>,
) -> Result<Vec<faces::SuggestedFace>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    faces::FaceRepo::new(&archive)
        .pending_suggestions(&person_id, limit.unwrap_or(200))
        .map_err(map_err)
}

/// Accept every outstanding proposal for a person.
#[tauri::command]
fn confirm_suggestions(state: State<AppState>, person_id: String) -> Result<usize, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    faces::FaceRepo::new(&archive)
        .confirm_suggestions(&person_id)
        .map_err(map_err)
}

/// Reject every outstanding proposal for a person, freeing those faces.
#[tauri::command]
fn reject_suggestions(state: State<AppState>, person_id: String) -> Result<usize, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    faces::FaceRepo::new(&archive)
        .reject_suggestions(&person_id)
        .map_err(map_err)
}

/// Say yes or no to one proposed group.
#[tauri::command]
fn resolve_suggestion(
    state: State<AppState>,
    cluster_id: String,
    is_them: bool,
) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let repo = faces::FaceRepo::new(&archive);
    if is_them {
        repo.confirm_cluster_suggestion(&cluster_id).map_err(map_err)
    } else {
        repo.reject_cluster_suggestion(&cluster_id).map_err(map_err)
    }
}

/// Every photograph containing a named person, and which drive holds it.
#[tauri::command]
fn photos_of_person(
    state: State<AppState>,
    person_id: String,
) -> Result<Vec<faces::PersonPhoto>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    faces::FaceRepo::new(&archive)
        .photos_of_person(&person_id)
        .map_err(map_err)
}

/// Copy a person's photographs into a folder the user chose.
///
/// Reads originals and writes only into `destination`. Never moves, never
/// deletes, never writes to the source drive.
#[tauri::command]
fn copy_person_photos(
    state: State<AppState>,
    person_id: String,
    destination: String,
) -> Result<family_archive_core::export::ExportSummary, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let repo = faces::FaceRepo::new(&archive);
    let ids: Vec<String> = repo
        .photos_of_person(&person_id)
        .map_err(map_err)?
        .into_iter()
        .map(|p| p.file_id)
        .collect();
    family_archive_core::export::copy_photos(&archive, &ids, std::path::Path::new(&destination))
        .map_err(map_err)
}

/// Write XMP sidecars next to a person's originals, for Bridge and Lightroom.
///
/// **Writes to the source drive.** Never called automatically — the interface
/// asks first, exactly as it does before writing a drive manifest.
#[tauri::command]
fn write_sidecars_for_person(
    state: State<AppState>,
    person_id: String,
) -> Result<family_archive_core::export::SidecarSummary, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let ids: Vec<String> = faces::FaceRepo::new(&archive)
        .photos_of_person(&person_id)
        .map_err(map_err)?
        .into_iter()
        .map(|p| p.file_id)
        .collect();
    family_archive_core::export::write_xmp_sidecars(&archive, &ids).map_err(map_err)
}

/// A small JPEG of a photograph, as a data URL for the results grid.
///
/// Derived on demand from the catalogue's stored thumbnail rather than served
/// from disk: the stored thumbnails are 512px lossless PNGs (~255KB each), which
/// is right for the catalogue's verified contract but far too heavy to put a
/// hundred of into a grid. This re-encodes to a small JPEG per request.
///
/// Works with the drive disconnected — it reads the local thumbnail, never the
/// original.
#[tauri::command]
fn photo_thumbnail(
    state: State<AppState>,
    file_id: String,
    max_edge: Option<u32>,
) -> Result<Option<String>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let rel: Option<String> = archive
        .query_row(
            "SELECT rel_path FROM thumbnails WHERE file_id = ?1",
            [&file_id],
            |r| r.get(0),
        )
        .ok();
    let Some(rel) = rel else { return Ok(None) };

    let abs = paths.thumbnails_dir().join(rel);
    let Ok(img) = image::open(&abs) else { return Ok(None) };
    let edge = max_edge.unwrap_or(240).clamp(64, 512);
    let small = img.thumbnail(edge, edge);

    let mut jpeg = Vec::new();
    let mut encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::Cursor::new(&mut jpeg), 78);
    encoder
        .encode_image(&small.to_rgb8())
        .map_err(|e| format!("could not encode thumbnail: {e}"))?;
    Ok(Some(format!("data:image/jpeg;base64,{}", b64(&jpeg))))
}

/// Where a person's photographs live, grouped by folder.
#[tauri::command]
fn person_folders(
    state: State<AppState>,
    person_id: String,
) -> Result<Vec<faces::PersonFolder>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    faces::FaceRepo::new(&archive)
        .folders_for_person(&person_id)
        .map_err(map_err)
}

/// Open a folder in Finder. Read-only: it shows a window, nothing more.
#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    let dir = std::path::Path::new(&path);
    if !dir.is_dir() {
        return Err("That folder is not available — connect the drive and try again.".into());
    }
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(dir)
        .spawn()
        .map_err(|e| format!("could not open Finder: {e}"))?;
    Ok(())
}

/// Remove a person added by mistake. Their faces are kept and become unnamed.
#[tauri::command]
fn forget_person(state: State<AppState>, person_id: String) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    faces::FaceRepo::new(&archive)
        .remove_person(&person_id)
        .map_err(map_err)
}

/// Correct a person's name, merging into an existing person on a name clash.
#[tauri::command]
fn rename_person(
    state: State<AppState>,
    person_id: String,
    name: String,
) -> Result<faces::Person, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    faces::FaceRepo::new(&archive)
        .rename_person(&person_id, &name)
        .map_err(map_err)
}

/// Re-scan a drive for photographs added since the last scan.
///
/// Uses the folder the drive was last scanned from, so the user does not have to
/// remember or retype it. Unchanged photographs are skipped, so this is cheap.
#[tauri::command]
fn rescan_drive(state: State<AppState>, drive_number: i64) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let scan_root: Option<String> = archive
        .query_row(
            "SELECT sr.scan_root FROM scan_runs sr
               JOIN drives d ON d.id = sr.drive_id
              WHERE d.drive_number = ?1 AND sr.mode <> 'dry-run'
              ORDER BY sr.started_at DESC LIMIT 1",
            [drive_number],
            |r| r.get(0),
        )
        .ok();
    let Some(root) = scan_root else {
        return Err(format!(
            "Drive {drive_number} has not been scanned yet — start a scan from Scan activity."
        ));
    };
    if !std::path::Path::new(&root).is_dir() {
        return Err(format!(
            "Connect Drive {drive_number} and try again — {root} is not available."
        ));
    }
    drop(archive);
    start_index(state, drive_number, root.clone(), false, false)?;
    Ok(format!("Looking for new photographs in {root}."))
}

/// Everyone the user has named, and how established each is.
#[tauri::command]
fn list_people(state: State<AppState>) -> Result<Vec<faces::NamedPerson>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    faces::FaceRepo::new(&archive).people().map_err(map_err)
}

/// Mark a group as not a person at all (a false detection).
#[tauri::command]
fn reject_face_cluster(state: State<AppState>, cluster_id: String) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    archive
        .execute(
            "UPDATE face_clusters SET status='rejected', updated_at=?2 WHERE id=?1",
            rusqlite::params![cluster_id, family_archive_core::util::now_iso8601()],
        )
        .map_err(map_err)?;
    Ok(())
}

/// Rename a drive, keeping its number and everything indexed from it.
#[tauri::command]
fn rename_drive(
    state: State<AppState>,
    drive_number: i64,
    name: String,
) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let repo = DriveRepo::new(&archive);
    let drive = repo
        .get_by_number(drive_number)
        .map_err(map_err)?
        .ok_or_else(|| format!("Drive {drive_number} is not registered."))?;
    repo.rename(&drive.id, &name).map_err(map_err)
}

/// Every subject the catalogue recognised, for browsing rather than guessing.
#[tauri::command]
fn catalogue_tags(
    state: State<AppState>,
    limit: Option<usize>,
) -> Result<Vec<family_archive_core::inventory::TagCount>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    family_archive_core::inventory::all_tags(&archive, limit.unwrap_or(60)).map_err(map_err)
}

/// What is stored on each drive — answerable with every drive unplugged.
#[tauri::command]
fn drive_contents(
    state: State<AppState>,
    drive_number: Option<i64>,
) -> Result<Vec<family_archive_core::inventory::DriveContents>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    family_archive_core::inventory::drive_contents(&archive, drive_number).map_err(map_err)
}

#[tauri::command]
fn search_catalogue(
    state: State<AppState>,
    query: String,
    drive: Option<i64>,
    include_offline: bool,
) -> Result<SearchResponse, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let repo = family_archive_core::search::SearchRepo::new(&archive);
    let filters = SearchFilters {
        drive_number: drive,
        online_only: !include_offline,
        include_offline,
        limit: 100,
        ..Default::default()
    };

    // Embed the query locally so it can be compared against image embeddings.
    let registry = EngineRegistry::local_default();
    let engine = registry.engine_for(Capability::TextEmbedding);
    let embedded = engine.text_embedding(&query, &CancelToken::new()).ok();
    let visual = embedded.as_ref().map(|q| VisualQuery {
        vector: &q.value.vector,
        model_id: engine.model_id(),
        model_version: engine.model_version(),
        coverage: q.meta.confidence,
    });
    let text_only = embedded.as_ref().is_none_or(|q| q.meta.confidence == 0.0);
    let understood = family_archive_core::ai::text::render_query(&query).matched_terms;

    let mut results = repo
        .natural_language_search(&query, visual, &filters)
        .map_err(map_err)?;
    // Populate a friendly date label from the stored range.
    for r in &mut results {
        if let Some((a, b)) = &r.date_range {
            r.date_label = Some(if a == b {
                format!("Around {a}")
            } else {
                format!("Likely between {} and {}", &a[..4.min(a.len())], &b[..4.min(b.len())])
            });
        }
    }
    // Answer "which drive do I need?" alongside the photographs themselves.
    let mut drives = family_archive_core::inventory::drives_matching(&results);
    family_archive_core::inventory::locate_matches(&archive, &mut drives).map_err(map_err)?;
    let where_to_look = family_archive_core::inventory::where_to_look(&drives);

    Ok(SearchResponse { results, understood, text_only, drives, where_to_look })
}

/// Start (or resume) an index run in the background and return immediately.
///
/// A long scan must never block the UI thread, so this spawns a worker and the
/// interface polls [`get_progress`]. Only one run may be active at a time.
#[tauri::command]
fn start_index(
    state: State<AppState>,
    drive: i64,
    path: String,
    dry_run: bool,
    resume: bool,
) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();

    // Refuse to start a second concurrent run.
    {
        let mut running = state.running.lock().unwrap();
        if running.is_some() {
            return Err("an index run is already in progress".into());
        }
        *running = Some(CancelToken::new());
    }
    let cancel = state.running.lock().unwrap().clone().unwrap();
    let running_slot = state.running.clone();

    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let archive = open_archive(&paths)?;
            let queue = open_queue(&paths)?;
            let key = keystore::default_keystore(paths.keys_dir())
                .get_or_create()
                .map_err(map_err)?;
            let pipeline = Pipeline {
                archive: &archive,
                queue: &queue,
                paths: &paths,
                engines: std::sync::Arc::new(EngineRegistry::local_with_vision()),
                key: &key,
                logger: Logger::new(paths.index_log()),
                cancel,
            };
            let mut opts = IndexOptions::new(drive, path);
            opts.mode = if dry_run { IndexMode::DryRun } else { IndexMode::Normal };
            opts.resume = resume;
            opts.config = Config::default();
            pipeline.run(&opts).map_err(map_err)?;
            Ok(())
        })();
        if let Err(e) = result {
            // Surfaced to the UI through progress.json / index.log.
            eprintln!("index run failed: {e}");
        }
        *running_slot.lock().unwrap() = None;
    });

    Ok(())
}

/// Ask a running index to stop at the next safe boundary.
#[tauri::command]
fn cancel_index(state: State<AppState>) -> Result<(), String> {
    if let Some(token) = state.running.lock().unwrap().as_ref() {
        token.cancel();
    }
    Ok(())
}

/// True while an index run is active.
#[tauri::command]
fn is_indexing(state: State<AppState>) -> Result<bool, String> {
    Ok(state.running.lock().unwrap().is_some())
}

#[tauri::command]
fn get_progress(state: State<AppState>) -> Result<Option<Progress>, String> {
    let paths = state.paths.lock().unwrap().clone();
    Progress::load(&paths).map_err(map_err)
}

#[tauri::command]
fn run_verifier(state: State<AppState>) -> Result<Vec<Check>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let queue = open_queue(&paths)?;
    let key = keystore::default_keystore(paths.keys_dir()).get_or_create().ok();
    let config = Config { free_space_floor_bytes: 0, ..Default::default() };
    let ctx = verifier::VerifyContext {
        archive: &archive,
        queue: Some(&queue),
        paths: &paths,
        config: &config,
        key: key.as_ref(),
        face_model: (
            family_archive_core::ai::local::MODEL_ID.to_string(),
            family_archive_core::ai::local::MODEL_VERSION.to_string(),
        ),
        observed_throughput: None,
        network_blocked_attempts: 0,
    };
    let report = verifier::run(&ctx).map_err(map_err)?;
    Ok(report.checks)
}

#[tauri::command]
fn prepare_review(
    state: State<AppState>,
    limit: usize,
) -> Result<Vec<family_archive_core::faces::ClusterSummary>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let repo = faces::FaceRepo::new(&archive);
    repo.prepare_review(limit).map_err(map_err)
}

/// Record the user's own correction to a photograph's date.
///
/// Returns the phrasing to show, e.g. "Taken on 1998-08-12". The correction
/// outranks the estimator and survives re-analysis.
#[tauri::command]
fn set_date_override(
    state: State<AppState>,
    file_id: String,
    earliest: String,
    latest: Option<String>,
) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let repo = family_archive_core::dates::DateRepo::new(&archive);
    let latest = latest.unwrap_or_else(|| earliest.clone());
    let est = repo
        .set_user_override(&file_id, &earliest, &latest)
        .map_err(map_err)?;
    Ok(family_archive_core::dates::describe(&est))
}

/// Remove a correction, letting AtlasDrive's own estimate apply again.
#[tauri::command]
fn clear_date_override(state: State<AppState>, file_id: String) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    family_archive_core::dates::DateRepo::new(&archive)
        .clear_user_override(&file_id)
        .map_err(map_err)
}

/// Record where a drive physically lives and how it is categorised.
///
/// Both fields are optional and independent: omitting one leaves it alone,
/// rather than blanking it.
#[tauri::command]
fn update_drive_details(
    state: State<AppState>,
    drive_number: i64,
    physical_location: Option<String>,
    categories: Option<Vec<String>>,
) -> Result<DriveDto, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let repo = DriveRepo::new(&archive);
    let drive = repo
        .get_by_number(drive_number)
        .map_err(map_err)?
        .ok_or_else(|| format!("Drive {drive_number} is not registered."))?;
    repo.update_details(
        &drive.id,
        physical_location.as_deref(),
        categories.as_deref(),
    )
    .map_err(map_err)?;

    let updated = repo
        .get_by_number(drive_number)
        .map_err(map_err)?
        .ok_or_else(|| format!("Drive {drive_number} is not registered."))?;
    let image_count: i64 = archive
        .query_row(
            "SELECT count(*) FROM files WHERE drive_id=?1 AND status='complete'",
            [&updated.id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(DriveDto {
        id: updated.id,
        drive_number: updated.drive_number,
        friendly_name: updated.friendly_name,
        status: updated.status,
        physical_location: updated.physical_location,
        categories: updated.categories,
        last_scan_at: updated.last_scan_at,
        image_count,
    })
}

/// Show an indexed original in Finder, when its drive is connected.
///
/// Read-only by construction: `open -R` selects the file in a Finder window and
/// cannot alter it. When the drive is not connected this returns a plain-language
/// message rather than an error string, because a disconnected drive is a normal
/// state in this product, not a fault.
#[tauri::command]
fn reveal_in_finder(state: State<AppState>, file_id: String) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let drive_number: Option<i64> = archive
        .query_row(
            "SELECT d.drive_number FROM files f JOIN drives d ON d.id=f.drive_id WHERE f.id=?1",
            [&file_id],
            |r| r.get(0),
        )
        .ok();

    match family_archive_core::search::resolve_original(&archive, &file_id).map_err(map_err)? {
        Some(path) => {
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("open")
                    .arg("-R")
                    .arg(&path)
                    .spawn()
                    .map_err(|e| format!("could not open Finder: {e}"))?;
            }
            Ok(format!("Showing {} in Finder.", path.display()))
        }
        None => Ok(match drive_number {
            Some(n) => format!("Connect Drive {n} to open the original."),
            None => "That photograph is no longer in the catalogue.".to_string(),
        }),
    }
}

/// Write a privacy-redacted diagnostics bundle and return its path.
///
/// There is no unredacted variant: the export is built from counts and check
/// outcomes, so the user never has to audit it before sharing it.
#[tauri::command]
fn export_diagnostics(state: State<AppState>) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let queue = open_queue(&paths)?;
    let diag =
        family_archive_core::diagnostics::collect(&archive, Some(&queue), &paths, None)
            .map_err(map_err)?;
    let path = family_archive_core::diagnostics::write(&paths, &diag).map_err(map_err)?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn doctor(state: State<AppState>) -> Result<std::collections::BTreeMap<String, String>, String> {
    let paths = state.paths.lock().unwrap().clone();
    let mut out = std::collections::BTreeMap::new();
    let ks = keystore::default_keystore(paths.keys_dir());
    out.insert("keystore".into(), ks.backend_name().into());
    out.insert("key".into(), if ks.get_or_create().is_ok() { "available".into() } else { "error".into() });
    let archive = open_archive(&paths)?;
    out.insert(
        "archive_integrity".into(),
        if db::integrity_check(&archive).is_ok() { "ok".into() } else { "fail".into() },
    );
    let registry = EngineRegistry::local_with_vision();
    out.insert("ai_offline".into(), registry.all_offline().to_string());
    // Whether real image understanding is active, and which engine is doing it.
    // Worth surfacing: without it, search falls back to colour matching and the
    // difference is invisible from the interface.
    match registry.file_analyser() {
        Some(engine) => {
            out.insert(
                "image_recognition".into(),
                format!("{} {}", engine.model_id(), engine.model_version()),
            );
        }
        None => {
            out.insert(
                "image_recognition".into(),
                "unavailable — colour matching only".into(),
            );
        }
    }
    Ok(out)
}

/// Application entry point invoked from `main.rs`.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let paths = AppPaths::discover();
            paths.ensure()?;
            // Ensure both databases exist and are migrated at startup.
            let _ = db::open(&paths.archive_db(), db::SchemaKind::Archive);
            let _ = db::open(&paths.queue_db(), db::SchemaKind::Queue);
            app.manage(AppState {
                paths: Mutex::new(paths),
                running: Arc::new(Mutex::new(None)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_drives,
            register_drive,
            search_catalogue,
            start_index,
            cancel_index,
            is_indexing,
            get_progress,
            run_verifier,
            prepare_review,
            doctor,
            export_diagnostics,
            reveal_in_finder,
            update_drive_details,
            set_date_override,
            clear_date_override,
            drive_contents,
            tag_face_cluster,
            list_people,
            reject_face_cluster,
            rename_drive,
            face_gallery,
            face_thumbnail,
            tag_face,
            photos_of_person,
            copy_person_photos,
            write_sidecars_for_person,
            person_folders,
            open_folder,
            forget_person,
            rename_person,
            rescan_drive,
            confirm_suggestions,
            reject_suggestions,
            resolve_suggestion,
            photo_thumbnail,
            pending_suggestions,
            catalogue_tags
        ])
        .run(tauri::generate_context!())
        .expect("error while running AtlasDrive");
}
