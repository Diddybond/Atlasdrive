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
    /// Why the last background run stopped, if it stopped badly.
    ///
    /// Indexing runs on its own thread, so a failure there has no caller to
    /// return to. Without somewhere to put it the error went to stderr, which
    /// in a packaged app goes nowhere: the screen kept saying "Looking for new
    /// photographs in ..." and the owner watched nothing happen for a day. A
    /// scan that dies has to be able to say so.
    last_error: Arc<Mutex<Option<String>>>,
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
    /// Something worth saying about the registration that is not a failure —
    /// an identity file that could not be written to a read-only drive, say.
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[tauri::command]
async fn list_drives(state: State<'_, AppState>) -> Result<Vec<DriveDto>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<DriveDto>, String> {
        let archive = open_archive(&paths)?;
        let repo = DriveRepo::new(&archive);
        let drives = repo.list().map_err(map_err)?;
        // DriveRepo::list already resolves live connection status; doing it again
        // here is how the two ended up disagreeing in the first place.
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
                friendly_name: d.friendly_name,
                status: d.status,
                drive_number: d.drive_number,
                physical_location: d.physical_location,
                categories: d.categories,
                last_scan_at: d.last_scan_at,
                image_count,
                note: None,
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn register_drive(
    state: State<'_, AppState>,
    number: i64,
    path: String,
    name: Option<String>,
    write_manifest: bool,
) -> Result<DriveDto, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<DriveDto, String> {
        let archive = open_archive(&paths)?;
        let repo = DriveRepo::new(&archive);
        let vol = PathBuf::from(&path);
        let drive = repo
            .register(&RegisterParams {
                drive_number: number,
                friendly_name: name.clone(),
                volume_name: vol.file_name().map(|s| s.to_string_lossy().to_string()),
                // Remembered so the drive can be scanned straight after
                // registering; without it there is nothing to scan until a scan has
                // already run.
                registered_root: Some(path.clone()),
                ..Default::default()
            })
            .map_err(map_err)?;
        // The identity file is a convenience, not part of registering: it lets the
        // drive be recognised automatically next time, and the drive is perfectly
        // usable without it. Failing the whole registration when it cannot be
        // written was a real defect — the drive was already recorded by the call
        // above, so the owner was told registration failed when it had succeeded,
        // and retrying then complained the number was in use.
        //
        // Read-only drives are the common case here, not an edge one: macOS mounts
        // NTFS read-only, so every Windows-formatted disk lands on this path.
        let mut note = None;
        if write_manifest {
            let m = DriveManifest::new(&drive.id, drive.drive_number, name);
            match m.write_to_volume(&vol) {
                Ok(_) => {
                    let _ = repo.audit(&drive.id, "manifest_written", None);
                }
                Err(e) => {
                    let read_only = e.to_string().contains("Read-only")
                        || e.to_string().contains("os error 30");
                    note = Some(if read_only {
                        "Registered. This drive is read-only, so the identity file was not saved \
                         — AtlasDrive will recognise it by name and contents instead. Nothing else \
                         changes."
                            .to_string()
                    } else {
                        format!(
                            "Registered, but the identity file could not be saved ({e}). \
                             AtlasDrive will recognise this drive by name and contents instead."
                        )
                    });
                    let _ = repo.audit(&drive.id, "manifest_skipped", None);
                }
            }
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
            note,
        })
    })
    .await
    .map_err(|e| e.to_string())?
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
    /// How many photographs match in total, when that can be counted exactly.
    /// `None` for free-text searches, where no total is claimed.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_matches: Option<i64>,
}

/// Tag a face group with a person's name.
///
/// This is the only way a name is ever attached to a face. Confirming promotes
/// the group's faces to exemplars, so the person is recognised on later scans.
#[tauri::command]
async fn tag_face_cluster(
    state: State<'_, AppState>,
    cluster_id: String,
    name: String,
) -> Result<faces::Person, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<faces::Person, String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive)
            .tag_cluster_with_name(&cluster_id, &name)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Faces to browse: one per group, biggest groups first. No names required.
///
/// Off the main thread: on an archive with a hundred thousand faces the query
/// is long enough to be felt, and a synchronous command freezes the window.
#[tauri::command]
async fn face_gallery(
    state: State<'_, AppState>,
    limit: Option<usize>,
    drive_number: Option<i64>,
) -> Result<Vec<faces::GalleryFace>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive)
            .gallery_on_drive(limit.unwrap_or(200), drive_number)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Photographs that exist on only one drive, per drive and folder.
#[tauri::command]
async fn single_copies(
    state: State<'_, AppState>,
) -> Result<Vec<family_archive_core::copies::DriveCopies>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let archive = open_archive(&paths)?;
        family_archive_core::copies::single_copies(&archive).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Plugged-in drives with photographs not read back for six months, and how
/// many — so AtlasDrive can offer to check them for silent damage.
#[tauri::command]
async fn health_due(state: State<'_, AppState>) -> Result<Vec<(i64, i64)>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<(i64, i64)>, String> {
        let archive = open_archive(&paths)?;
        let mut out = Vec::new();
        for d in DriveRepo::new(&archive).list().map_err(map_err)? {
            if d.status != "online" {
                continue;
            }
            let due = family_archive_core::bitrot::due_for_check(
                &archive,
                d.drive_number,
                family_archive_core::bitrot::RECHECK_AFTER_DAYS,
            )
            .map_err(map_err)?;
            if due > 0 {
                out.push((d.drive_number, due));
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read back a drive's least-recently-checked photographs and compare them
/// with what was scanned. Read-only; says what it found in words.
#[tauri::command]
async fn spot_check_drive(state: State<'_, AppState>, drive_number: i64) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let archive = open_archive(&paths)?;
        let report = family_archive_core::bitrot::verify_drive(
            &archive,
            drive_number,
            &family_archive_core::bitrot::VerifyOptions {
                limit: Some(family_archive_core::bitrot::SPOT_CHECK_FILES),
                ..Default::default()
            },
            |_, _| {},
        )
        .map_err(map_err)?;
        family_archive_core::bitrot::describe(&archive, &report).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// How many faces nobody has named, per drive — counted, not sampled.
#[tauri::command]
async fn unnamed_face_counts(
    state: State<'_, AppState>,
) -> Result<Vec<faces::UnnamedOnDrive>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive).unnamed_counts().map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Group every drive's ungrouped faces with their look-alikes (D-089).
///
/// For an archive indexed before scans did this themselves. Drive by drive, so
/// the work stays proportionate; nothing already grouped or named moves.
#[tauri::command]
async fn group_faces(state: State<'_, AppState>) -> Result<faces::GroupingReport, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let archive = open_archive(&paths)?;
        let key = keystore::default_keystore(paths.keys_dir())
            .get_or_create()
            .map_err(map_err)?;
        let repo = faces::FaceRepo::new(&archive);
        let mut total = faces::GroupingReport::default();
        for drive in DriveRepo::new(&archive).list().map_err(map_err)? {
            let r = repo.group_ungrouped(Some(&drive.id), &key).map_err(map_err)?;
            total.faces_considered += r.faces_considered;
            total.groups_created += r.groups_created;
            total.faces_grouped += r.faces_grouped;
        }
        total.groups_merged = repo.merge_lookalike_groups(&key).map_err(map_err)?.groups_merged;
        Ok(total)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One face crop, as a data URL the webview can render directly.
///
/// The crop is decrypted here and never written to disk in the clear; the CSP
/// permits `data:` images, so nothing needs to be served from a file path.
#[tauri::command]
async fn face_thumbnail(state: State<'_, AppState>, face_id: String) -> Result<Option<String>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
        let archive = open_archive(&paths)?;
        let key = keystore::default_keystore(paths.keys_dir())
            .get_or_create()
            .map_err(map_err)?;
        let crop = faces::FaceRepo::new(&archive)
            .thumbnail(&face_id, &key)
            .map_err(map_err)?;
        Ok(crop.map(|(bytes, format)| format!("data:image/{format};base64,{}", b64(&bytes))))
    })
    .await
    .map_err(|e| e.to_string())?
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
async fn tag_face(state: State<'_, AppState>, face_id: String, name: String) -> Result<TagResult, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<TagResult, String> {
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
    })
    .await
    .map_err(|e| e.to_string())?
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
async fn pending_suggestions(
    state: State<'_, AppState>,
    person_id: String,
    limit: Option<usize>,
) -> Result<Vec<faces::SuggestedFace>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<faces::SuggestedFace>, String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive)
            .pending_suggestions(&person_id, limit.unwrap_or(200))
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Accept every outstanding proposal for a person.
#[tauri::command]
async fn confirm_suggestions(state: State<'_, AppState>, person_id: String) -> Result<usize, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive)
            .confirm_suggestions(&person_id)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Reject every outstanding proposal for a person, freeing those faces.
#[tauri::command]
async fn reject_suggestions(state: State<'_, AppState>, person_id: String) -> Result<usize, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive)
            .reject_suggestions(&person_id)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Say yes or no to one proposed group.
#[tauri::command]
async fn resolve_suggestion(
    state: State<'_, AppState>,
    cluster_id: String,
    is_them: bool,
) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let archive = open_archive(&paths)?;
        let repo = faces::FaceRepo::new(&archive);
        if is_them {
            repo.confirm_cluster_suggestion(&cluster_id).map_err(map_err)
        } else {
            repo.reject_cluster_suggestion(&cluster_id).map_err(map_err)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Every photograph containing a named person, and which drive holds it.
#[tauri::command]
async fn photos_of_person(
    state: State<'_, AppState>,
    person_id: String,
) -> Result<Vec<faces::PersonPhoto>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<faces::PersonPhoto>, String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive)
            .photos_of_person(&person_id)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Copy a person's photographs into a folder the user chose.
///
/// Reads originals and writes only into `destination`. Never moves, never
/// deletes, never writes to the source drive.
#[tauri::command]
async fn copy_person_photos(
    state: State<'_, AppState>,
    person_id: String,
    destination: String,
) -> Result<family_archive_core::export::ExportSummary, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<family_archive_core::export::ExportSummary, String> {
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
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Write XMP sidecars next to a person's originals, for Bridge and Lightroom.
///
/// **Writes to the source drive.** Never called automatically — the interface
/// asks first, exactly as it does before writing a drive manifest.
#[tauri::command]
async fn write_sidecars_for_person(
    state: State<'_, AppState>,
    person_id: String,
) -> Result<family_archive_core::export::SidecarSummary, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<family_archive_core::export::SidecarSummary, String> {
        let archive = open_archive(&paths)?;
        let ids: Vec<String> = faces::FaceRepo::new(&archive)
            .photos_of_person(&person_id)
            .map_err(map_err)?
            .into_iter()
            .map(|p| p.file_id)
            .collect();
        family_archive_core::export::write_xmp_sidecars(&archive, &ids).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
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
async fn photo_thumbnail(
    state: State<'_, AppState>,
    file_id: String,
    max_edge: Option<u32>,
) -> Result<Option<String>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
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
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Where a person's photographs live, grouped by folder.
#[tauri::command]
async fn person_folders(
    state: State<'_, AppState>,
    person_id: String,
) -> Result<Vec<faces::PersonFolder>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<faces::PersonFolder>, String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive)
            .folders_for_person(&person_id)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open a folder in Finder. Read-only: it shows a window, nothing more.
///
/// The path must be a folder the catalogue actually knows about — one that
/// contains an indexed photograph. Without that check this command would open
/// *any* directory on the machine on request, which is more authority than a
/// photo catalogue needs. The webview only ever loads local bundled assets, so
/// there is no known way to reach it with a hostile path today; this is here so
/// that stays true if the interface ever renders anything less trusted.
#[tauri::command]
fn open_folder(state: State<AppState>, path: String) -> Result<(), String> {
    let dir = std::path::Path::new(&path);
    if !dir.is_dir() {
        return Err("That folder is not available — connect the drive and try again.".into());
    }

    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let canonical = dir
        .canonicalize()
        .map_err(|_| "That folder is not available.".to_string())?;

    // Resolve each scan root once and check the folder sits inside one.
    let mut roots = archive
        .prepare("SELECT DISTINCT scan_root FROM scan_runs WHERE mode <> 'dry-run'")
        .map_err(map_err)?;
    let known = roots
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(map_err)?
        .filter_map(|r| r.ok())
        .filter_map(|root| std::path::Path::new(&root).canonicalize().ok())
        .any(|root| canonical.starts_with(&root));

    if !known {
        return Err("AtlasDrive only opens folders it has indexed.".into());
    }

    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&canonical)
        .spawn()
        .map_err(|e| format!("could not open Finder: {e}"))?;
    Ok(())
}

/// Remove a person added by mistake. Their faces are kept and become unnamed.
#[tauri::command]
async fn forget_person(state: State<'_, AppState>, person_id: String) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive)
            .remove_person(&person_id)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Correct a person's name, merging into an existing person on a name clash.
#[tauri::command]
async fn rename_person(
    state: State<'_, AppState>,
    person_id: String,
    name: String,
) -> Result<faces::Person, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<faces::Person, String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive)
            .rename_person(&person_id, &name)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Re-scan a drive for photographs added since the last scan.
///
/// Scans the **whole drive** when it is connected, not the folder it was last
/// scanned from. Unchanged photographs are skipped, so this is cheap.
///
/// The order used to be the other way round, and it quietly hid most of a
/// drive. Drive 1 had been registered by pointing at a single wedding folder,
/// so every later "check for new photographs" re-examined those 758 files,
/// found nothing, and stopped — while thirty other shoots on the same disk had
/// never been looked at once. The screen then reported "all 758 photographs
/// indexed, safe to unplug", which was true of the folder and false of the
/// drive.
///
/// A button on a drive means the drive.
#[tauri::command]
fn rescan_drive(state: State<AppState>, drive_number: i64) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    let archive = open_archive(&paths)?;
    let last_scan_root: Option<String> = archive
        .query_row(
            "SELECT sr.scan_root FROM scan_runs sr
               JOIN drives d ON d.id = sr.drive_id
              WHERE d.drive_number = ?1 AND sr.mode <> 'dry-run'
              ORDER BY sr.started_at DESC LIMIT 1",
            [drive_number],
            |r| r.get(0),
        )
        .ok();
    // The mounted volume first: it is the only one of these that means "the
    // drive". The registered folder and the last-scanned folder are both
    // whatever the owner happened to pick once, and neither is a reason to
    // ignore the rest of the disk. They remain as fallbacks for a drive that
    // is not currently mounted where AtlasDrive can see it.
    let root = match family_archive_core::volumes::mount_point_for_drive(&archive, drive_number)
        .or_else(|| DriveRepo::new(&archive).registered_root(drive_number))
        .or(last_scan_root)
    {
        Some(r) => r,
        None => {
            return Err(format!(
                "Connect Drive {drive_number} and try again — AtlasDrive cannot find it, \
                 so there is nothing to scan."
            ))
        }
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

/// Mark how a person relates to the owner — "family" being the one that matters.
#[tauri::command]
async fn set_person_relationship(
    state: State<'_, AppState>,
    person_id: String,
    relationship: Option<String>,
) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive)
            .set_relationship(&person_id, relationship.as_deref())
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Every relationship in use, with how many people carry it.
#[tauri::command]
async fn person_relationships(state: State<'_, AppState>) -> Result<Vec<(String, i64)>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<(String, i64)>, String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive).relationships().map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Everyone the user has named, and how established each is.
#[tauri::command]
async fn list_people(state: State<'_, AppState>) -> Result<Vec<faces::NamedPerson>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<faces::NamedPerson>, String> {
        let archive = open_archive(&paths)?;
        faces::FaceRepo::new(&archive).people().map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Mark a group as not a person at all (a false detection).
#[tauri::command]
async fn reject_face_cluster(state: State<'_, AppState>, cluster_id: String) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let archive = open_archive(&paths)?;
        archive
            .execute(
                "UPDATE face_clusters SET status='rejected', updated_at=?2 WHERE id=?1",
                rusqlite::params![cluster_id, family_archive_core::util::now_iso8601()],
            )
            .map_err(map_err)?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Rename a drive, keeping its number and everything indexed from it.
#[tauri::command]
async fn rename_drive(
    state: State<'_, AppState>,
    drive_number: i64,
    name: String,
) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let archive = open_archive(&paths)?;
        let repo = DriveRepo::new(&archive);
        let drive = repo
            .get_by_number(drive_number)
            .map_err(map_err)?
            .ok_or_else(|| format!("Drive {drive_number} is not registered."))?;
        repo.rename(&drive.id, &name).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Every subject the catalogue recognised, for browsing rather than guessing.
///
/// `useful` leaves out subjects on so many photographs that picking them
/// narrows nothing ("people", "adult"); see `inventory::useful_subjects`.
#[tauri::command]
async fn catalogue_tags(
    state: State<'_, AppState>,
    limit: Option<usize>,
    drive_number: Option<i64>,
    useful: Option<bool>,
) -> Result<Vec<family_archive_core::inventory::TagCount>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let archive = open_archive(&paths)?;
        let limit = limit.unwrap_or(60);
        if useful.unwrap_or(false) {
            family_archive_core::inventory::useful_subjects(&archive, limit, drive_number)
        } else {
            family_archive_core::inventory::tags_on_drive(&archive, limit, drive_number)
        }
        .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What is stored on each drive — answerable with every drive unplugged.
#[tauri::command]
async fn drive_contents(
    state: State<'_, AppState>,
    drive_number: Option<i64>,
) -> Result<Vec<family_archive_core::inventory::DriveContents>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<family_archive_core::inventory::DriveContents>, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::inventory::drive_contents(&archive, drive_number).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Photographs that look like this one.
///
/// This is what the vector index is for. Text queries deliberately do not use
/// it: Apple Vision has no text encoder, so a typed query cannot be placed in
/// the same space as the photographs, and the only local text encoder available
/// matches on colour rather than meaning. Text search goes through Vision's
/// classification labels and OCR instead. See D-040.
#[tauri::command]
async fn similar_photographs(
    state: State<'_, AppState>,
    file_id: String,
    limit: Option<usize>,
) -> Result<Vec<family_archive_core::search::SearchResult>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<family_archive_core::search::SearchResult>, String> {
        let archive = open_archive(&paths)?;
        let repo = family_archive_core::search::SearchRepo::with_index_dir(&archive, paths.cache_dir());
        let similar = repo
            .similar_to(
                &file_id,
                &SearchFilters {
                    limit: limit.unwrap_or(24),
                    include_offline: true,
                    ..Default::default()
                },
            )
            .map_err(map_err)?;
        family_archive_core::search::fold_copies(&archive, similar).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn search_catalogue(
    state: State<'_, AppState>,
    query: String,
    drive: Option<i64>,
    include_offline: bool,
    // Restrict to one event, or to every shoot for one client.
    event_id: Option<String>,
    client: Option<String>,
    // Subjects every result must carry. Each one narrows the search.
    tags: Option<Vec<String>>,
    // How many results to return. The screen raises this when the owner asks
    // to see the rest.
    limit: Option<usize>,
) -> Result<SearchResponse, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<SearchResponse, String> {
        let archive = open_archive(&paths)?;
        // The index lives in the cache directory: it is derived entirely from the
        // catalogue, so losing it costs a rebuild and nothing else.
        let repo = family_archive_core::search::SearchRepo::with_index_dir(
            &archive,
            paths.cache_dir(),
        );
        let filters = SearchFilters {
            drive_number: drive,
            online_only: !include_offline,
            include_offline,
            event_id,
            client,
            tags: tags.unwrap_or_default(),
            limit: limit.unwrap_or(100).clamp(1, 5000),
            ..Default::default()
        };

        // An empty box with subjects picked is a browse, not a search: the tag
        // rows answer it exactly, and routing it through free text told the owner
        // a subject with 995 photographs had none.
        let browsing = query.trim().is_empty() && !filters.tags.is_empty();

        let (mut results, text_only, understood) = if browsing {
            (repo.browse_by_tags(&filters).map_err(map_err)?, false, Vec::new())
        } else {
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
            let results = repo
                .natural_language_search(&query, visual, &filters)
                .map_err(map_err)?;
            (results, text_only, understood)
        };
        // A photograph on several drives is one result that names them all.
        results = family_archive_core::search::fold_copies(&archive, results).map_err(map_err)?;
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

        // Only browsing can state an exact total cheaply; a fused text/visual
        // search does not, and claiming one would be a guess dressed as a fact.
        let total_matches =
            if browsing { repo.count_by_tags(&filters).ok() } else { None };

        Ok(SearchResponse { results, understood, text_only, drives, where_to_look, total_matches })
    })
    .await
    .map_err(|e| e.to_string())?
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
    // A new run clears the last failure: what is on screen should describe
    // this attempt, not the previous one. It also withdraws any outstanding
    // stop request, so a Stop pressed yesterday cannot kill today's scan.
    *state.last_error.lock().unwrap() = None;
    let _ = family_archive_core::stop::clear(&paths);
    let cancel = state.running.lock().unwrap().clone().unwrap();
    let running_slot = state.running.clone();
    let error_slot = state.last_error.clone();

    std::thread::spawn(move || {
        // The whole run is held inside catch_unwind. A panic anywhere in the
        // pipeline used to unwind straight past the cleanup at the bottom of
        // this closure, leaving `running` set forever: the interface showed a
        // scan that no longer existed, Stop waited politely on a dead thread,
        // and no new scan could start — "an index run is already in progress",
        // said an app whose scan thread had been gone for two days. A crash
        // must land in the same place as an error: recorded, visible, cleared.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
            // Held for the whole run. A drive is plugged in and left for a
            // night or two; if the Mac sleeps, indexing stops and the morning
            // shows a part-indexed drive with no clue why. Dropped
            // automatically when this closure ends, however it ends.
            let _awake = family_archive_core::awake::StayAwake::hold(
                format!("indexing drive {drive}"),
            );
            eprintln!("{}", _awake.describe());

            let mut opts = IndexOptions::new(drive, path);
            opts.mode = if dry_run { IndexMode::DryRun } else { IndexMode::Normal };
            opts.resume = resume;
            opts.config = Config::default();
            pipeline.run(&opts).map_err(map_err)?;
            Ok(())
        })();
        // A successful run is the moment there is new work worth protecting, so
        // it is the right moment to back up — if the owner asked for that. A
        // failed or cancelled run is not, because the catalogue may be
        // mid-batch. Dry runs change nothing and are skipped too.
        if result.is_ok() && !dry_run {
            let prefs = family_archive_core::settings::load(&paths);
            if let (true, Some(dest)) = (prefs.backup_after_indexing, prefs.backup_destination.clone()) {
                let options = family_archive_core::backup::BackupOptions {
                    include_key: prefs.backup_include_key,
                    include_thumbnails: true,
                    keep: prefs.backup_keep,
                };
                match family_archive_core::backup::create(
                    &paths,
                    std::path::Path::new(&dest),
                    &options,
                ) {
                    Ok(report) => {
                        let mut prefs = prefs;
                        prefs.last_backup_at = Some(family_archive_core::util::now_iso8601());
                        let _ = family_archive_core::settings::save(&paths, &prefs);
                        eprintln!("automatic backup written to {}", report.bundle);
                    }
                    // A failed backup must not be reported as a failed index
                    // run: the indexing succeeded and the catalogue is intact.
                    Err(e) => eprintln!("automatic backup failed: {e}"),
                }
            }
        }

        result
        }));

        let outcome = match caught {
            Ok(r) => r,
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown internal error".into());
                Err(format!("the scan crashed: {msg}"))
            }
        };
        if let Err(e) = outcome {
            // Recorded where the interface can find it. A run that fails
            // before it writes any progress — a folder that vanished, a drive
            // unplugged between the click and the walk — leaves no other trace
            // at all, which is exactly the case that went unnoticed.
            eprintln!("index run failed: {e}");
            *error_slot.lock().unwrap() = Some(e);
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
    let mut progress = Progress::load(&paths).map_err(map_err)?;

    // A run that was killed rather than cancelled leaves "running" on disk,
    // because nothing got the chance to write anything else. Reconciling it
    // against whether a run is actually in flight is the difference between a
    // dashboard that reports the world and one that reports a stale file: the
    // owner would otherwise open the app to a live pulse, a read speed and
    // "Reading photographs from this drive" with nothing running at all.
    //
    // The rule itself lives in the core `Progress`, tested there, because the
    // verifier and the command line have to reach the same verdict about the
    // same file — this app used to be the only thing that knew (D-085).
    if let Some(p) = progress.as_mut() {
        let in_flight = state.running.lock().unwrap().is_some();
        p.status = p.reconciled_status(Some(in_flight));
    }
    Ok(progress)
}

/// Runs off the main thread. A synchronous command runs on it, and this one
/// reads every thumbnail in the archive — on 218,000 photographs that froze the
/// whole window for minutes, which is how opening Settings "hung" the app.
#[tauri::command]
async fn run_verifier(state: State<'_, AppState>) -> Result<Vec<Check>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || verify_catalogue(&paths))
        .await
        .map_err(|e| e.to_string())?
}

fn verify_catalogue(paths: &AppPaths) -> Result<Vec<Check>, String> {
    let paths = paths.clone();
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
        observed_throughput: None,
        network_blocked_attempts: 0,
    };
    let report = verifier::run(&ctx).map_err(map_err)?;
    Ok(report.checks)
}

#[tauri::command]
async fn prepare_review(
    state: State<'_, AppState>,
    limit: usize,
) -> Result<Vec<family_archive_core::faces::ClusterSummary>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<family_archive_core::faces::ClusterSummary>, String> {
        let archive = open_archive(&paths)?;
        let repo = faces::FaceRepo::new(&archive);
        repo.prepare_review(limit).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Record the user's own correction to a photograph's date.
///
/// Returns the phrasing to show, e.g. "Taken on 1998-08-12". The correction
/// outranks the estimator and survives re-analysis.
#[tauri::command]
async fn set_date_override(
    state: State<'_, AppState>,
    file_id: String,
    earliest: String,
    latest: Option<String>,
) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let archive = open_archive(&paths)?;
        let repo = family_archive_core::dates::DateRepo::new(&archive);
        let latest = latest.unwrap_or_else(|| earliest.clone());
        let est = repo
            .set_user_override(&file_id, &earliest, &latest)
            .map_err(map_err)?;
        Ok(family_archive_core::dates::describe(&est))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Remove a correction, letting AtlasDrive's own estimate apply again.
#[tauri::command]
async fn clear_date_override(state: State<'_, AppState>, file_id: String) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let archive = open_archive(&paths)?;
        family_archive_core::dates::DateRepo::new(&archive)
            .clear_user_override(&file_id)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Record where a drive physically lives and how it is categorised.
///
/// Both fields are optional and independent: omitting one leaves it alone,
/// rather than blanking it.
#[tauri::command]
async fn update_drive_details(
    state: State<'_, AppState>,
    drive_number: i64,
    physical_location: Option<String>,
    categories: Option<Vec<String>>,
) -> Result<DriveDto, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<DriveDto, String> {
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
            note: None,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Show an indexed original in Finder, when its drive is connected.
///
/// Read-only by construction: `open -R` selects the file in a Finder window and
/// cannot alter it. When the drive is not connected this returns a plain-language
/// message rather than an error string, because a disconnected drive is a normal
/// state in this product, not a fault.
#[tauri::command]
async fn reveal_in_finder(state: State<'_, AppState>, file_id: String) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
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
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Where Lightroom Classic lives when it is installed.
const LIGHTROOM_CLASSIC: &str = "/Applications/Adobe Lightroom Classic/Adobe Lightroom Classic.app";

/// Whether Lightroom Classic is installed, so the option is only offered when
/// it can work.
#[tauri::command]
fn lightroom_available() -> bool {
    std::path::Path::new(LIGHTROOM_CLASSIC).exists()
}

/// Open an original in its default app, or in Lightroom Classic.
///
/// Read-only from AtlasDrive's side: the file is handed to the other app by
/// path and nothing here writes to it. Lightroom decides what to do with it —
/// normally its own import dialog — and AtlasDrive never touches Lightroom's
/// catalogue.
#[tauri::command]
async fn open_original(
    state: State<'_, AppState>,
    file_id: String,
    app: Option<String>,
) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let archive = open_archive(&paths)?;
        let drive_number: Option<i64> = archive
            .query_row(
                "SELECT d.drive_number FROM files f JOIN drives d ON d.id=f.drive_id WHERE f.id=?1",
                [&file_id],
                |r| r.get(0),
            )
            .ok();
        let Some(path) =
            family_archive_core::search::resolve_original(&archive, &file_id).map_err(map_err)?
        else {
            return Ok(match drive_number {
                Some(n) => format!("Plug in Drive {n} to open the original."),
                None => "That photograph is no longer in the catalogue.".to_string(),
            });
        };
        let lightroom = app.as_deref() == Some("lightroom");
        #[cfg(target_os = "macos")]
        {
            let mut cmd = std::process::Command::new("open");
            if lightroom {
                cmd.arg("-a").arg(LIGHTROOM_CLASSIC);
            }
            cmd.arg(&path)
                .spawn()
                .map_err(|e| format!("could not open the photograph: {e}"))?;
        }
        Ok(if lightroom {
            format!("Opened {} in Lightroom Classic.", path.display())
        } else {
            format!("Opened {}.", path.display())
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Write a privacy-redacted diagnostics bundle and return its path.
///
/// There is no unredacted variant: the export is built from counts and check
/// outcomes, so the user never has to audit it before sharing it.
#[tauri::command]
async fn export_diagnostics(state: State<'_, AppState>) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let archive = open_archive(&paths)?;
        let queue = open_queue(&paths)?;
        let diag =
            family_archive_core::diagnostics::collect(&archive, Some(&queue), &paths, None)
                .map_err(map_err)?;
        let path = family_archive_core::diagnostics::write(&paths, &diag).map_err(map_err)?;
        Ok(path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Off the main thread, like [`run_verifier`]: an integrity check of the whole
/// catalogue and the Vision worker's selftest can each take many seconds.
#[tauri::command]
async fn doctor(
    state: State<'_, AppState>,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || doctor_report(paths))
        .await
        .map_err(|e| e.to_string())?
}

fn doctor_report(paths: AppPaths) -> Result<std::collections::BTreeMap<String, String>, String> {
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

/// Drives plugged in right now, so one can be picked rather than typed.
#[tauri::command]
async fn connected_volumes(
    state: State<'_, AppState>,
) -> Result<Vec<family_archive_core::volumes::Volume>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<family_archive_core::volumes::Volume>, String> {
        // The catalogue is only needed to say which volumes are already registered;
        // the picker must still work before one exists.
        let archive = open_archive(&paths).ok();
        family_archive_core::volumes::connected(archive.as_ref()).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Folders on a volume where photographs usually live.
#[tauri::command]
fn likely_photo_folders(path: String) -> Vec<String> {
    family_archive_core::volumes::likely_photo_folders(std::path::Path::new(&path))
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Coverage and estimates
// ---------------------------------------------------------------------------

/// What a scan has produced so far, for the live dashboard.
///
/// Counted from the catalogue each time rather than tracked in memory, so the
/// figures survive the app being closed mid-run and cannot drift from what was
/// actually written.
#[tauri::command]
async fn scan_stats(
    state: State<'_, AppState>,
    drive_number: i64,
    recent: Option<usize>,
) -> Result<family_archive_core::inventory::ScanStats, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<family_archive_core::inventory::ScanStats, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::inventory::scan_stats(&archive, drive_number, recent.unwrap_or(12))
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Look for names printed on things in the photographs, from their text.
///
/// A backfill rather than a rescan: no original is opened, so it works with
/// every drive unplugged.
#[tauri::command]
async fn find_names(
    state: State<'_, AppState>,
    drive_number: Option<i64>,
) -> Result<family_archive_core::inventory::NameScan, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<family_archive_core::inventory::NameScan, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::inventory::scan_for_names(&archive, drive_number).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Ask whichever process is scanning to stop at the next batch boundary.
///
/// Deliberately not limited to runs this app started. A scan may have been
/// launched from the command line and left running for days; Stop has to stop
/// the scan that is actually running, not the one this process knows about.
/// Nothing is lost — stopping between batches is exactly what unplugging the
/// drive does, and the queue is durable.
#[tauri::command]
fn stop_scan(state: State<AppState>) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    // Nothing running means nothing to stop — and no flag left on disk to
    // ambush the next run.
    if state.running.lock().unwrap().is_none() {
        let _ = family_archive_core::stop::clear(&paths);
        return Ok("Nothing is scanning right now.".to_string());
    }
    // Cancel our own run too, so a scan started here stops without waiting for
    // the flag to be noticed.
    if let Some(token) = state.running.lock().unwrap().as_ref() {
        token.cancel();
    }
    family_archive_core::stop::request(&paths).map_err(map_err)?;
    Ok("Stopping after the current batch. Nothing is lost — the drive can be \
        started again, or a different one scanned, whenever you like."
        .to_string())
}

/// True when a stop has been asked for and the scan has not yet noticed.
#[tauri::command]
fn stop_pending(state: State<AppState>) -> bool {
    // Pending means a running scan has been asked and has not yet obeyed.
    // Reporting the bare flag showed "Stopping..." forever when the scan it
    // was addressed to no longer existed.
    let paths = state.paths.lock().unwrap().clone();
    state.running.lock().unwrap().is_some() && family_archive_core::stop::requested(&paths)
}

/// Why the last background scan stopped, if it stopped badly.
///
/// `None` while a run is healthy or has never failed.
#[tauri::command]
fn last_scan_error(state: State<AppState>) -> Option<String> {
    state.last_error.lock().unwrap().clone()
}

/// What each folder on a drive appears to contain, in plain language.
/// Works with the drive disconnected — it reads only the catalogue.
#[tauri::command]
async fn folder_summaries(
    state: State<'_, AppState>,
    drive_number: i64,
) -> Result<Vec<family_archive_core::foldersum::FolderSummary>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<family_archive_core::foldersum::FolderSummary>, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::foldersum::folder_summaries(&archive, drive_number).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open a shoot folder in Finder, when its drive is connected.
#[tauri::command]
async fn reveal_folder(
    state: State<'_, AppState>,
    drive_number: i64,
    folder: String,
    example_path: String,
) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let archive = open_archive(&paths)?;
        match family_archive_core::foldersum::folder_abs_path(
            &archive,
            drive_number,
            &example_path,
            &folder,
        )
        .map_err(map_err)?
        {
            Some(dir) => {
                #[cfg(target_os = "macos")]
                {
                    std::process::Command::new("open")
                        .arg(&dir)
                        .spawn()
                        .map_err(|e| format!("could not open Finder: {e}"))?;
                }
                Ok(format!("Opened {} in Finder.", dir.display()))
            }
            None => Ok(format!(
                "Connect Drive {drive_number} to open this folder — it is not plugged in right now."
            )),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Why files on this drive were given up on, most common reason first.
#[tauri::command]
async fn scan_failures(
    state: State<'_, AppState>,
    drive_number: i64,
) -> Result<Vec<family_archive_core::queue::FailureReason>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<family_archive_core::queue::FailureReason>, String> {
        let archive = open_archive(&paths)?;
        let drive_id: String = archive
            .query_row("SELECT id FROM drives WHERE drive_number=?1", [drive_number], |r| r.get(0))
            .map_err(|_| format!("no drive numbered {drive_number}"))?;
        let queue = open_queue(&paths)?;
        family_archive_core::queue::Queue::new(&queue)
            .failure_reasons(&drive_id)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Put files that were given up on back in the queue.
///
/// Needed whenever AtlasDrive learns to read something it previously could
/// not: without it those photographs stay out of the catalogue permanently,
/// because an item that failed three times is never leased again.
#[tauri::command]
async fn retry_failed_files(
    state: State<'_, AppState>,
    drive_number: i64,
    code: Option<String>,
) -> Result<usize, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        let archive = open_archive(&paths)?;
        let drive_id: String = archive
            .query_row("SELECT id FROM drives WHERE drive_number=?1", [drive_number], |r| r.get(0))
            .map_err(|_| format!("no drive numbered {drive_number}"))?;
        let queue = open_queue(&paths)?;
        family_archive_core::queue::Queue::new(&queue)
            .retry_failed(&drive_id, code.as_deref())
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// How completely each drive has been indexed, least complete first.
#[tauri::command]
async fn drive_coverage(
    state: State<'_, AppState>,
) -> Result<Vec<family_archive_core::inventory::DriveCoverage>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<family_archive_core::inventory::DriveCoverage>, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::inventory::drive_coverage(&archive).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// How long indexing a folder is likely to take, before a night is committed
/// to it. Counts the files by walking the tree, which is fast — nothing is
/// decoded or analysed.
#[tauri::command]
async fn estimate_index(
    state: State<'_, AppState>,
    path: String,
) -> Result<family_archive_core::inventory::IndexEstimate, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<family_archive_core::inventory::IndexEstimate, String> {
        use family_archive_core::scan::{enumerate, ScanOptions};
        let archive = open_archive(&paths)?;
        let found = enumerate(std::path::Path::new(&path), &ScanOptions::default())
            .map_err(map_err)?;
        Ok(family_archive_core::inventory::estimate_indexing(&archive, found.len() as u64))
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[tauri::command]
async fn propose_events(
    state: State<'_, AppState>,
    gap_hours: Option<f64>,
) -> Result<family_archive_core::events::ProposeReport, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<family_archive_core::events::ProposeReport, String> {
        use family_archive_core::events::{EventRepo, DEFAULT_GAP_HOURS};
        let archive = open_archive(&paths)?;
        EventRepo::new(&archive)
            .propose(gap_hours.unwrap_or(DEFAULT_GAP_HOURS))
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_events(
    state: State<'_, AppState>,
    status: Option<String>,
) -> Result<Vec<family_archive_core::events::Event>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<family_archive_core::events::Event>, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::events::EventRepo::new(&archive)
            .list(status.as_deref())
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The next event awaiting a decision, so the interface can review one at a
/// time rather than presenting a wall of proposals.
#[tauri::command]
async fn next_event_proposal(
    state: State<'_, AppState>,
    skip: Option<Vec<String>>,
) -> Result<Option<family_archive_core::events::Event>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Option<family_archive_core::events::Event>, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::events::EventRepo::new(&archive)
            .next_proposal_skipping(&skip.unwrap_or_default())
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A name to offer for an event, from who is in it and where it lives.
#[tauri::command]
async fn suggest_event_name(
    state: State<'_, AppState>,
    event_id: String,
) -> Result<family_archive_core::events::NameSuggestion, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<family_archive_core::events::NameSuggestion, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::events::EventRepo::new(&archive)
            .suggest_name(&event_id)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn name_event(
    state: State<'_, AppState>,
    event_id: String,
    name: String,
    client: Option<String>,
) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let archive = open_archive(&paths)?;
        family_archive_core::events::EventRepo::new(&archive)
            .name_event(&event_id, &name, client.as_deref())
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn forget_event(state: State<'_, AppState>, event_id: String) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let archive = open_archive(&paths)?;
        family_archive_core::events::EventRepo::new(&archive)
            .forget(&event_id)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn merge_events(state: State<'_, AppState>, into: String, from: String) -> Result<u64, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<u64, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::events::EventRepo::new(&archive)
            .merge(&into, &from)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn split_event(state: State<'_, AppState>, event_id: String, at: String) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::events::EventRepo::new(&archive)
            .split(&event_id, &at)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Where an event could sensibly be divided, largest pause first.
#[tauri::command]
async fn event_split_points(
    state: State<'_, AppState>,
    event_id: String,
    limit: Option<usize>,
) -> Result<Vec<family_archive_core::events::SplitPoint>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<family_archive_core::events::SplitPoint>, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::events::EventRepo::new(&archive)
            .split_points(&event_id, limit.unwrap_or(3))
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn event_clients(state: State<'_, AppState>) -> Result<Vec<(String, i64)>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<(String, i64)>, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::events::EventRepo::new(&archive)
            .clients()
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn event_files(state: State<'_, AppState>, event_id: String) -> Result<Vec<String>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<String>, String> {
        let archive = open_archive(&paths)?;
        family_archive_core::events::EventRepo::new(&archive)
            .files(&event_id)
            .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------
// Backup
// ---------------------------------------------------------------------------

/// Show a native folder chooser and return what was picked.
///
/// Done in Rust with the system dialog rather than by granting the webview a
/// dialog plugin, so `capabilities/default.json` stays empty of filesystem and
/// shell permissions — the webview asks a command, exactly as it does for
/// everything else privileged.
#[tauri::command]
fn choose_folder(prompt: Option<String>) -> Result<Option<String>, String> {
    let prompt = prompt.unwrap_or_else(|| "Choose a folder".to_string());
    // Quotes inside the prompt would break out of the AppleScript string.
    let safe: String = prompt.chars().filter(|c| *c != '"' && *c != '\\').collect();
    let script = format!(
        "try\n  POSIX path of (choose folder with prompt \"{safe}\")\non error number -128\n  return \"\"\nend try"
    );
    let out = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(map_err)?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let picked = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // Cancelling is a normal outcome, not an error.
    Ok(if picked.is_empty() { None } else { Some(picked) })
}

#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> Result<family_archive_core::settings::Settings, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<family_archive_core::settings::Settings, String> {
        Ok(family_archive_core::settings::load(&paths))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn save_settings(
    state: State<'_, AppState>,
    settings: family_archive_core::settings::Settings,
) -> Result<(), String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        family_archive_core::settings::save(&paths, &settings).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Which cloud service, if any, appears to synchronise a folder. Advisory:
/// the user is told whether a backup will leave this Mac, never prevented.
#[tauri::command]
fn describe_backup_destination(path: String) -> Option<String> {
    family_archive_core::settings::is_cloud_synced(std::path::Path::new(&path))
        .map(|s| s.to_string())
}

#[tauri::command]
async fn backup_now(
    state: State<'_, AppState>,
    destination: Option<String>,
) -> Result<family_archive_core::backup::BackupReport, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<family_archive_core::backup::BackupReport, String> {
        use family_archive_core::{backup, settings};
        let mut prefs = settings::load(&paths);

        let dest = destination
            .or_else(|| prefs.backup_destination.clone())
            .ok_or_else(|| "no backup folder chosen yet".to_string())?;

        let report = backup::create(
            &paths,
            std::path::Path::new(&dest),
            &backup::BackupOptions {
                include_key: prefs.backup_include_key,
                include_thumbnails: true,
                keep: prefs.backup_keep,
            },
        )
        .map_err(map_err)?;

        // Remember a destination that worked, so the next backup is one click.
        prefs.backup_destination = Some(dest);
        prefs.last_backup_at = Some(family_archive_core::util::now_iso8601());
        let _ = settings::save(&paths, &prefs);

        Ok(report)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_backups(
    state: State<'_, AppState>,
    destination: Option<String>,
) -> Result<Vec<family_archive_core::backup::BackupInfo>, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<family_archive_core::backup::BackupInfo>, String> {
        use family_archive_core::{backup, settings};
        let dest = destination.or_else(|| settings::load(&paths).backup_destination);
        let Some(dest) = dest else { return Ok(Vec::new()) };
        backup::list(std::path::Path::new(&dest)).map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Restore the catalogue. The catalogue being replaced is kept, not deleted.
#[tauri::command]
async fn restore_backup(
    state: State<'_, AppState>,
    bundle: String,
) -> Result<family_archive_core::backup::RestoreReport, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<family_archive_core::backup::RestoreReport, String> {
        use family_archive_core::backup;
        backup::restore(
            &paths,
            std::path::Path::new(&bundle),
            &backup::RestoreOptions { restore_key: true, restore_thumbnails: true },
        )
        .map_err(map_err)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Reclaim disk space. Changes nothing the user can see.
#[tauri::command]
async fn compact_catalogue(state: State<'_, AppState>) -> Result<String, String> {
    let paths = state.paths.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        use family_archive_core::pipeline::thumbnail;
        let archive = open_archive(&paths)?;

        let report = thumbnail::recompress_to_jpeg(&archive, &paths.thumbnails_dir()).map_err(map_err)?;
        let before = std::fs::metadata(paths.archive_db()).map(|m| m.len()).unwrap_or(0);
        archive.execute_batch("VACUUM").map_err(map_err)?;
        let after = std::fs::metadata(paths.archive_db()).map(|m| m.len()).unwrap_or(0);

        let mb = |n: u64| n / (1024 * 1024);
        Ok(format!(
            "{} thumbnails re-encoded ({} MB saved); catalogue {} MB -> {} MB",
            report.converted,
            mb(report.bytes_before.saturating_sub(report.bytes_after)),
            mb(before),
            mb(after)
        ))
    })
    .await
    .map_err(|e| e.to_string())?
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
                last_error: Arc::new(Mutex::new(None)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connected_volumes,
            likely_photo_folders,
            drive_coverage,
            scan_stats,
            scan_failures,
            folder_summaries,
            reveal_folder,
            last_scan_error,
            stop_scan,
            stop_pending,
            find_names,
            retry_failed_files,
            estimate_index,
            similar_photographs,
            propose_events,
            list_events,
            next_event_proposal,
            name_event,
            forget_event,
            merge_events,
            split_event,
            event_clients,
            event_split_points,
            event_files,
            choose_folder,
            get_settings,
            save_settings,
            describe_backup_destination,
            backup_now,
            list_backups,
            restore_backup,
            compact_catalogue,
            list_drives,
            register_drive,
            search_catalogue,
            start_index,
            cancel_index,
            is_indexing,
            get_progress,
            run_verifier,
            unnamed_face_counts,
            lightroom_available,
            open_original,
            health_due,
            spot_check_drive,
            suggest_event_name,
            single_copies,
            group_faces,
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
            set_person_relationship,
            person_relationships,
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
