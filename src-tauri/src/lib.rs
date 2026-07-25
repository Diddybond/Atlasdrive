//! Family Archive desktop backend (Tauri v2).
//!
//! Thin command layer over `family-archive-core`. The GUI and CLI call the same
//! service layer, so all safety guarantees live in core, not here. Long-running
//! indexing is executed on a background thread so the UI stays responsive.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{Manager, State};

use family_archive_core::ai::{CancelToken, EngineRegistry};
use family_archive_core::config::{AppPaths, Config};
use family_archive_core::crypto::keystore;
use family_archive_core::drive::{manifest::DriveManifest, DriveRepo, RegisterParams};
use family_archive_core::logging::Logger;
use family_archive_core::pipeline::{IndexMode, IndexOptions, Pipeline};
use family_archive_core::progress::Progress;
use family_archive_core::search::{SearchFilters, SearchResult};
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
        last_scan_at: drive.last_scan_at,
        image_count: 0,
    })
}

#[tauri::command]
fn search_catalogue(
    state: State<AppState>,
    query: String,
    drive: Option<i64>,
    include_offline: bool,
) -> Result<Vec<SearchResult>, String> {
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
    let mut results = repo.text_search(&query, &filters).map_err(map_err)?;
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
    Ok(results)
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
                engines: std::sync::Arc::new(EngineRegistry::local_default()),
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
    let mut config = Config::default();
    config.free_space_floor_bytes = 0;
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
    out.insert("ai_offline".into(), EngineRegistry::local_default().all_offline().to_string());
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
            doctor
        ])
        .run(tauri::generate_context!())
        .expect("error while running Family Archive");
}
