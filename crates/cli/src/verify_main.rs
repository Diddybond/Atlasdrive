//! `atlasdrive-verify` — the real, standalone verifier binary.
//!
//! This is an executable that exits non-zero on failure (see
//! `docs/13_TESTING_AND_VERIFIER.md`). It is intentionally a separate binary
//! from the main CLI so it can be wired into CI and pre-commit hooks as an
//! independent safety gate that is never weakened to obtain a pass.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use family_archive_core::config::{AppPaths, Config};
use family_archive_core::crypto::keystore;
use family_archive_core::{db, verifier};

#[derive(Parser)]
#[command(name = "atlasdrive-verify", version, about = "Independent AtlasDrive verifier.")]
struct Args {
    /// Application-support data directory (defaults to the OS location).
    #[arg(long)]
    home: Option<PathBuf>,
    /// Emit the full report as JSON.
    #[arg(long)]
    json: bool,
    /// Enforce the configured free-space floor (off for a targeted check).
    #[arg(long)]
    enforce_disk_floor: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let paths = match args.home {
        Some(h) => AppPaths::new(h),
        None => AppPaths::discover(),
    };
    if let Err(e) = paths.ensure() {
        eprintln!("verify: cannot prepare data dir: {e}");
        return ExitCode::from(family_archive_core::error::exit::INTERNAL as u8);
    }

    let archive = match db::open(&paths.archive_db(), db::SchemaKind::Archive) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("verify: cannot open archive.db: {e}");
            return ExitCode::from(e.exit_code() as u8);
        }
    };
    let queue = db::open(&paths.queue_db(), db::SchemaKind::Queue).ok();
    let key = keystore::default_keystore(paths.keys_dir()).get_or_create().ok();

    let mut config = Config::default();
    if !args.enforce_disk_floor {
        config.free_space_floor_bytes = 0;
    }

    let ctx = verifier::VerifyContext {
        archive: &archive,
        queue: queue.as_ref(),
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

    let report = match verifier::run(&ctx) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("verify: {e}");
            return ExitCode::from(e.exit_code() as u8);
        }
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    } else {
        for c in &report.checks {
            println!("[{:?}] {} - {}", c.status, c.name, c.detail);
        }
        println!("\n{}", report.summary());
    }

    ExitCode::from(report.exit_code() as u8)
}
