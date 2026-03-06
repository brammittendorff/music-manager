//! Background worker that pulls releases from the database and checks their
//! availability on configured streaming/download platforms.
//!
//! Usage:
//!   mm worker                    # default: 4 workers, batch of 50
//!   mm worker --workers 2 --batch-size 100

use anyhow::Result;
use chrono::Utc;
use clap::Args;
use mm_config::AppConfig;
use mm_db::models::Release;
use mm_platforms::{
    rate_limits,
    PlatformCoordinator,
};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::sleep;
use tracing::{error, info, warn};
use uuid::Uuid;

// ─── CLI arguments ────────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct WorkerArgs {
    /// Number of concurrent worker tasks
    #[arg(long, default_value_t = 4)]
    pub workers: usize,

    /// Maximum releases to process per run
    #[arg(long, default_value_t = 50)]
    pub batch_size: i64,

    /// Re-check releases whose checks are older than this many days
    #[arg(long, default_value_t = 7)]
    pub stale_days: i64,

    /// Skip rate-limit delays (useful for testing; do NOT use in production)
    #[arg(long, hide = true)]
    pub no_rate_limit: bool,
}

// ─── Per-platform rate-limit delays ──────────────────────────────────────────

/// Return the appropriate inter-request delay for a named platform.
fn delay_for_platform(platform: &str) -> std::time::Duration {
    match platform {
        "discogs" => rate_limits::DISCOGS_DELAY,
        "spotify" => rate_limits::SPOTIFY_DELAY,
        "youtube_music" => rate_limits::YOUTUBE_DELAY,
        "deezer" => rate_limits::DEEZER_DELAY,
        "musicbrainz" => rate_limits::MUSICBRAINZ_DELAY,
        "apple_music" => rate_limits::ITUNES_DELAY,
        "bandcamp" => rate_limits::BANDCAMP_DELAY,
        // Unknown platforms: use a safe 1-second default
        _ => std::time::Duration::from_secs(1),
    }
}

// ─── Database helpers ─────────────────────────────────────────────────────────

/// Fetch releases that have no platform checks at all, or whose most recent
/// check is older than `stale_days` days.
async fn fetch_pending_releases(
    pool: &PgPool,
    batch_size: i64,
    stale_days: i64,
) -> Result<Vec<Release>> {
    let stale_cutoff = Utc::now() - chrono::Duration::days(stale_days);

    let rows = sqlx::query_as::<_, Release>(
        r#"
        SELECT r.*
        FROM releases r
        WHERE
            -- Either no checks exist yet
            NOT EXISTS (
                SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id
            )
            OR
            -- Or all checks are stale
            (
                SELECT MAX(pc.checked_at)
                FROM platform_checks pc
                WHERE pc.release_id = r.id
            ) < $1
        ORDER BY r.created_at ASC
        LIMIT $2
        "#,
    )
    .bind(stale_cutoff)
    .bind(batch_size)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Upsert a platform check result into the database.
async fn upsert_platform_check(
    pool: &PgPool,
    release_id: Uuid,
    platform: &str,
    found: bool,
    match_score: Option<f64>,
    platform_url: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO platform_checks (id, release_id, platform, found, match_score, platform_url, checked_at)
        VALUES ($1, $2, $3, $4, $5, $6, NOW())
        ON CONFLICT (release_id, platform)
        DO UPDATE SET
            found        = EXCLUDED.found,
            match_score  = EXCLUDED.match_score,
            platform_url = EXCLUDED.platform_url,
            checked_at   = EXCLUDED.checked_at
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(release_id)
    .bind(platform)
    .bind(found)
    .bind(match_score)
    .bind(platform_url)
    .execute(pool)
    .await?;

    Ok(())
}

// ─── Worker entry point ───────────────────────────────────────────────────────

pub async fn run(args: &WorkerArgs, cfg: &AppConfig, pool: &PgPool) -> Result<()> {
    info!(
        workers = args.workers,
        batch_size = args.batch_size,
        stale_days = args.stale_days,
        "Starting platform check worker"
    );

    // Build platform coordinator (logs which checkers are active at startup)
    let coordinator = Arc::new(PlatformCoordinator::new(cfg).await?);

    // Fetch the batch of releases to process
    let releases = fetch_pending_releases(pool, args.batch_size, args.stale_days).await?;

    if releases.is_empty() {
        info!("No releases need platform checks. All done.");
        return Ok(());
    }

    info!("Processing {} release(s)", releases.len());

    // Semaphore limits concurrent tasks to `--workers`
    let semaphore = Arc::new(Semaphore::new(args.workers));
    let pool = Arc::new(pool.clone());
    let no_rate_limit = args.no_rate_limit;

    let mut handles = Vec::with_capacity(releases.len());

    for (idx, release) in releases.into_iter().enumerate() {
        let coordinator = Arc::clone(&coordinator);
        let pool = Arc::clone(&pool);
        let permit = Arc::clone(&semaphore).acquire_owned().await?;

        let handle = tokio::spawn(async move {
            // permit is held for the duration of this task
            let _permit = permit;

            let artist = release.primary_artist().to_owned();
            let title = release.title.clone();
            let release_id = release.id;

            // Check all platforms
            let results = coordinator
                .check_all(&artist, &title, cfg_threshold())
                .await;

            let mut errors = 0usize;

            for result in &results {
                match upsert_platform_check(
                    &pool,
                    release_id,
                    &result.platform,
                    result.found,
                    result.match_result.as_ref().map(|m| m.score),
                    None, // URL extraction can be added per-platform later
                )
                .await
                {
                    Ok(()) => {}
                    Err(e) => {
                        error!(
                            release_id = %release_id,
                            platform = %result.platform,
                            error = %e,
                            "Failed to save platform check"
                        );
                        errors += 1;
                    }
                }

                // Respect per-platform rate limits between writes
                if !no_rate_limit {
                    sleep(delay_for_platform(&result.platform)).await;
                }
            }

            // Log progress every 10 releases
            if (idx + 1) % 10 == 0 {
                let found_count = results.iter().filter(|r| r.found).count();
                info!(
                    processed = idx + 1,
                    artist = %artist,
                    title = %title,
                    platforms_checked = results.len(),
                    found_on = found_count,
                    db_errors = errors,
                    "Worker progress"
                );
            }

            (results.len(), errors)
        });

        handles.push(handle);
    }

    // Collect results
    let mut total_checks = 0usize;
    let mut total_errors = 0usize;
    let mut task_failures = 0usize;

    for handle in handles {
        match handle.await {
            Ok((checks, errors)) => {
                total_checks += checks;
                total_errors += errors;
            }
            Err(e) => {
                warn!(error = %e, "Worker task panicked");
                task_failures += 1;
            }
        }
    }

    info!(
        total_checks,
        total_errors,
        task_failures,
        "Worker finished"
    );

    if total_errors > 0 || task_failures > 0 {
        warn!(
            "Completed with {} DB error(s) and {} task failure(s). \
             Check logs above for details.",
            total_errors, task_failures
        );
    } else {
        info!("All platform checks completed successfully.");
    }

    Ok(())
}

/// Default match threshold - could come from cfg in the future.
#[inline]
fn cfg_threshold() -> f64 {
    0.75
}
