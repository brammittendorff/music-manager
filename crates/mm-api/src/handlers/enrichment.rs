use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
};
use governor::{Quota, RateLimiter};
use mm_discogs::DiscogsClient;
use reqwest::Client;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;

// ─── Popularity scoring helpers ──────────────────────────────────────────────

fn compute_popularity_score(
    want: i32,
    have: i32,
    rating_avg: f64,
    rating_count: i32,
    lastfm_playcount: i32,
    has_wikipedia: bool,
) -> f64 {
    // Discogs component (70%)
    let want_norm = (((want as f64) + 1.0).ln() / (1000.0_f64).ln()).min(1.0);
    let have_norm = (((have as f64) + 1.0).ln() / (5000.0_f64).ln()).min(1.0);
    let rating_norm = if rating_count >= 3 {
        rating_avg / 5.0
    } else {
        0.0
    };
    let scarcity = if have > 0 {
        ((want as f64) / (have as f64)).min(1.0)
    } else {
        0.0
    };

    let discogs_score = 0.30 * want_norm + 0.20 * have_norm + 0.25 * rating_norm + 0.25 * scarcity;

    // Enrichment component (30%)
    let lastfm_norm = (((lastfm_playcount as f64) + 1.0).ln() / (100_000.0_f64).ln()).min(1.0);
    let wiki_norm = if has_wikipedia { 1.0 } else { 0.0 };

    let enrichment_score = 0.50 * lastfm_norm + 0.50 * wiki_norm;

    0.70 * discogs_score + 0.30 * enrichment_score
}

async fn fetch_lastfm_album_info(
    http: &Client,
    api_key: &str,
    artist: &str,
    album: &str,
    limiter: &governor::DefaultDirectRateLimiter,
) -> Option<(i32, i32)> {
    limiter.until_ready().await;

    let url = format!(
        "http://ws.audioscrobbler.com/2.0/?method=album.getinfo&api_key={}&artist={}&album={}&format=json",
        api_key,
        urlenccode(artist),
        urlenccode(album),
    );

    let resp = match http.get(&url).timeout(Duration::from_secs(15)).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("Last.fm request failed: {e}");
            return None;
        }
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return None,
    };

    let album_obj = json.get("album")?;
    let listeners = album_obj
        .get("listeners")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    let playcount = album_obj
        .get("playcount")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);

    Some((listeners, playcount))
}

async fn check_wikipedia(
    http: &Client,
    artist: &str,
    limiter: &governor::DefaultDirectRateLimiter,
) -> bool {
    // Check English Wikipedia
    if check_wikipedia_lang(http, artist, "en", limiter).await {
        return true;
    }
    // Check Dutch Wikipedia
    check_wikipedia_lang(http, artist, "nl", limiter).await
}

async fn check_wikipedia_lang(
    http: &Client,
    artist: &str,
    lang: &str,
    limiter: &governor::DefaultDirectRateLimiter,
) -> bool {
    limiter.until_ready().await;

    let url = format!(
        "https://{lang}.wikipedia.org/w/api.php?action=query&titles={}&format=json",
        urlenccode(artist),
    );

    let resp = match http.get(&url).timeout(Duration::from_secs(10)).send().await {
        Ok(r) => r,
        Err(_) => return false,
    };

    let text = match resp.text().await {
        Ok(t) => t,
        Err(_) => return false,
    };

    // If the response contains "missing", the page does not exist
    !text.contains("\"missing\"")
}

/// Simple percent-encoding for query params (mirrors mm-discogs)
fn urlenccode(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => result.push(b as char),
            b' ' => result.push('+'),
            _ => result.push_str(&format!("%{:02X}", b)),
        }
    }
    result
}

// ─── Enrich a single release (used by platform checker background task) ──────

pub async fn enrich_single_release(
    state: &Arc<AppState>,
    release: &mm_db::models::Release,
    discogs: &DiscogsClient,
    http: &Client,
    lastfm_limiter: &governor::DefaultDirectRateLimiter,
    wiki_limiter: &governor::DefaultDirectRateLimiter,
) {
    // Skip if already enriched
    let enriched: bool = sqlx::query_scalar(
        "SELECT enriched_at IS NOT NULL FROM releases WHERE id = $1"
    )
    .bind(release.id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(true);

    if enriched {
        return;
    }

    let artist = release.artists.first().cloned().unwrap_or_default();

    // 1. Discogs community data
    let (want, have, rating_avg, rating_count) =
        match discogs.get_release(release.discogs_id as u32).await {
            Ok(json) => {
                let community = &json["community"];
                (
                    community["want"].as_i64().unwrap_or(0) as i32,
                    community["have"].as_i64().unwrap_or(0) as i32,
                    community["rating"]["average"].as_f64().unwrap_or(0.0),
                    community["rating"]["count"].as_i64().unwrap_or(0) as i32,
                )
            }
            Err(e) => {
                tracing::debug!("Enrichment: Discogs fetch failed for {}: {e}", release.id);
                (0, 0, 0.0, 0)
            }
        };

    // 2. Last.fm (if configured)
    let has_lastfm = !state.cfg.api.lastfm_api_key.is_empty();
    let (lastfm_listeners, lastfm_playcount) = if has_lastfm {
        fetch_lastfm_album_info(http, &state.cfg.api.lastfm_api_key, &artist, &release.title, lastfm_limiter)
            .await
            .unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    // 3. Wikipedia
    let has_wikipedia = check_wikipedia(http, &artist, wiki_limiter).await;

    // 4. Compute score
    let score = compute_popularity_score(want, have, rating_avg, rating_count, lastfm_playcount, has_wikipedia);

    // 5. Store
    sqlx::query(
        "UPDATE releases SET
            discogs_want = $1, discogs_have = $2,
            discogs_rating = $3, discogs_rating_count = $4,
            lastfm_listeners = $5, lastfm_playcount = $6,
            has_wikipedia = $7, popularity_score = $8,
            enriched_at = now()
         WHERE id = $9",
    )
    .bind(want)
    .bind(have)
    .bind(rating_avg as f32)
    .bind(rating_count)
    .bind(lastfm_listeners)
    .bind(lastfm_playcount)
    .bind(has_wikipedia)
    .bind(score as f32)
    .bind(release.id)
    .execute(&state.pool)
    .await
    .ok();
}

// ─── Background enrichment loop ─────────────────────────────────────────────

pub async fn run_enrichment_loop(state: Arc<AppState>) {
    info!("Enrichment background loop started");

    let http = match Client::builder()
        .user_agent("music-manager/0.1 +https://github.com/brammittendorff/music-manager")
        .timeout(Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            error!("Enrichment loop: failed to create HTTP client: {e}");
            return;
        }
    };

    let discogs = match DiscogsClient::new(&state.cfg) {
        Ok(d) => d,
        Err(e) => {
            error!("Enrichment loop: failed to create Discogs client: {e}");
            return;
        }
    };

    let lastfm_limiter = RateLimiter::direct(
        Quota::per_second(NonZeroU32::new(4).unwrap()),
    );
    let wiki_limiter = RateLimiter::direct(
        Quota::per_second(NonZeroU32::new(1).unwrap()),
    );

    loop {
        // Find next un-enriched release
        let release = sqlx::query_as::<_, mm_db::models::Release>(
            "SELECT * FROM releases WHERE enriched_at IS NULL ORDER BY created_at ASC LIMIT 1"
        )
        .fetch_optional(&state.pool)
        .await;

        match release {
            Ok(Some(r)) => {
                enrich_single_release(&state, &r, &discogs, &http, &lastfm_limiter, &wiki_limiter).await;
                tokio::time::sleep(Duration::from_millis(2500)).await;
            }
            Ok(None) => {
                // All enriched, sleep and check again later
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            Err(e) => {
                warn!("Enrichment loop DB error: {e}");
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
}

// ─── POST /api/releases/enrich ───────────────────────────────────────────────

pub async fn enrich_releases(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    // Find releases that haven't been enriched yet
    #[derive(sqlx::FromRow)]
    struct EnrichRow {
        id: Uuid,
        discogs_id: i32,
        title: String,
        artists: Vec<String>,
    }

    let releases = match sqlx::query_as::<_, EnrichRow>(
        "SELECT id, discogs_id, title, artists FROM releases
         WHERE discogs_want IS NULL OR enriched_at IS NULL
         ORDER BY created_at ASC
         LIMIT 100",
    )
    .fetch_all(&s.pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    if releases.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "enriched": 0, "message": "No releases need enrichment" })),
        )
            .into_response();
    }

    let total = releases.len();

    // Spawn background task
    let pool = s.pool.clone();
    let cfg = s.cfg.clone();
    tokio::spawn(async move {
        let http = match Client::builder()
            .user_agent("music-manager/0.1 +https://github.com/brammittendorff/music-manager")
            .timeout(Duration::from_secs(30))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to create HTTP client for enrichment: {e}");
                return;
            }
        };

        let discogs = match DiscogsClient::new(&cfg) {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to create Discogs client for enrichment: {e}");
                return;
            }
        };

        // Rate limiters
        let lastfm_limiter = RateLimiter::direct(
            Quota::per_second(NonZeroU32::new(4).unwrap()),
        );
        let wiki_limiter = RateLimiter::direct(
            Quota::per_second(NonZeroU32::new(1).unwrap()),
        );

        let has_lastfm = !cfg.api.lastfm_api_key.is_empty();

        let mut enriched_count = 0usize;

        for release in &releases {
            let artist = release.artists.first().cloned().unwrap_or_default();

            // 1. Fetch Discogs community data
            let (want, have, rating_avg, rating_count) =
                match discogs.get_release(release.discogs_id as u32).await {
                    Ok(json) => {
                        let community = &json["community"];
                        let want = community["want"].as_i64().unwrap_or(0) as i32;
                        let have = community["have"].as_i64().unwrap_or(0) as i32;
                        let rating_avg = community["rating"]["average"]
                            .as_f64()
                            .unwrap_or(0.0);
                        let rating_count = community["rating"]["count"]
                            .as_i64()
                            .unwrap_or(0) as i32;
                        (want, have, rating_avg, rating_count)
                    }
                    Err(e) => {
                        warn!(
                            "Failed to fetch Discogs release {} for enrichment: {e}",
                            release.discogs_id
                        );
                        (0, 0, 0.0, 0)
                    }
                };

            // 2. Fetch Last.fm data (if API key configured)
            let (lastfm_listeners, lastfm_playcount) = if has_lastfm {
                fetch_lastfm_album_info(
                    &http,
                    &cfg.api.lastfm_api_key,
                    &artist,
                    &release.title,
                    &lastfm_limiter,
                )
                .await
                .unwrap_or((0, 0))
            } else {
                (0, 0)
            };

            // 3. Check Wikipedia
            let has_wikipedia = check_wikipedia(&http, &artist, &wiki_limiter).await;

            // 4. Compute popularity score
            let score = compute_popularity_score(
                want,
                have,
                rating_avg,
                rating_count,
                lastfm_playcount,
                has_wikipedia,
            );

            // 5. Store results
            if let Err(e) = sqlx::query(
                "UPDATE releases SET
                    discogs_want = $1,
                    discogs_have = $2,
                    discogs_rating = $3,
                    discogs_rating_count = $4,
                    lastfm_listeners = $5,
                    lastfm_playcount = $6,
                    has_wikipedia = $7,
                    popularity_score = $8,
                    enriched_at = now()
                 WHERE id = $9",
            )
            .bind(want)
            .bind(have)
            .bind(rating_avg as f32)
            .bind(rating_count)
            .bind(lastfm_listeners)
            .bind(lastfm_playcount)
            .bind(has_wikipedia)
            .bind(score as f32)
            .bind(release.id)
            .execute(&pool)
            .await
            {
                warn!("Failed to store enrichment for release {}: {e}", release.id);
            } else {
                enriched_count += 1;
            }

            // Pause between releases (2-3 seconds)
            tokio::time::sleep(Duration::from_millis(2500)).await;
        }

        info!("Enrichment complete: {enriched_count}/{total} releases enriched");
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "started": true,
            "releases_to_enrich": total,
            "message": "Enrichment started in background"
        })),
    )
        .into_response()
}
