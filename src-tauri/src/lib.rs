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
    Ok(SearchResponse { results, understood, text_only })
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
            clear_date_override
        ])
        .run(tauri::generate_context!())
        .expect("error while running AtlasDrive");
}
