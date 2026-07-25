//! `atlasdrive` command-line interface.
//!
//! The GUI calls the same core services; this CLI is for development, testing
//! and advanced recovery (see `docs/12_CLI_AND_COMMANDS.md`). Exit codes are the
//! stable contract from `family_archive_core::error::exit`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use family_archive_core::ai::EngineRegistry;
use family_archive_core::config::{AppPaths, Config};
use family_archive_core::crypto::keystore;
use family_archive_core::drive::{manifest::DriveManifest, DriveRepo, RegisterParams};
use family_archive_core::error::{exit, Error, Result};
use family_archive_core::logging::Logger;
use family_archive_core::pipeline::{IndexMode, IndexOptions, Pipeline};
use family_archive_core::search::{SearchFilters, SearchRepo, VisualQuery};
use family_archive_core::{db, diagnostics, faces, verifier};

#[derive(Parser)]
#[command(
    name = "atlasdrive",
    version,
    about = "Private, local-first family photo catalogue across numbered drives."
)]
struct Cli {
    /// Override the application-support data directory.
    #[arg(long, global = true)]
    home: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Drive registration and inspection.
    Drive {
        #[command(subcommand)]
        action: DriveAction,
    },
    /// Index a drive's photographs (safe, resumable, offline).
    Index(IndexArgs),
    /// Search the catalogue (works with drives offline).
    Search(SearchArgs),
    /// Run the real verifier.
    Verify(VerifyArgs),
    /// Face review preparation and related tools.
    Faces {
        #[command(subcommand)]
        action: FaceAction,
    },
    /// Environment and catalogue diagnostics.
    Doctor,
    /// Correct the date of a photograph. Your correction always wins.
    Date {
        /// File id, as shown by `atlasdrive search`.
        #[arg(long)]
        file: String,
        /// Earliest possible date, YYYY-MM-DD.
        #[arg(long)]
        from: String,
        /// Latest possible date; omit if the date is exact.
        #[arg(long)]
        to: Option<String>,
        /// Remove a previous correction and let the estimate stand again.
        #[arg(long)]
        clear: bool,
    },
    /// Write a privacy-redacted diagnostics bundle safe to share in a bug report.
    Report {
        /// Required, and the only supported mode: unredacted export is not offered.
        #[arg(long)]
        redacted: bool,
    },
}

#[derive(Subcommand)]
enum DriveAction {
    /// Register a drive and assign a physical number.
    Register {
        #[arg(long)]
        number: i64,
        #[arg(long)]
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        write_manifest: bool,
        #[arg(long)]
        physical_location: Option<String>,
        #[arg(long)]
        category: Vec<String>,
    },
    /// Inspect a connected drive's identity signals (changes nothing).
    Inspect {
        #[arg(long)]
        path: PathBuf,
    },
    /// Update where a drive lives and how it is categorised.
    Set {
        #[arg(long)]
        number: i64,
        /// Rename the drive. The number stays the same.
        #[arg(long)]
        name: Option<String>,
        /// Where the drive physically is, e.g. "Drawer 2". Pass "" to clear.
        #[arg(long)]
        physical_location: Option<String>,
        /// Replaces the existing categories; repeat for several.
        #[arg(long)]
        category: Vec<String>,
    },
    /// What is stored on a drive — works with the drive disconnected.
    Contents {
        /// Limit to one drive; omit to inventory every registered drive.
        #[arg(long)]
        number: Option<i64>,
    },
    /// List registered drives.
    List,
}

#[derive(Parser)]
struct IndexArgs {
    #[arg(long)]
    drive: i64,
    #[arg(long)]
    path: PathBuf,
    #[arg(long)]
    resume: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    verify_only: bool,
    #[arg(long)]
    rebuild_faces: bool,
    #[arg(long)]
    batch_size: Option<usize>,
    #[arg(long, default_value = "20GB")]
    free_space_floor: String,
    #[arg(long)]
    exclude: Vec<String>,
}

#[derive(Parser)]
struct SearchArgs {
    query: String,
    #[arg(long)]
    drive: Option<i64>,
    #[arg(long)]
    offline_included: bool,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    /// Search text and metadata only, skipping the local visual embedding.
    #[arg(long)]
    text_only: bool,
}

#[derive(Parser)]
struct VerifyArgs {
    #[arg(long)]
    run: Option<String>,
    #[arg(long)]
    drive: Option<i64>,
    #[arg(long)]
    full: bool,
    /// Emit the report as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum FaceAction {
    /// Prepare a bounded candidate batch for human review, then stop.
    PrepareReview {
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Rebuild face clusters without reopening originals.
    Rebuild,
    /// Tag a face cluster with a person's name. Recognised from then on.
    Name {
        #[arg(long)]
        cluster: String,
        #[arg(long)]
        name: String,
    },
    /// People you have named, and how many faces each has.
    People,
    /// Generate the missing face pictures for an archive indexed before the
    /// gallery existed. Needs the drive connected — it re-reads the originals.
    BackfillThumbnails {
        #[arg(long, default_value_t = 5000)]
        limit: usize,
    },
    /// Copy one person's photographs into a folder. Originals are never touched.
    Gather {
        #[arg(long)]
        person: String,
        #[arg(long)]
        into: PathBuf,
    },
    /// Write XMP sidecars next to a person's originals, for Bridge/Lightroom.
    ///
    /// This WRITES to the drive (a new .xmp beside each photograph). The
    /// original file is never opened for writing.
    Sidecars {
        #[arg(long)]
        person: String,
        /// Required. Writing to a drive is never implicit.
        #[arg(long)]
        write_to_drive: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::from(exit::SUCCESS as u8),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

/// Shared per-invocation context.
struct Ctx {
    paths: AppPaths,
    config: Config,
}

impl Ctx {
    fn new(home: Option<PathBuf>) -> Result<Self> {
        let paths = match home {
            Some(h) => AppPaths::new(h),
            None => AppPaths::discover(),
        };
        paths.ensure()?;
        Ok(Self {
            paths,
            config: Config::default(),
        })
    }
    fn open_archive(&self) -> Result<rusqlite::Connection> {
        db::open(&self.paths.archive_db(), db::SchemaKind::Archive)
    }
    fn open_queue(&self) -> Result<rusqlite::Connection> {
        db::open(&self.paths.queue_db(), db::SchemaKind::Queue)
    }
}

fn run(cli: Cli) -> Result<()> {
    let ctx = Ctx::new(cli.home)?;
    match cli.command {
        Command::Drive { action } => drive_cmd(&ctx, action),
        Command::Index(args) => index_cmd(&ctx, args),
        Command::Search(args) => search_cmd(&ctx, args),
        Command::Verify(args) => verify_cmd(&ctx, args),
        Command::Faces { action } => faces_cmd(&ctx, action),
        Command::Doctor => doctor_cmd(&ctx),
        Command::Date { file, from, to, clear } => date_cmd(&ctx, &file, &from, to.as_deref(), clear),
        Command::Report { redacted } => report_cmd(&ctx, redacted),
    }
}

fn drive_cmd(ctx: &Ctx, action: DriveAction) -> Result<()> {
    let archive = ctx.open_archive()?;
    let repo = DriveRepo::new(&archive);
    match action {
        DriveAction::Register {
            number,
            path,
            name,
            write_manifest,
            physical_location,
            category,
        } => {
            let drive = repo.register(&RegisterParams {
                drive_number: number,
                friendly_name: name.clone(),
                volume_name: path.file_name().map(|s| s.to_string_lossy().to_string()),
                physical_location,
                categories: category,
                ..Default::default()
            })?;
            println!("Registered Drive {} ({})", drive.drive_number, drive.id);
            if write_manifest {
                let m = DriveManifest::new(&drive.id, drive.drive_number, name);
                let written = m.write_to_volume(&path)?;
                repo.audit(&drive.id, "manifest_written", None)?;
                println!("Wrote identity manifest: {}", written.display());
            }
            Ok(())
        }
        DriveAction::Inspect { path } => {
            let manifest = DriveManifest::read_from_volume(&path)?;
            let rec = repo.recognize(manifest.as_ref(), None, None)?;
            println!("{}", serde_json::to_string_pretty(&rec)?);
            if let Some(m) = manifest {
                println!("Manifest: Drive {} ({})", m.drive_number, m.drive_id);
            } else {
                println!("No app manifest present on this volume.");
            }
            Ok(())
        }
        DriveAction::Set {
            number,
            name,
            physical_location,
            category,
        } => {
            let drive = repo
                .get_by_number(number)?
                .ok_or_else(|| Error::InvalidArgs(format!("drive {number} not registered")))?;
            // An empty --category list means "not specified", not "clear them":
            // clearing is a separate, explicit act the user can do by passing an
            // empty string.
            let categories = (!category.is_empty()).then_some(category.as_slice());
            repo.update_details(&drive.id, physical_location.as_deref(), categories)?;
            if let Some(new_name) = name.as_deref() {
                repo.rename(&drive.id, new_name)?;
            }
            let updated = repo.get_by_number(number)?.expect("drive still present");
            println!(
                "Drive {} — {}: {} · {}",
                updated.drive_number,
                updated.friendly_name.clone().unwrap_or_else(|| "unnamed".into()),
                updated
                    .physical_location
                    .unwrap_or_else(|| "no location recorded".into()),
                if updated.categories.is_empty() {
                    "no categories".to_string()
                } else {
                    updated.categories.join(", ")
                }
            );
            Ok(())
        }
        DriveAction::Contents { number } => {
            let all = family_archive_core::inventory::drive_contents(&archive, number)?;
            if all.is_empty() {
                println!("No drives registered yet.");
                return Ok(());
            }
            for c in all {
                println!("{}", c.summary());
                if !c.top_tags.is_empty() {
                    let shown: Vec<String> = c
                        .top_tags
                        .iter()
                        .map(|t| format!("{} ({})", t.tag, t.count))
                        .collect();
                    println!("   What's in the pictures: {}", shown.join(", "));
                }
                if c.with_text_count > 0 {
                    println!("   {} with readable text", c.with_text_count);
                }
                if c.missing_count > 0 {
                    println!(
                        "   {} catalogued file(s) were not found on the last scan",
                        c.missing_count
                    );
                }
                println!(
                    "   Last scanned: {}",
                    c.last_scan_at.unwrap_or_else(|| "never".into())
                );
            }
            Ok(())
        }
        DriveAction::List => {
            for d in repo.list()? {
                println!(
                    "Drive {:>4}  {:<20}  {}  {}",
                    d.drive_number,
                    d.friendly_name.unwrap_or_default(),
                    d.status,
                    d.last_scan_at.unwrap_or_else(|| "never scanned".into())
                );
                let location = d
                    .physical_location
                    .unwrap_or_else(|| "no location recorded".into());
                if d.categories.is_empty() {
                    println!("            {location}");
                } else {
                    println!("            {location} · {}", d.categories.join(", "));
                }
            }
            Ok(())
        }
    }
}

fn build_pipeline<'a>(
    ctx: &'a Ctx,
    archive: &'a rusqlite::Connection,
    queue: &'a rusqlite::Connection,
    key: &'a family_archive_core::crypto::MasterKey,
) -> Pipeline<'a> {
    Pipeline {
        archive,
        queue,
        paths: &ctx.paths,
        engines: Arc::new(EngineRegistry::local_with_vision()),
        key,
        logger: Logger::new(ctx.paths.index_log()).echo_stderr(true),
        cancel: family_archive_core::ai::CancelToken::new(),
    }
}

fn index_cmd(ctx: &Ctx, args: IndexArgs) -> Result<()> {
    let archive = ctx.open_archive()?;
    let queue = ctx.open_queue()?;
    let key = keystore::default_keystore(ctx.paths.keys_dir()).get_or_create()?;

    let mut config = ctx.config.clone();
    config.free_space_floor_bytes = Config::parse_size(&args.free_space_floor)?;
    if let Some(bs) = args.batch_size {
        config.batch_size = bs;
    }

    let mode = if args.dry_run {
        IndexMode::DryRun
    } else if args.verify_only {
        IndexMode::VerifyOnly
    } else if args.rebuild_faces {
        IndexMode::RebuildFaces
    } else {
        IndexMode::Normal
    };

    let mut opts = IndexOptions::new(args.drive, args.path);
    opts.mode = mode;
    opts.resume = args.resume;
    opts.exclusions = args.exclude;
    opts.config = config;

    let pipeline = build_pipeline(ctx, &archive, &queue, &key);
    let summary = pipeline.run(&opts)?;

    println!(
        "\nDrive {}: discovered {}, done {}, failed {}, batches {}{}",
        args.drive,
        summary.files_discovered,
        summary.files_done,
        summary.files_failed,
        summary.batches,
        if summary.dry_run { " (dry-run)" } else { "" }
    );
    if summary.files_changed > 0 || summary.files_missing > 0 {
        println!(
            "Rescan: {} photograph(s) changed since the last scan and were re-read; \
             {} no longer on this drive and marked missing.",
            summary.files_changed, summary.files_missing
        );
    }
    Ok(())
}

fn search_cmd(ctx: &Ctx, args: SearchArgs) -> Result<()> {
    let archive = ctx.open_archive()?;
    let repo = SearchRepo::new(&archive);
    let filters = SearchFilters {
        drive_number: args.drive,
        online_only: !args.offline_included,
        include_offline: args.offline_included,
        limit: args.limit,
        ..Default::default()
    };
    // Natural-language search: text/metadata plus a locally embedded query.
    // Everything reads the local catalogue, so this works with drives offline.
    let mut visual_note = None;
    let embedded = if args.text_only {
        None
    } else {
        let registry = EngineRegistry::local_default();
        let engine = registry.engine_for(family_archive_core::ai::Capability::TextEmbedding);
        let cancel = family_archive_core::ai::CancelToken::new();
        // A missing text encoder is not an error: text search still works.
        engine.text_embedding(&args.query, &cancel).ok().map(|q| {
            if q.meta.confidence == 0.0 {
                visual_note = Some("No visual terms recognised; searched text and metadata only.");
            }
            (q, engine.model_id().to_string(), engine.model_version().to_string())
        })
    };
    let visual = embedded.as_ref().map(|(q, id, version)| VisualQuery {
        vector: &q.value.vector,
        model_id: id,
        model_version: version,
        coverage: q.meta.confidence,
    });

    let results = repo.natural_language_search(&args.query, visual, &filters)?;
    if let Some(note) = visual_note {
        println!("{note}");
    }
    if results.is_empty() {
        println!("No results for \"{}\".", args.query);
        return Ok(());
    }

    // Lead with the answer to "which drive do I need?", then the photographs.
    let mut grouped = family_archive_core::inventory::drives_matching(&results);
    family_archive_core::inventory::locate_matches(&archive, &mut grouped)?;
    println!("{}", family_archive_core::inventory::where_to_look(&grouped));
    for g in &grouped {
        println!(
            "  Drive {:>3}  {} photograph(s){}",
            g.drive_number,
            g.match_count,
            if g.online { "" } else { " — disconnected" }
        );
    }
    println!();

    for r in results {
        let status = if r.online { "online" } else { "OFFLINE" };
        let date = r
            .date_range
            .map(|(a, b)| if a == b { a } else { format!("{a}..{b}") })
            .unwrap_or_else(|| "date uncertain".into());
        println!(
            "Drive {:>3} [{}]  {}  ({})  {}  [{} {:.0}%]",
            r.drive_number,
            status,
            r.filename,
            date,
            r.relative_path,
            r.matched.join("+"),
            r.score * 100.0
        );
        if !r.online {
            println!("      -> Connect Drive {} to open the original.", r.drive_number);
        }
    }
    Ok(())
}

fn verify_cmd(ctx: &Ctx, args: VerifyArgs) -> Result<()> {
    let archive = ctx.open_archive()?;
    let queue = ctx.open_queue()?;
    let key = keystore::default_keystore(ctx.paths.keys_dir()).get_or_create().ok();

    let mut config = ctx.config.clone();
    if !args.full {
        // A targeted verify does not fail purely on the machine's free disk.
        config.free_space_floor_bytes = 0;
    }
    let _ = (args.run, args.drive); // targeted scoping reserved; full suite runs today

    let vctx = verifier::VerifyContext {
        archive: &archive,
        queue: Some(&queue),
        paths: &ctx.paths,
        config: &config,
        key: key.as_ref(),
        face_model: (
            family_archive_core::ai::local::MODEL_ID.to_string(),
            family_archive_core::ai::local::MODEL_VERSION.to_string(),
        ),
        observed_throughput: None,
        network_blocked_attempts: 0,
    };
    let report = verifier::run(&vctx)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for c in &report.checks {
            println!("  [{:?}] {} - {}", c.status, c.name, c.detail);
        }
        println!("\n{}", report.summary());
    }

    if report.has_halt() || !report.ok() {
        return Err(Error::VerifierFailure(report.summary()));
    }
    Ok(())
}

fn faces_cmd(ctx: &Ctx, action: FaceAction) -> Result<()> {
    let archive = ctx.open_archive()?;
    let repo = faces::FaceRepo::new(&archive);
    match action {
        FaceAction::PrepareReview { limit } => {
            let batch = repo.prepare_review(limit)?;
            println!("Prepared {} cluster(s) for human review:", batch.len());
            for c in &batch {
                println!(
                    "  cluster {}  faces={}  status={}",
                    &c.cluster_id[..8.min(c.cluster_id.len())],
                    c.face_count,
                    c.status
                );
            }
            println!("\nReview stops here for human judgement - no names assigned automatically.");
            Ok(())
        }
        FaceAction::Rebuild => {
            let key = keystore::default_keystore(ctx.paths.keys_dir()).get_or_create()?;
            let n = repo.rebuild_clusters(
                family_archive_core::ai::local::MODEL_ID,
                family_archive_core::ai::local::MODEL_VERSION,
                &key,
                faces::DEFAULT_CLUSTER_THRESHOLD,
            )?;
            println!("Rebuilt {n} cluster(s) without reopening originals.");
            Ok(())
        }
        FaceAction::Name { cluster, name } => {
            let person = repo.tag_cluster_with_name(&cluster, &name)?;
            println!("Tagged as {}.", person.display_name);
            println!(
                "AtlasDrive will suggest {} when it sees a similar face on the next scan.",
                person.display_name
            );
            Ok(())
        }
        FaceAction::BackfillThumbnails { limit } => {
            let queue = ctx.open_queue()?;
            let key = keystore::default_keystore(ctx.paths.keys_dir()).get_or_create()?;
            let pipeline = build_pipeline(ctx, &archive, &queue, &key);
            let (done, skipped) = pipeline.backfill_face_thumbnails(limit)?;
            println!("Generated {done} face picture(s); {skipped} skipped.");
            if skipped > 0 {
                println!("Skipped faces are on drives that are not connected, or too small to show.");
            }
            Ok(())
        }
        FaceAction::Gather { person, into } => {
            let ids: Vec<String> = repo
                .photos_of_person(&person)?
                .into_iter()
                .map(|p| p.file_id)
                .collect();
            let summary = family_archive_core::export::copy_photos(&archive, &ids, &into)?;
            println!("{}", summary.summary());
            Ok(())
        }
        FaceAction::Sidecars { person, write_to_drive } => {
            if !write_to_drive {
                return Err(Error::InvalidArgs(
                    "writing sidecars puts new .xmp files on the drive; pass --write-to-drive to confirm"
                        .into(),
                ));
            }
            let ids: Vec<String> = repo
                .photos_of_person(&person)?
                .into_iter()
                .map(|p| p.file_id)
                .collect();
            let summary = family_archive_core::export::write_xmp_sidecars(&archive, &ids)?;
            println!(
                "Wrote {} sidecar(s). {} skipped (drive not connected), {} had nothing to record.",
                summary.written, summary.skipped_offline, summary.skipped_nothing_to_say
            );
            println!("Originals were not modified — each .xmp is a new file beside its photograph.");
            Ok(())
        }
        FaceAction::People => {
            let people = repo.people()?;
            if people.is_empty() {
                println!("Nobody named yet. Run `atlasdrive faces prepare-review` to see candidates.");
                return Ok(());
            }
            for p in people {
                println!(
                    "{:<28} {} confirmed, {} awaiting your confirmation",
                    p.display_name, p.confirmed_faces, p.suggested_faces
                );
            }
            Ok(())
        }
    }
}

fn date_cmd(ctx: &Ctx, file: &str, from: &str, to: Option<&str>, clear: bool) -> Result<()> {
    let archive = ctx.open_archive()?;
    let repo = family_archive_core::dates::DateRepo::new(&archive);
    if clear {
        repo.clear_user_override(file)?;
        println!("Correction removed. AtlasDrive's own estimate applies again.");
        return Ok(());
    }
    let est = repo.set_user_override(file, from, to.unwrap_or(from))?;
    println!("{}", family_archive_core::dates::describe(&est));
    println!("Your correction is kept even if this photograph is analysed again.");
    Ok(())
}

fn report_cmd(ctx: &Ctx, redacted: bool) -> Result<()> {
    if !redacted {
        // There is no unredacted export. Refusing loudly is better than quietly
        // producing one kind of file when the user asked for another.
        return Err(Error::InvalidArgs(
            "only --redacted export is supported; AtlasDrive does not produce an \
             unredacted diagnostics bundle"
                .into(),
        ));
    }
    let archive = ctx.open_archive()?;
    let queue = ctx.open_queue()?;
    let diag = diagnostics::collect(&archive, Some(&queue), &ctx.paths, None)?;
    let path = diagnostics::write(&ctx.paths, &diag)?;
    println!("Wrote redacted diagnostics: {}", path.display());
    println!(
        "  {} drive(s), {} photograph(s) catalogued, {} complete.",
        diag.catalogue.drives, diag.catalogue.files_total, diag.catalogue.files_complete
    );
    println!("  Contains counts, versions and check outcomes only — no names, paths or dates.");
    Ok(())
}

fn doctor_cmd(ctx: &Ctx) -> Result<()> {
    println!("AtlasDrive doctor");
    println!("  data root: {}", ctx.paths.root.display());
    let ks = keystore::default_keystore(ctx.paths.keys_dir());
    println!("  keystore:  {}", ks.backend_name());
    match ks.get_or_create() {
        Ok(_) => println!("  key:       available"),
        Err(e) => println!("  key:       ERROR {e}"),
    }
    let archive = ctx.open_archive()?;
    let sv = db::schema_version(&archive)?;
    println!("  archive schema version: {sv}");
    match db::integrity_check(&archive) {
        Ok(()) => println!("  archive integrity: ok"),
        Err(e) => println!("  archive integrity: FAIL {e}"),
    }
    let free = family_archive_core::util::available_space(&ctx.paths.root)?;
    println!("  free space on data volume: {} MB", free / (1024 * 1024));
    let engines = EngineRegistry::local_with_vision();
    println!("  AI engine offline-only: {}", engines.all_offline());
    Ok(())
}
