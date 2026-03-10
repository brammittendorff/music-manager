use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
};
use governor::{Quota, RateLimiter};
use mm_db::queries;
use mm_discogs::DiscogsClient;
use mm_platforms::PlatformCoordinator;
use reqwest::Client;
use std::num::NonZeroU32;
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;

use super::enrichment::enrich_single_release;

// ─── GET /api/platform-checker/status ────────────────────────────────────────

pub async fn platform_checker_status(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let unchecked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM releases r WHERE NOT EXISTS (
            SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id
        )"
    ).fetch_one(&s.pool).await.unwrap_or(0);

    let total_checked: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT release_id) FROM platform_checks"
    ).fetch_one(&s.pool).await.unwrap_or(0);

    // Determine which platforms are configured
    let cfg = &s.cfg.api;
    let mut active_platforms: Vec<&str> = vec!["deezer", "apple_music", "bandcamp"];
    let mut skipped_platforms: Vec<&str> = vec![];
    if !cfg.spotify_client_id.is_empty() { active_platforms.push("spotify"); }
    else { skipped_platforms.push("spotify"); }
    if !cfg.youtube_api_key.is_empty() { active_platforms.push("youtube_music"); }
    else { skipped_platforms.push("youtube_music"); }

    Json(serde_json::json!({
        "active": s.platform_checker_active.load(Ordering::Relaxed),
        "unchecked_count": unchecked,
        "total_checked": total_checked,
        "active_platforms": active_platforms,
        "skipped_platforms": skipped_platforms,
    })).into_response()
}

// ─── POST /api/platform-checker/start ────────────────────────────────────────

pub async fn platform_checker_start(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    if s.platform_checker_active.load(Ordering::Relaxed) {
        return (StatusCode::CONFLICT, "Platform checker already running").into_response();
    }
    s.platform_checker_cancel.store(false, Ordering::Relaxed);
    s.platform_checker_active.store(true, Ordering::Relaxed);
    let handle = tokio::spawn(run_platform_checker(s.clone()));
    *s.platform_checker_handle.lock().await = Some(handle);
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

// ─── POST /api/platform-checker/stop ─────────────────────────────────────────

pub async fn platform_checker_stop(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    s.platform_checker_cancel.store(true, Ordering::Relaxed);
    // Abort immediately — don't wait for the next cancel-flag check
    if let Some(handle) = s.platform_checker_handle.lock().await.take() {
        handle.abort();
    }
    s.platform_checker_active.store(false, Ordering::Relaxed);
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

// ─── Background platform checker task ────────────────────────────────────────

pub async fn run_platform_checker(state: Arc<AppState>) {
    info!("Platform checker started");

    // Create coordinator ONCE - reused across all releases (shares token cache)
    let coordinator = match PlatformCoordinator::new(&state.cfg).await {
        Ok(c) => c,
        Err(e) => {
            warn!("Platform checker: failed to create coordinator: {e}");
            state.platform_checker_active.store(false, Ordering::Relaxed);
            return;
        }
    };
    let discogs = match DiscogsClient::new(&state.cfg) {
        Ok(d) => d,
        Err(e) => {
            warn!("Platform checker: failed to create Discogs client: {e}");
            state.platform_checker_active.store(false, Ordering::Relaxed);
            return;
        }
    };

    // Enrichment resources (reused across all releases)
    let enrich_http = Client::builder()
        .user_agent("music-manager/0.1 +https://github.com/music-manager")
        .timeout(Duration::from_secs(30))
        .local_address("0.0.0.0".parse().ok())
        .build()
        .ok();
    let lastfm_limiter = RateLimiter::direct(
        Quota::per_second(NonZeroU32::new(4).unwrap()),
    );
    let wiki_limiter = RateLimiter::direct(
        Quota::per_second(NonZeroU32::new(1).unwrap()),
    );

    loop {
        if state.platform_checker_cancel.load(Ordering::Relaxed) {
            break;
        }

        // Find next unchecked release, prioritized by popularity score (most popular first),
        // then newest. This ensures interesting releases get checked before obscure ones.
        let release = sqlx::query_as::<_, mm_db::models::Release>(
            "SELECT * FROM releases r
             WHERE NOT EXISTS (SELECT 1 FROM track_checks tc WHERE tc.release_id = r.id)
             ORDER BY r.popularity_score DESC NULLS LAST, r.created_at DESC
             LIMIT 1"
        )
        .fetch_optional(&state.pool)
        .await;

        match release {
            Ok(Some(r)) => {
                // Run platform checks, price fetch, and enrichment concurrently.
                // They hit mostly different APIs so they don't block each other.
                let platforms_fut = async {
                    if let Err(e) = check_release_platforms(&state, &r, &coordinator, &discogs).await {
                        warn!("Platform check failed for {}: {e}", r.id);
                    }
                };
                let price_fut = fetch_release_price(&state, &r, &discogs);
                let enrich_fut = async {
                    if let Some(ref http) = enrich_http {
                        enrich_single_release(&state, &r, &discogs, http, &lastfm_limiter, &wiki_limiter).await;
                    }
                };
                tokio::join!(platforms_fut, price_fut, enrich_fut);
                // Pause between releases - each platform has its own rate limiter
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Ok(None) => {
                info!("Platform checker: all releases checked, sleeping 60s");
                for _ in 0..60 {
                    if state.platform_checker_cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
            Err(e) => {
                warn!("Platform checker DB error: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    info!("Platform checker stopped");
    state.platform_checker_active.store(false, Ordering::Relaxed);
}

async fn fetch_release_price(
    state: &Arc<AppState>,
    release: &mm_db::models::Release,
    discogs: &DiscogsClient,
) {
    // Skip if we already have a price
    let has_price: bool = sqlx::query_scalar(
        "SELECT price_checked_at IS NOT NULL FROM releases WHERE id = $1"
    )
    .bind(release.id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(true); // default true to skip on error

    if has_price {
        return;
    }

    match discogs.get_marketplace_stats(release.discogs_id as u32).await {
        Ok(stats) => {
            let price = stats.lowest_price.as_ref().map(|p| p.value);
            sqlx::query(
                "UPDATE releases SET lowest_price_eur = $1, num_for_sale = $2, price_checked_at = now() WHERE id = $3"
            )
            .bind(price)
            .bind(stats.num_for_sale as i32)
            .bind(release.id)
            .execute(&state.pool)
            .await
            .ok();
        }
        Err(e) => {
            // Mark as checked even on error so we don't retry every loop
            tracing::debug!("Price fetch failed for {}: {e}", release.id);
            sqlx::query("UPDATE releases SET price_checked_at = now() WHERE id = $1")
                .bind(release.id)
                .execute(&state.pool)
                .await
                .ok();
        }
    }
}

async fn check_release_platforms(
    state: &Arc<AppState>,
    release: &mm_db::models::Release,
    coordinator: &PlatformCoordinator,
    discogs: &DiscogsClient,
) -> anyhow::Result<()> {
    // Verify the release still exists (it may have been deleted via Clear while queued)
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM releases WHERE id = $1)")
        .bind(release.id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(false);
    if !exists {
        tracing::debug!("Release {} was deleted, skipping platform check", release.id);
        return Ok(());
    }

    let artist = release.artists.first().cloned().unwrap_or_default();
    let title = &release.title;

    // Get tracklist; fall back to release title
    let track_titles = discogs.get_tracklist(release.discogs_id as u32).await;
    let search_titles = if track_titles.is_empty() {
        vec![title.clone()]
    } else {
        track_titles
    };

    // Deduplicate track titles to avoid checking the same title twice
    let search_titles = {
        let mut seen = std::collections::HashSet::new();
        search_titles.into_iter().filter(|t| {
            seen.insert(t.to_lowercase())
        }).collect::<Vec<_>>()
    };

    // Per-platform accumulator: (found, url, score)
    let mut platform_found: std::collections::HashMap<String, (bool, Option<String>, Option<f64>)> = std::collections::HashMap::new();
    // Platforms that returned transient errors (will be retried)
    let mut platform_errored: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    // Platforms fully resolved by check_album_tracks (no per-track fallback needed)
    let mut platform_album_tracks_done: std::collections::HashSet<String> = std::collections::HashSet::new();

    // ── Album-level check with tracklist fetch (2 API calls per platform) ─────
    // Try check_album_tracks first — returns per-track results from the album tracklist.
    let threshold = state.cfg.platforms.match_threshold;
    let album_tracks_tasks: Vec<_> = coordinator.checkers().iter().map(|checker| {
        let name = checker.name().to_owned();
        let fut = checker.check_album_tracks(&artist, title, &search_titles, threshold);
        async move {
            match tokio::time::timeout(std::time::Duration::from_secs(20), fut).await {
                Ok(Ok(Some(r))) => {
                    tracing::info!(platform = %name, album_found = r.album_found, tracks = r.track_matches.len(), "Album tracks check result");
                    Some(r)
                }
                Ok(Ok(None)) => None, // Platform doesn't support check_album_tracks
                Ok(Err(e)) => {
                    tracing::warn!(platform = %name, error = %e, "Album tracks check failed");
                    None
                }
                Err(_) => {
                    tracing::warn!(platform = %name, "Album tracks check timed out after 20s");
                    None
                }
            }
        }
    }).collect();

    let album_tracks_results = futures::future::join_all(album_tracks_tasks).await;
    for result in album_tracks_results.into_iter().flatten() {
        let platform_name = result.platform.clone();
        let any_found = result.track_matches.iter().any(|t| t.found);

        // Save per-track results from the album tracklist fetch
        for (track_number, tm) in result.track_matches.iter().enumerate() {
            sqlx::query(
                "INSERT INTO track_checks (release_id, track_title, track_number, platform, found, match_score, platform_url)
                 SELECT $1, $2, $3, $4, $5, $6, $7
                 WHERE EXISTS (SELECT 1 FROM releases WHERE id = $1)
                 ON CONFLICT (release_id, track_title, platform) DO UPDATE SET
                     found = EXCLUDED.found, match_score = EXCLUDED.match_score,
                     platform_url = EXCLUDED.platform_url, checked_at = now()"
            )
            .bind(release.id)
            .bind(&tm.track_title)
            .bind(track_number as i32)
            .bind(&platform_name)
            .bind(tm.found)
            .bind(tm.score)
            .bind(&tm.platform_url)
            .execute(&state.pool)
            .await.ok();
        }

        // Save aggregate result
        let best_track = result.track_matches.iter()
            .filter(|t| t.found)
            .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));

        platform_found.insert(platform_name.clone(), (
            any_found || result.album_found,
            best_track.and_then(|t| t.platform_url.clone()).or(result.album_url.clone()),
            best_track.and_then(|t| t.score),
        ));

        let check = mm_db::models::PlatformCheck {
            id: Uuid::new_v4(),
            release_id: release.id,
            platform: platform_name.clone(),
            found: any_found || result.album_found,
            error: false,
            match_score: best_track.and_then(|t| t.score),
            platform_url: best_track.and_then(|t| t.platform_url.clone()).or(result.album_url),
            checked_at: chrono::Utc::now(),
        };
        queries::upsert_platform_check(&state.pool, &check).await.ok();

        platform_album_tracks_done.insert(platform_name);
    }

    // ── Fallback: album-level check for platforms that don't support check_album_tracks ──
    let album_results = coordinator.check_albums(&artist, title, threshold).await;
    for result in &album_results {
        if platform_album_tracks_done.contains(&result.platform) {
            continue; // Already resolved by check_album_tracks
        }
        if result.error {
            platform_errored.insert(result.platform.clone(), true);
        } else {
            let entry = platform_found.entry(result.platform.clone()).or_insert((false, None, None));
            if result.found {
                entry.0 = true;
                entry.1 = result.match_result.as_ref().and_then(|m| m.platform_url.clone());
                entry.2 = result.match_result.as_ref().map(|m| m.score);
            }
        }
    }

    // Save album-level aggregate results (only for platforms not already handled)
    for (platform, (found, url, score)) in &platform_found {
        if platform_album_tracks_done.contains(platform) { continue; }
        let check = mm_db::models::PlatformCheck {
            id: Uuid::new_v4(),
            release_id: release.id,
            platform: platform.clone(),
            found: *found,
            error: false,
            match_score: *score,
            platform_url: url.clone(),
            checked_at: chrono::Utc::now(),
        };
        queries::upsert_platform_check(&state.pool, &check).await.ok();
    }
    for (platform, _) in &platform_errored {
        if platform_found.contains_key(platform) { continue; }
        let check = mm_db::models::PlatformCheck {
            id: Uuid::new_v4(),
            release_id: release.id,
            platform: platform.clone(),
            found: false,
            error: true,
            match_score: None,
            platform_url: None,
            checked_at: chrono::Utc::now(),
        };
        queries::upsert_platform_check(&state.pool, &check).await.ok();
    }

    // ── Per-platform independent pipelines ─────────────────────────────────────
    // Each platform processes all tracks independently at its own rate limit,
    // so fast platforms (Deezer ~1s) don't wait for slow ones (Bandcamp ~15s).
    let platform_tasks: Vec<_> = coordinator.checkers().iter().map(|checker| {
        let platform_name = checker.name().to_owned();
        let artist = artist.clone();
        let search_titles = search_titles.clone();
        let pool = state.pool.clone();
        let release_id = release.id;
        let cancel = state.platform_checker_cancel.clone();
        // Check if this platform was already resolved by check_album_tracks or album-level check
        let already_done = platform_album_tracks_done.contains(&platform_name);
        let already_found = platform_found.get(&platform_name).map(|e| e.0).unwrap_or(false);
        // If album-level check returned not-found and the platform opts in,
        // skip per-track checking (e.g. Bandcamp: slow scraping, unlikely to find tracks if album missing).
        let skip_tracks = checker.skip_tracks_if_album_not_found()
            && platform_found.get(&platform_name).map(|e| !e.0).unwrap_or(false)
            && !platform_errored.contains_key(&platform_name);

        async move {
            if already_done {
                return; // Skip — fully resolved by check_album_tracks
            }
            if already_found {
                return; // Skip — already resolved at album level
            }
            if skip_tracks {
                tracing::info!(platform = %platform_name, "Skipping track-level check: album not found on this platform");
                return;
            }

            let mut found = false;
            let mut best_url: Option<String> = None;
            let mut best_score: Option<f64> = None;
            let mut errored = false;

            for (track_number, search_title) in search_titles.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }

                let result = match tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    checker.check(&artist, search_title, threshold),
                ).await {
                    Ok(Ok(r)) => {
                        tracing::info!(platform = %platform_name, found = r.found, "Platform check result");
                        r
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(platform = %platform_name, error = %e, "Check failed");
                        mm_platforms::PlatformResult::errored(&platform_name)
                    }
                    Err(_) => {
                        tracing::warn!(platform = %platform_name, "Check timed out after 15s");
                        mm_platforms::PlatformResult::errored(&platform_name)
                    }
                };

                if result.error {
                    errored = true;
                    continue;
                }

                errored = false; // Clear error state on successful check

                // Save per-track result
                sqlx::query(
                    "INSERT INTO track_checks (release_id, track_title, track_number, platform, found, match_score, platform_url)
                     SELECT $1, $2, $3, $4, $5, $6, $7
                     WHERE EXISTS (SELECT 1 FROM releases WHERE id = $1)
                     ON CONFLICT (release_id, track_title, platform) DO UPDATE SET
                         found = EXCLUDED.found, match_score = EXCLUDED.match_score,
                         platform_url = EXCLUDED.platform_url, checked_at = now()"
                )
                .bind(release_id)
                .bind(search_title)
                .bind(track_number as i32)
                .bind(&platform_name)
                .bind(result.found)
                .bind(result.match_result.as_ref().map(|m| m.score))
                .bind(result.match_result.as_ref().and_then(|m| m.platform_url.clone()))
                .execute(&pool)
                .await.ok();

                if result.found && !found {
                    found = true;
                    best_url = result.match_result.as_ref().and_then(|m| m.platform_url.clone());
                    best_score = result.match_result.as_ref().map(|m| m.score);
                }

                // Save/update aggregate after each track so UI updates live
                let check = mm_db::models::PlatformCheck {
                    id: Uuid::new_v4(),
                    release_id,
                    platform: platform_name.clone(),
                    found,
                    error: false,
                    match_score: best_score,
                    platform_url: best_url.clone(),
                    checked_at: chrono::Utc::now(),
                };
                queries::upsert_platform_check(&pool, &check).await.ok();

                // Stop early if we found a match — no need to check remaining tracks
                if found {
                    break;
                }
            }

            // Final aggregate: save errored state if never succeeded
            if errored && !found {
                let check = mm_db::models::PlatformCheck {
                    id: Uuid::new_v4(),
                    release_id,
                    platform: platform_name.clone(),
                    found: false,
                    error: true,
                    match_score: None,
                    platform_url: None,
                    checked_at: chrono::Utc::now(),
                };
                queries::upsert_platform_check(&pool, &check).await.ok();
            }
        }
    }).collect();

    futures::future::join_all(platform_tasks).await;

    info!("Checked platforms for release {} ({})", release.id, title);
    Ok(())
}

// ─── Watchdog: auto-starts platform checker ───────────────────────────────────

pub async fn run_platform_checker_watchdog(state: Arc<AppState>) {
    loop {
        if !state.platform_checker_active.load(Ordering::Relaxed)
            && !state.platform_checker_cancel.load(Ordering::Relaxed)
        {
            state.platform_checker_active.store(true, Ordering::Relaxed);
            run_platform_checker(state.clone()).await;
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}
