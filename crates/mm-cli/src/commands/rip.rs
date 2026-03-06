use anyhow::Result;
use clap::{Args, Subcommand};
use mm_config::AppConfig;
use mm_db::Db;
use mm_ripper::{run_daemon, musicbrainz, pipeline::RipPipeline};
use tracing::info;
use uuid::Uuid;

#[derive(Subcommand)]
pub enum RipCommands {
    /// Start the ripper daemon - watches for disc insertion automatically
    Daemon,
    /// Manually rip a disc from a specific drive
    Manual(ManualArgs),
    /// List rip jobs
    Jobs(JobsArgs),
}

#[derive(Args)]
pub struct ManualArgs {
    /// Drive path, e.g. /dev/cdrom (Linux) or D:\\ (Windows)
    #[arg(long)]
    drive: String,

    /// Optionally link this rip to a watchlist entry UUID
    #[arg(long)]
    watchlist_id: Option<Uuid>,
}

#[derive(Args)]
pub struct JobsArgs {
    /// Filter by status: detected, ripping, encoding, tagging, done, failed
    #[arg(long)]
    status: Option<String>,

    /// Show last N jobs
    #[arg(long, default_value = "20")]
    limit: i64,
}

pub async fn run(cmd: &RipCommands, cfg: &AppConfig, pool: Db) -> Result<()> {
    match cmd {
        RipCommands::Daemon => {
            println!("Starting ripper daemon...");
            println!("Insert a CD to begin automatic ripping.");
            println!("Press Ctrl+C to stop.\n");
            run_daemon(cfg.clone(), pool).await?;
        }

        RipCommands::Manual(args) => {
            println!("Manual rip from: {}", args.drive);

            info!("Looking up disc in MusicBrainz...");
            let disc = musicbrainz::lookup_disc_toc(&args.drive, cfg).await?;

            if let Some(title) = &disc.title {
                println!("Disc identified: {}", title);
                if let Some(artist) = &disc.artist {
                    println!("Artist:          {artist}");
                }
                if let Some(count) = disc.track_count {
                    println!("Tracks:          {count}");
                }
            } else {
                println!("Disc not identified in MusicBrainz - will use generic track names.");
            }

            println!();
            println!("Starting rip pipeline...");

            let job_id = Uuid::new_v4();

            // Create rip job record
            let job = mm_db::models::RipJob {
                id: job_id,
                watchlist_id: args.watchlist_id,
                release_id: None,
                disc_id: disc.disc_id.clone(),
                musicbrainz_id: disc.musicbrainz_release_id,
                status: "detected".to_owned(),
                drive_path: args.drive.clone(),
                backend: if cfg!(target_os = "linux") {
                    cfg.ripper.backend_linux.clone()
                } else {
                    cfg.ripper.backend_windows.clone()
                },
                temp_dir: cfg.ripper.temp_dir.clone(),
                output_dir: Some(cfg.storage.local_path.clone()),
                track_count: disc.track_count.map(|n| n as i32),
                error_msg: None,
                accuraterip_status: None,
                started_at: chrono::Utc::now(),
                finished_at: None,
            };

            mm_db::queries::create_rip_job(&pool, &job).await?;

            let pipeline = RipPipeline::new(cfg.clone(), job_id, pool.clone());
            let result = pipeline.run(&args.drive, &disc).await?;

            println!();
            println!("Rip complete!");
            println!("Output directory: {}", result.output_dir);
            println!("Tracks:");
            for t in &result.tracks {
                println!("  {:02}. {}", t.number, t.title);
            }
        }

        RipCommands::Jobs(args) => {
            #[derive(sqlx::FromRow)]
            struct JobRow {
                id: uuid::Uuid,
                status: String,
                drive_path: String,
                track_count: Option<i32>,
                started_at: chrono::DateTime<chrono::Utc>,
                error_msg: Option<String>,
            }

            let rows = sqlx::query_as::<_, JobRow>(
                r#"
                SELECT id, status, drive_path, track_count, started_at, error_msg
                FROM rip_jobs
                ORDER BY started_at DESC
                LIMIT $1
                "#,
            )
            .bind(args.limit)
            .fetch_all(&pool)
            .await?;

            if rows.is_empty() {
                println!("No rip jobs found.");
                return Ok(());
            }

            println!(
                "{:<36}  {:<10}  {:<15}  {:<8}  {}",
                "ID", "Status", "Drive", "Tracks", "Started"
            );
            println!("{}", "─".repeat(90));

            for row in &rows {
                println!(
                    "{:<36}  {:<10}  {:<15}  {:<8}  {}",
                    row.id,
                    row.status,
                    row.drive_path,
                    row.track_count.map(|n| n.to_string()).unwrap_or_else(|| "?".to_owned()),
                    row.started_at.format("%Y-%m-%d %H:%M"),
                );
                if let Some(err) = &row.error_msg {
                    println!("         Error: {err}");
                }
            }
        }
    }

    Ok(())
}
