pub mod accuraterip;
pub mod detector;
pub mod musicbrainz;
pub mod pipeline;

pub use detector::{DriveEvent, DriveWatcher};
pub use pipeline::{RipPipeline, RipResult};

use anyhow::Result;
use mm_config::AppConfig;
use mm_db::{models::RipJob, queries, Db};
use tracing::{error, info};
use uuid::Uuid;

/// Start the ripper daemon: watch for disc insertion and auto-rip.
pub async fn run_daemon(cfg: AppConfig, pool: Db) -> Result<()> {
    info!("Ripper daemon started. Watching for disc insertion...");

    let mut watcher = DriveWatcher::new(&cfg)?;

    loop {
        match watcher.next_event().await {
            Ok(DriveEvent::Inserted(drive_path)) => {
                info!("Disc inserted: {drive_path}");
                let cfg = cfg.clone();
                let pool = pool.clone();
                let drive = drive_path.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_disc_insert(&cfg, &pool, &drive).await {
                        error!("Rip failed for {drive}: {e:#}");
                    }
                });
            }
            Ok(DriveEvent::Ejected(drive_path)) => {
                info!("Disc ejected: {drive_path}");
            }
            Err(e) => {
                error!("Drive watcher error: {e:#}");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn handle_disc_insert(cfg: &AppConfig, pool: &Db, drive_path: &str) -> Result<()> {
    // 1. Compute disc TOC and look up MusicBrainz disc ID
    let disc_info = musicbrainz::lookup_disc_toc(drive_path, cfg).await?;
    info!("Disc identified: {:?}", disc_info.title);

    // 2. Try to match disc to a watchlist entry
    let watchlist_id = queries::list_watchlist(pool, Some("ready_to_rip"))
        .await?
        .into_iter()
        .find(|_w| {
            // Match by MusicBrainz ID if available, otherwise by release title heuristic
            disc_info.musicbrainz_release_id.map_or(false, |_mb_id| {
                // Would look up release.musicbrainz_id from DB - simplified here
                true
            })
        })
        .map(|w| w.id);

    let backend = if cfg!(target_os = "linux") {
        cfg.ripper.backend_linux.clone()
    } else {
        cfg.ripper.backend_windows.clone()
    };

    // 3. Create rip job record
    let job = RipJob {
        id: Uuid::new_v4(),
        watchlist_id,
        release_id: None,
        disc_id: disc_info.disc_id.clone(),
        musicbrainz_id: disc_info.musicbrainz_release_id,
        status: "detected".to_owned(),
        drive_path: drive_path.to_owned(),
        backend: backend.clone(),
        temp_dir: cfg.ripper.temp_dir.clone(),
        output_dir: Some(cfg.storage.local_path.clone()),
        track_count: disc_info.track_count.map(|n| n as i32),
        error_msg: None,
        accuraterip_status: None,
        started_at: chrono::Utc::now(),
        finished_at: None,
    };

    let job_id = queries::create_rip_job(pool, &job).await?;

    // 4. Run the rip pipeline
    let pipeline = RipPipeline::new(cfg.clone(), job_id, pool.clone());
    match pipeline.run(drive_path, &disc_info).await {
        Ok(result) => {
            info!("Rip complete: {} tracks → {}", result.tracks.len(), result.output_dir);
            queries::update_rip_job_status(pool, job_id, "done", None).await?;

            // 5. Advance watchlist status if matched
            if let Some(wl_id) = watchlist_id {
                queries::update_watchlist_status(
                    pool,
                    wl_id,
                    mm_db::models::WatchlistStatus::Done,
                )
                .await?;
            }
        }
        Err(e) => {
            error!("Rip pipeline error: {e:#}");
            queries::update_rip_job_status(pool, job_id, "failed", Some(&e.to_string())).await?;
        }
    }

    Ok(())
}
