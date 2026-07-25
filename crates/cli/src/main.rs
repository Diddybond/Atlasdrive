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
    /// Back up the catalogue, or restore it.
    ///
    /// Point `--to` at a folder your cloud client synchronises (Google Drive
    /// for Desktop, Dropbox, iCloud) and the backup travels off this Mac
    /// without AtlasDrive itself ever touching the network.
    Backup {
        #[command(subcommand)]
        action: BackupAction,
    },
    /// Reclaim disk space: re-encode legacy PNG thumbnails and compact the
    /// database. Safe to run at any time; changes nothing you can see.
    Compact,
    /// Write a privacy-redacted diagnostics bundle safe to share in a bug report.
    Report {
        /// Required, and the only supported mode: unredacted export is not offered.
        #[arg(long)]
        redacted: bool,
    },
}

#[derive(Subcommand)]
enum BackupAction {
    /// Write a backup.
    Now {
        /// Destination folder.
        #[arg(long)]
        to: PathBuf,
        /// Leave the master key out of the bundle. Face data in the backup then
        /// only decrypts on this Mac.
        #[arg(long)]
        no_key: bool,
        /// Skip the thumbnail mirror; back up the database only.
        #[arg(long)]
        no_thumbnails: bool,
        /// How many snapshots to keep.
        #[arg(long, default_value_t = 7)]
        keep: usize,
    },
    /// List the backups at a destination, newest first.
    List {
        #[arg(long)]
        at: PathBuf,
    },
    /// Restore the catalogue from a backup folder.
    ///
    /// The catalogue being replaced is kept, not deleted.
    Restore {
        /// A snapshot folder, as printed by `backup list`.
        #[arg(long)]
        from: PathBuf,
        /// Do not put the backup's master key back into the Keychain.
        #[arg(long)]
        no_key: bool,
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
    /// Re-read a connected drive and check the files are still the files.
    ///
    /// Compares every original against the content hash recorded when it was
    /// indexed. A file whose content changed while its size and modification
    /// time did not was not edited by anyone — that is silent corruption.
    Check {
        #[arg(long)]
        number: i64,
        /// Stop after this many files, for a quick sample of a large drive.
        #[arg(long)]
        limit: Option<usize>,
        /// Skip files already checked within this many days, so a big drive can
        /// be worked through over several sessions.
        #[arg(long)]
        skip_recent_days: Option<i64>,
    },
    /// Compare two drives by content. Neither needs to be connected.
    ///
    /// For understanding clone relationships, not for deleting anything.
    Compare {
        #[arg(long)]
        a: i64,
        #[arg(long)]
        b: i64,
        /// List the differing files, not just the counts.
        #[arg(long)]
        list: bool,
    },
    /// Find every pair of drives that look like clones of one another.
    Clones,
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
    /// Also index this file type, e.g. --include-type arw --include-type heic.
    /// By default a scan takes JPEG, PNG, TIFF and PSD only — RAW is skipped.
    #[arg(long = "include-type")]
    include_type: Vec<String>,
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
    Rebuild {
        /// Override the similarity threshold for grouping faces.
        #[arg(long)]
        threshold: Option<f32>,
    },
    /// Remove a person added by mistake. Faces are kept and return to unnamed.
    Forget {
        #[arg(long)]
        person: String,
    },
    /// Correct a person's name. If the name already exists, the two are merged.
    Rename {
        #[arg(long)]
        person: String,
        #[arg(long)]
        name: String,
    },
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
        Command::Backup { action } => backup_cmd(&ctx, action),
        Command::Compact => compact_cmd(&ctx),
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
        DriveAction::Check { number, limit, skip_recent_days } => {
            drive_check_cmd(ctx, number, limit, skip_recent_days)
        }
        DriveAction::Compare { a, b, list } => {
            let archive = db::open(&ctx.paths.archive_db(), db::SchemaKind::Archive)?;
            let c = family_archive_core::compare::compare_drives(&archive, a, b)?;
            println!("{}", c.summary());
            println!(
                "  drive {}: {} distinct files, {} not on {}",
                c.a_number, c.a_total, c.only_a_count, c.b_number
            );
            println!(
                "  drive {}: {} distinct files, {} not on {}",
                c.b_number, c.b_total, c.only_b_count, c.a_number
            );
            if list {
                if !c.only_a.is_empty() {
                    println!("\n  only on drive {} ({}):", c.a_number, human_bytes(c.only_a_bytes as u64));
                    for f in &c.only_a {
                        println!("    {}", f.relative_path);
                    }
                }
                if !c.only_b.is_empty() {
                    println!("\n  only on drive {} ({}):", c.b_number, human_bytes(c.only_b_bytes as u64));
                    for f in &c.only_b {
                        println!("    {}", f.relative_path);
                    }
                }
                if c.truncated {
                    println!("\n  (list truncated)");
                }
            }
            Ok(())
        }
        DriveAction::Clones => {
            let archive = db::open(&ctx.paths.archive_db(), db::SchemaKind::Archive)?;
            let pairs = family_archive_core::compare::find_near_identical(&archive)?;
            if pairs.is_empty() {
                println!("No drives look like clones of one another.");
                return Ok(());
            }
            println!("Drives that look like clones:");
            for c in pairs {
                println!("  {}", c.summary());
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
    opts.extra_extensions = args
        .include_type
        .iter()
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .collect();

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
    let repo = SearchRepo::with_index_dir(&archive, ctx.paths.cache_dir());
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
        FaceAction::Forget { person } => {
            repo.remove_person(&person)?;
            println!("Removed. Their faces are kept and are unnamed again.");
            Ok(())
        }
        FaceAction::Rename { person, name } => {
            let updated = repo.rename_person(&person, &name)?;
            println!("Now called {}.", updated.display_name);
            Ok(())
        }
        FaceAction::Rebuild { threshold } => {
            let key = keystore::default_keystore(ctx.paths.keys_dir()).get_or_create()?;
            // Cluster the partition the faces were actually written under, and
            // use the threshold that suits that model's embedding space.
            let (model_id, model_version): (String, String) = archive
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
                });
            let t = threshold.unwrap_or_else(|| faces::cluster_threshold_for(&model_id));
            let n = repo.rebuild_clusters(&model_id, &model_version, &key, t)?;
            println!("Rebuilt {n} group(s) from {model_id} faces at threshold {t}.");
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
            println!("{}", summary.summary());
            if summary.skipped_nothing_to_say > 0 {
                println!("{} had nothing to record.", summary.skipped_nothing_to_say);
            }
            println!(
                "Originals were not modified, and no existing .xmp was altered — \
                 AtlasDrive only ever creates a sidecar where none exists."
            );
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

/// Reclaim space without changing what the catalogue contains.
///
/// Two independent sources of waste, both worth addressing before an archive
/// reaches twenty drives. Thumbnails written by older versions are PNG, roughly
/// five times the size of the JPEG now used. And SQLite does not return freed
/// pages to the filesystem, so a catalogue that has had faces rebuilt or files
/// removed keeps the space.
fn compact_cmd(ctx: &Ctx) -> Result<()> {
    use family_archive_core::pipeline::thumbnail;

    let archive = db::open(&ctx.paths.archive_db(), db::SchemaKind::Archive)?;

    println!("Re-encoding legacy PNG thumbnails...");
    let report = thumbnail::recompress_to_jpeg(&archive, &ctx.paths.thumbnails_dir())?;
    if report.converted > 0 {
        println!(
            "  {} converted: {} -> {} ({}x smaller)",
            report.converted,
            human_bytes(report.bytes_before),
            human_bytes(report.bytes_after),
            if report.bytes_after > 0 {
                report.bytes_before / report.bytes_after.max(1)
            } else {
                0
            }
        );
    } else {
        println!("  nothing to convert ({} already JPEG)", report.already_jpeg);
    }
    if report.failed > 0 {
        println!("  {} could not be read and were left alone", report.failed);
    }

    let before = std::fs::metadata(ctx.paths.archive_db()).map(|m| m.len()).unwrap_or(0);
    println!("Compacting the catalogue...");
    archive.execute_batch("VACUUM")?;
    let after = std::fs::metadata(ctx.paths.archive_db()).map(|m| m.len()).unwrap_or(0);
    println!("  {} -> {}", human_bytes(before), human_bytes(after));

    Ok(())
}

fn drive_check_cmd(
    ctx: &Ctx,
    number: i64,
    limit: Option<usize>,
    skip_recent_days: Option<i64>,
) -> Result<()> {
    use family_archive_core::bitrot::{self, Verdict};

    let archive = db::open(&ctx.paths.archive_db(), db::SchemaKind::Archive)?;
    let started = std::time::Instant::now();

    let report = bitrot::verify_drive(
        &archive,
        number,
        &bitrot::VerifyOptions {
            limit,
            skip_verified_within_days: skip_recent_days,
            cancel: None,
        },
        |done, total| {
            if total > 0 && (done % 200 == 0 || done == total) {
                eprint!("\r  checked {done}/{total}");
            }
        },
    )?;
    eprintln!();

    println!("Drive {} — integrity check", report.drive_number);
    println!(
        "  {} files read ({}) in {:.0}s",
        report.checked,
        human_bytes(report.bytes_read),
        started.elapsed().as_secs_f64()
    );
    println!("  {} intact", report.intact);

    let corrupt = report.count(Verdict::Corrupt);
    let unreadable = report.count(Verdict::Unreadable);
    let edited = report.count(Verdict::Edited);
    let missing = report.count(Verdict::Missing);

    if edited > 0 {
        println!("  {edited} edited since indexing — re-index to bring the catalogue up to date");
    }
    if missing > 0 {
        println!("  {missing} no longer on the drive");
    }

    if corrupt == 0 && unreadable == 0 {
        println!("  no corruption found");
    } else {
        println!();
        println!("  {corrupt} CORRUPT, {unreadable} unreadable:");
        for f in report.problems() {
            println!("    {} — {}", f.relative_path, f.detail);
        }
    }
    if report.incomplete {
        println!("  (stopped early; run again to continue)");
    }

    // A corrupt original is a real fault, so the exit code has to say so —
    // this command belongs in a cron job.
    if corrupt > 0 || unreadable > 0 {
        return Err(Error::VerifierFailure(format!(
            "{corrupt} corrupt and {unreadable} unreadable file(s) on drive {number}"
        )));
    }
    Ok(())
}

fn human_bytes(n: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if n >= 1024 * MB {
        format!("{:.1} GB", n as f64 / (1024.0 * MB as f64))
    } else if n >= MB {
        format!("{} MB", n / MB)
    } else {
        format!("{} KB", n.div_ceil(1024))
    }
}

fn backup_cmd(ctx: &Ctx, action: BackupAction) -> Result<()> {
    use family_archive_core::backup;

    match action {
        BackupAction::Now { to, no_key, no_thumbnails, keep } => {
            let options = backup::BackupOptions {
                include_key: !no_key,
                include_thumbnails: !no_thumbnails,
                keep: Some(keep),
            };
            let report = backup::create(&ctx.paths, &to, &options)?;
            println!("Backed up to {}", report.bundle);
            println!("  catalogue: {}", human_bytes(report.db_bytes));
            if !no_thumbnails {
                println!(
                    "  thumbnails: {} new ({}), {} already there",
                    report.thumbnails_copied,
                    human_bytes(report.thumbnail_bytes_copied),
                    report.thumbnails_present
                );
            }
            if report.key_included {
                println!("  master key included — anyone who can read that folder can read the");
                println!("  face data. Delete {} to store it separately.", backup::KEY_FILE);
            } else {
                println!("  master key NOT included — face data restores only on this Mac.");
            }
            if report.pruned > 0 {
                println!("  removed {} older backup(s)", report.pruned);
            }
            Ok(())
        }
        BackupAction::List { at } => {
            let all = backup::list(&at)?;
            if all.is_empty() {
                println!("No backups at {}", at.display());
                return Ok(());
            }
            println!("Backups at {} (newest first)", at.display());
            for b in all {
                match b.manifest {
                    Some(m) => println!(
                        "  {}  {} files, {} faces, {} named  ({})",
                        b.name,
                        m.counts.files,
                        m.counts.faces,
                        m.counts.people_named,
                        human_bytes(m.db_bytes)
                    ),
                    None => println!("  {}  (no manifest)", b.name),
                }
            }
            Ok(())
        }
        BackupAction::Restore { from, no_key } => {
            let report = backup::restore(
                &ctx.paths,
                &from,
                &backup::RestoreOptions {
                    restore_key: !no_key,
                    restore_thumbnails: true,
                },
            )?;
            println!("Restored from {}", report.restored_from);
            println!(
                "  {} drives, {} files, {} faces, {} named people",
                report.counts.drives,
                report.counts.files,
                report.counts.faces,
                report.counts.people_named
            );
            println!("  {} thumbnails restored", report.thumbnails_restored);
            if report.key_restored {
                println!("  master key restored to the Keychain");
            }
            if let Some(prev) = report.previous_catalogue {
                println!("  the catalogue this replaced is kept at {prev}");
            }
            Ok(())
        }
    }
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

    // Reported for the desktop bundle rather than this CLI binary where one is
    // present: the bundle is the thing that holds the Keychain entry, and the
    // two can legitimately differ.
    let signature = match family_archive_core::signing::enclosing_bundle() {
        Some(bundle) => family_archive_core::signing::of_path(&bundle),
        None => family_archive_core::signing::current(),
    };
    println!("  code signature: {}", signature.describe());
    if !signature.identity_is_stable() {
        println!("    an unsigned build has no tamper detection, and its identity");
        println!("    changes on every rebuild — which is why macOS keeps asking");
        println!("    for Keychain access. Fix with: ./scripts/sign-app.sh");
    }
    Ok(())
}
