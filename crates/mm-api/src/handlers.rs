use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use governor::{Quota, RateLimiter};
use mm_copyright::estimate as estimate_copyright;
use mm_db::{models::Release, queries};
use mm_discogs::DiscogsClient;
use mm_platforms::PlatformCoordinator;
use reqwest::Client;
use std::num::NonZeroU32;
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::models::*;

type AppResult<T> = Result<Json<T>, (StatusCode, String)>;

fn db_err(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

// ─── GET /api/stats ──────────────────────────────────────────────────────────

pub async fn stats(State(s): State<Arc<AppState>>) -> AppResult<StatsResponse> {
    let pool = &s.pool;

    let total_releases: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM releases")
        .fetch_one(pool).await.map_err(db_err)?;

    let public_domain: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM releases WHERE copyright_status IN ('PUBLIC_DOMAIN','LIKELY_PUBLIC_DOMAIN')"
    ).fetch_one(pool).await.map_err(db_err)?;

    let missing_from_streaming: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM (
            SELECT release_id FROM platform_checks
            GROUP BY release_id HAVING bool_and(NOT found)
        ) sub"#
    ).fetch_one(pool).await.map_err(db_err)?;

    let watchlist_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM watchlist")
        .fetch_one(pool).await.map_err(db_err)?;

    let watchlist_to_buy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM watchlist WHERE status = 'to_buy'"
    ).fetch_one(pool).await.map_err(db_err)?;

    let watchlist_done: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM watchlist WHERE status = 'done'"
    ).fetch_one(pool).await.map_err(db_err)?;

    let tracks_digitized: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks")
        .fetch_one(pool).await.map_err(db_err)?;

    let rip_jobs_done: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM rip_jobs WHERE status = 'done'"
    ).fetch_one(pool).await.map_err(db_err)?;

    Ok(Json(StatsResponse {
        total_releases,
        missing_from_streaming,
        public_domain,
        watchlist_total,
        watchlist_to_buy,
        watchlist_done,
        tracks_digitized,
        rip_jobs_done,
    }))
}

// ─── GET /api/releases ────────────────────────────────────────────────────────

pub async fn releases(
    State(s): State<Arc<AppState>>,
    Query(q): Query<ReleasesQuery>,
) -> AppResult<serde_json::Value> {
    let pool = &s.pool;

    let country_code = q.country.as_deref().unwrap_or("NL");
    let year_from = q.year_from.unwrap_or(1900);
    let year_to = q.year_to.unwrap_or(2100);
    let limit = q.limit.unwrap_or(100).min(500);
    let offset = q.offset.unwrap_or(0);
    let missing_only = q.missing_only.unwrap_or(false);

    let format_type = q.format_type.as_deref().unwrap_or("");
    let media = q.media.as_deref().unwrap_or("");
    let platform_status = q.platform_status.as_deref().unwrap_or("");
    // Parse comma-separated platforms e.g. "spotify,deezer"
    let platforms_vec: Vec<String> = q.platforms.as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let sort_by = q.sort_by.as_deref().unwrap_or("");

    let order_clause = match sort_by {
        "popularity" | "popularity_desc" => "ORDER BY r.popularity_score DESC NULLS LAST, r.artists[1], r.title",
        "popularity_asc" => "ORDER BY r.popularity_score ASC NULLS LAST, r.artists[1], r.title",
        "artist" | "artist_asc" => "ORDER BY r.artists[1] ASC, r.title ASC",
        "artist_desc" => "ORDER BY r.artists[1] DESC, r.title ASC",
        "title" | "title_asc" => "ORDER BY r.title ASC, r.artists[1] ASC",
        "title_desc" => "ORDER BY r.title DESC, r.artists[1] ASC",
        "year" | "year_desc" => "ORDER BY r.year DESC NULLS LAST, r.artists[1], r.title",
        "year_asc" => "ORDER BY r.year ASC NULLS LAST, r.artists[1], r.title",
        "label" | "label_asc" => "ORDER BY r.label ASC NULLS LAST, r.artists[1], r.title",
        "label_desc" => "ORDER BY r.label DESC NULLS LAST, r.artists[1], r.title",
        "copyright" | "copyright_asc" => "ORDER BY r.copyright_status ASC, r.artists[1], r.title",
        "copyright_desc" => "ORDER BY r.copyright_status DESC, r.artists[1], r.title",
        "format" | "format_asc" => "ORDER BY r.formats[1] ASC NULLS LAST, r.artists[1], r.title",
        "format_desc" => "ORDER BY r.formats[1] DESC NULLS LAST, r.artists[1], r.title",
        "price" | "price_asc" => "ORDER BY r.lowest_price_eur ASC NULLS LAST, r.artists[1], r.title",
        "price_desc" => "ORDER BY r.lowest_price_eur DESC NULLS LAST, r.artists[1], r.title",
        _ => "ORDER BY r.artists[1], r.title",
    };

    let query_str = format!(
        r#"
        SELECT r.id, r.discogs_id, r.title, r.artists, r.label,
               r.country, r.country_code, r.year, r.genres, r.formats,
               r.discogs_url, r.copyright_status,
               r.lowest_price_eur, r.num_for_sale,
               r.popularity_score, r.discogs_want, r.discogs_have,
               r.discogs_rating, r.discogs_rating_count,
               r.lastfm_listeners, r.lastfm_playcount, r.has_wikipedia
        FROM releases r
        WHERE r.country_code = $1
          AND (r.year IS NULL OR (r.year >= $2 AND r.year <= $3))
          AND ($4 = false OR NOT EXISTS (
              SELECT 1 FROM platform_checks pc
              WHERE pc.release_id = r.id AND pc.found = true
          ))
          AND ($5 = '' OR $5 = ANY(r.formats))
          AND ($6 = '' OR $6 = ANY(r.formats))
          AND (
              $7 = '' OR (
                  $7 = 'unchecked' AND NOT EXISTS (SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id)
              ) OR (
                  $7 = 'missing' AND EXISTS (SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id)
                      AND NOT EXISTS (SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id AND pc.found = true)
              ) OR (
                  $7 = 'found' AND EXISTS (SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id AND pc.found = true)
              )
          )
          AND (
              cardinality($8) = 0 OR (
                  SELECT COUNT(*) FROM unnest($8) AS p
                  WHERE EXISTS (
                      SELECT 1 FROM platform_checks pc
                      WHERE pc.release_id = r.id AND pc.platform = p AND pc.found = false
                  )
              ) = cardinality($8)
          )
        {order_clause}
        LIMIT $9 OFFSET $10
        "#,
    );

    let rows = sqlx::query_as::<_, ReleaseRow>(&query_str)
    .bind(country_code)
    .bind(year_from)
    .bind(year_to)
    .bind(missing_only)
    .bind(format_type)
    .bind(media)
    .bind(platform_status)
    .bind(&platforms_vec)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    // Fetch platform checks for all these releases in one query
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();

    let checks = sqlx::query_as::<_, (Uuid, String, bool, bool, Option<f64>, Option<String>)>(
        "SELECT release_id, platform, found, error, match_score, platform_url
         FROM platform_checks WHERE release_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    // Fetch watchlist status for all these releases in one query
    #[derive(sqlx::FromRow)]
    struct WlInfo {
        release_id: Uuid,
        id: Uuid,
        status: String,
    }

    let wl_rows = sqlx::query_as::<_, WlInfo>(
        "SELECT release_id, id, status FROM watchlist WHERE release_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    use std::collections::HashMap;

    // Group checks by release_id
    let mut checks_map: HashMap<Uuid, Vec<serde_json::Value>> = HashMap::new();
    for (rid, platform, found, error, score, url) in checks {
        checks_map.entry(rid).or_default().push(serde_json::json!({
            "platform": platform,
            "found": found,
            "error": error,
            "match_score": score,
            "platform_url": url,
        }));
    }

    // Group watchlist by release_id
    let mut wl_map: HashMap<Uuid, WlInfo> = HashMap::new();
    for wl in wl_rows {
        wl_map.insert(wl.release_id, wl);
    }

    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            let platforms = checks_map.remove(&r.id).unwrap_or_default();
            let buy_url = DiscogsClient::buy_url(r.discogs_id as u32);
            let wl = wl_map.get(&r.id);
            serde_json::json!({
                "id": r.id,
                "discogs_id": r.discogs_id,
                "title": r.title,
                "artists": r.artists,
                "label": r.label,
                "country_code": r.country_code,
                "year": r.year,
                "genres": r.genres,
                "formats": r.formats,
                "discogs_url": r.discogs_url,
                "buy_url": buy_url,
                "copyright_status": r.copyright_status,
                "platforms": platforms,
                "in_watchlist": wl.is_some(),
                "watchlist_id": wl.map(|w| w.id),
                "watchlist_status": wl.map(|w| &w.status),
                "lowest_price_eur": r.lowest_price_eur.as_ref().map(|p| p.to_string()),
                "num_for_sale": r.num_for_sale,
                "popularity_score": r.popularity_score,
                "discogs_want": r.discogs_want,
                "discogs_have": r.discogs_have,
            })
        })
        .collect();

    let total: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM releases r
           WHERE r.country_code = $1
             AND (r.year IS NULL OR (r.year >= $2 AND r.year <= $3))
             AND ($4 = false OR NOT EXISTS (
                 SELECT 1 FROM platform_checks pc
                 WHERE pc.release_id = r.id AND pc.found = true
             ))
             AND ($5 = '' OR $5 = ANY(r.formats))
             AND ($6 = '' OR $6 = ANY(r.formats))
             AND (
                 $7 = '' OR (
                     $7 = 'unchecked' AND NOT EXISTS (SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id)
                 ) OR (
                     $7 = 'missing' AND EXISTS (SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id)
                         AND NOT EXISTS (SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id AND pc.found = true)
                 ) OR (
                     $7 = 'found' AND EXISTS (SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id AND pc.found = true)
                 )
             )
             AND (
                 cardinality($8) = 0 OR (
                     SELECT COUNT(*) FROM unnest($8) AS p
                     WHERE EXISTS (
                         SELECT 1 FROM platform_checks pc
                         WHERE pc.release_id = r.id AND pc.platform = p AND pc.found = false
                     )
                 ) = cardinality($8)
             )"#,
    )
    .bind(country_code)
    .bind(year_from)
    .bind(year_to)
    .bind(missing_only)
    .bind(format_type)
    .bind(media)
    .bind(platform_status)
    .bind(&platforms_vec)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    Ok(Json(serde_json::json!({
        "data": result,
        "total": total,
        "limit": limit,
        "offset": offset,
    })))
}

// ─── GET /api/releases/:id ────────────────────────────────────────────────────

pub async fn release_detail(
    State(s): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> AppResult<serde_json::Value> {
    let pool = &s.pool;

    let release = sqlx::query_as::<_, ReleaseRow>(
        "SELECT id, discogs_id, title, artists, label, country, country_code,
                year, genres, formats, discogs_url, copyright_status,
                lowest_price_eur, num_for_sale,
                popularity_score, discogs_want, discogs_have,
                discogs_rating, discogs_rating_count,
                lastfm_listeners, lastfm_playcount, has_wikipedia
         FROM releases WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?
    .ok_or((StatusCode::NOT_FOUND, "Release not found".to_string()))?;

    let platforms = sqlx::query_as::<_, PlatformCheckRow>(
        "SELECT platform, found, error, match_score, platform_url
         FROM platform_checks WHERE release_id = $1 ORDER BY platform",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    #[derive(sqlx::FromRow)]
    struct WlRow {
        id: Uuid,
        status: String,
    }

    let wl = sqlx::query_as::<_, WlRow>(
        "SELECT id, status FROM watchlist WHERE release_id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let buy_url = DiscogsClient::buy_url(release.discogs_id as u32);

    Ok(Json(serde_json::json!({
        "id": release.id,
        "discogs_id": release.discogs_id,
        "title": release.title,
        "artists": release.artists,
        "label": release.label,
        "country_code": release.country_code,
        "year": release.year,
        "genres": release.genres,
        "formats": release.formats,
        "discogs_url": release.discogs_url,
        "buy_url": buy_url,
        "copyright_status": release.copyright_status,
        "platforms": platforms,
        "in_watchlist": wl.is_some(),
        "watchlist_id": wl.as_ref().map(|w| w.id),
        "watchlist_status": wl.as_ref().map(|w| &w.status),
        "lowest_price_eur": release.lowest_price_eur.as_ref().map(|p| p.to_string()),
        "num_for_sale": release.num_for_sale,
        "popularity_score": release.popularity_score,
        "discogs_want": release.discogs_want,
        "discogs_have": release.discogs_have,
        "discogs_rating": release.discogs_rating,
        "discogs_rating_count": release.discogs_rating_count,
        "lastfm_listeners": release.lastfm_listeners,
        "lastfm_playcount": release.lastfm_playcount,
        "has_wikipedia": release.has_wikipedia,
    })))
}

// ─── GET /api/watchlist ───────────────────────────────────────────────────────

pub async fn watchlist(State(s): State<Arc<AppState>>) -> AppResult<Vec<WatchlistRow>> {
    let rows = sqlx::query_as::<_, WatchlistRow>(
        r#"
        SELECT w.id, w.release_id, w.status, w.buy_url, w.notes, w.added_at,
               r.title, r.artists, r.year, r.label,
               r.copyright_status, r.discogs_url,
               w.lowest_price_eur, w.num_for_sale, w.skip_reason
        FROM watchlist w
        JOIN releases r ON r.id = w.release_id
        ORDER BY w.status, r.artists[1], r.title
        "#,
    )
    .fetch_all(&s.pool)
    .await
    .map_err(db_err)?;

    Ok(Json(rows))
}

// ─── POST /api/watchlist ──────────────────────────────────────────────────────

pub async fn add_to_watchlist(
    State(s): State<Arc<AppState>>,
    Json(body): Json<AddToWatchlistBody>,
) -> impl IntoResponse {
    let pool = &s.pool;

    let release = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM releases WHERE discogs_id = $1",
    )
    .bind(body.discogs_id)
    .fetch_optional(pool)
    .await;

    match release {
        Ok(Some((rid,))) => {
            let buy_url = DiscogsClient::buy_url(body.discogs_id as u32);
            match sqlx::query_scalar::<_, Uuid>(
                r#"INSERT INTO watchlist (release_id, buy_url, notes)
                   VALUES ($1, $2, $3)
                   ON CONFLICT (release_id) DO UPDATE SET updated_at = now()
                   RETURNING id"#,
            )
            .bind(rid)
            .bind(&buy_url)
            .bind(body.notes.as_deref())
            .fetch_one(pool)
            .await
            {
                Ok(id) => {
                    // Fetch marketplace price immediately in background
                    let cfg = s.cfg.clone();
                    let pool = pool.clone();
                    let discogs_id = body.discogs_id;
                    tokio::spawn(async move {
                        if let Ok(discogs) = DiscogsClient::new(&cfg) {
                            if let Ok(stats) = discogs.get_marketplace_stats(discogs_id as u32).await {
                                let price = stats.lowest_price.as_ref().map(|p| p.value);
                                sqlx::query(
                                    "UPDATE watchlist SET lowest_price_eur = $1, num_for_sale = $2,
                                     price_checked_at = now() WHERE id = $3"
                                )
                                .bind(price)
                                .bind(stats.num_for_sale as i32)
                                .bind(id)
                                .execute(&pool)
                                .await.ok();
                            }
                        }
                    });
                    (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
                }
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
        Ok(None) => (StatusCode::NOT_FOUND, "Release not found".to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── PATCH /api/watchlist/:id/status ─────────────────────────────────────────

pub async fn update_watchlist_status(
    State(s): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateStatusBody>,
) -> impl IntoResponse {
    let result = sqlx::query(
        "UPDATE watchlist SET status = $1, updated_at = now() WHERE id = $2",
    )
    .bind(&body.status)
    .bind(id)
    .execute(&s.pool)
    .await;

    match result {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── DELETE /api/watchlist/:id ────────────────────────────────────────────────

pub async fn delete_watchlist_item(
    State(s): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match sqlx::query("DELETE FROM watchlist WHERE id = $1")
        .bind(id)
        .execute(&s.pool)
        .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, "Watchlist item not found".to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── Discovery: request type ──────────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
pub struct StartJobRequest {
    pub country: Option<String>,
    pub country_code: Option<String>,
    pub genres: Option<Vec<String>>,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    pub format_filter: Option<String>,
    pub max_pages: Option<i32>,
}

// ─── GET /api/discovery/jobs ──────────────────────────────────────────────────

pub async fn discovery_list(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    #[derive(sqlx::FromRow, serde::Serialize)]
    struct Row {
        id: Uuid,
        status: String,
        country: String,
        country_code: String,
        genres: Vec<String>,
        year_from: i32,
        year_to: i32,
        format_filter: String,
        current_page: i32,
        total_pages: Option<i32>,
        releases_saved: i32,
        missing_count: i32,
        started_at: chrono::DateTime<chrono::Utc>,
        finished_at: Option<chrono::DateTime<chrono::Utc>>,
        error_msg: Option<String>,
        max_pages: Option<i32>,
    }

    let rows = sqlx::query_as::<_, Row>(
        "SELECT id, status, country, country_code, genres, year_from, year_to,
                format_filter, current_page, total_pages, releases_saved, missing_count,
                started_at, finished_at, error_msg, max_pages
         FROM discovery_jobs ORDER BY started_at DESC"
    ).fetch_all(&s.pool).await;

    match rows {
        Ok(r) => Json(serde_json::to_value(r).unwrap()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── POST /api/discovery/jobs ─────────────────────────────────────────────────

pub async fn discovery_create(
    State(s): State<Arc<AppState>>,
    Json(req): Json<StartJobRequest>,
) -> impl IntoResponse {
    if s.active_job_id.lock().await.is_some() {
        return (StatusCode::CONFLICT, "A job is already running").into_response();
    }
    s.discovery_cancel.store(false, Ordering::Relaxed);

    let country = req.country.unwrap_or_else(|| s.cfg.search.country.clone());
    let country_code = req.country_code.unwrap_or_else(|| s.cfg.search.country_code.clone());
    let genres: Vec<String> = req.genres.unwrap_or_else(|| s.cfg.search.genres.clone());
    let year_from = req.year_from.unwrap_or(s.cfg.search.year_from as i32);
    let year_to = req.year_to.unwrap_or(s.cfg.search.year_to as i32);
    let format_filter = req.format_filter.unwrap_or_else(|| s.cfg.search.format_filter.clone());
    let max_pages: Option<i32> = req.max_pages;

    let job_id: Uuid = match sqlx::query_scalar(
        "INSERT INTO discovery_jobs (country, country_code, genres, year_from, year_to, format_filter, max_pages)
         VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id"
    )
    .bind(&country)
    .bind(&country_code)
    .bind(&genres)
    .bind(year_from)
    .bind(year_to)
    .bind(&format_filter)
    .bind(max_pages)
    .fetch_one(&s.pool)
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    *s.active_job_id.lock().await = Some(job_id);
    let state = s.clone();
    tokio::spawn(run_discovery(state, job_id));
    info!("Discovery job {job_id} created and started");
    (StatusCode::OK, Json(serde_json::json!({ "id": job_id }))).into_response()
}

// ─── POST /api/discovery/jobs/:id/resume ─────────────────────────────────────

pub async fn discovery_resume(
    State(s): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if s.active_job_id.lock().await.is_some() {
        return (StatusCode::CONFLICT, "A job is already running").into_response();
    }

    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM discovery_jobs WHERE id = $1"
    )
    .bind(id)
    .fetch_optional(&s.pool)
    .await
    .unwrap_or(None);

    match status.as_deref() {
        Some("paused") | Some("failed") | Some("cancelled") => {}
        Some("completed") => return (StatusCode::BAD_REQUEST, "Job already completed").into_response(),
        Some("running") => return (StatusCode::CONFLICT, "Job already running").into_response(),
        _ => return (StatusCode::NOT_FOUND, "Job not found").into_response(),
    }

    sqlx::query(
        "UPDATE discovery_jobs SET status='running', finished_at=NULL, error_msg=NULL WHERE id=$1"
    )
    .bind(id)
    .execute(&s.pool)
    .await
    .ok();

    s.discovery_cancel.store(false, Ordering::Relaxed);
    *s.active_job_id.lock().await = Some(id);
    let state = s.clone();
    tokio::spawn(run_discovery(state, id));
    info!("Discovery job {id} resumed");
    (StatusCode::OK, Json(serde_json::json!({ "id": id }))).into_response()
}

// ─── POST /api/discovery/jobs/:id/pause ──────────────────────────────────────

pub async fn discovery_pause(
    State(s): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let active = *s.active_job_id.lock().await;
    if active != Some(id) {
        return (StatusCode::CONFLICT, "This job is not currently running").into_response();
    }
    // Signal pause - run_discovery will detect this and write status='paused'
    s.discovery_cancel.store(true, Ordering::Relaxed);
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

// ─── DELETE /api/discovery/jobs/:id ──────────────────────────────────────────

pub async fn discovery_delete_job(
    State(s): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if *s.active_job_id.lock().await == Some(id) {
        return (StatusCode::CONFLICT, "Cannot delete a running job - pause it first").into_response();
    }
    // Detach releases from this job (don't delete the releases themselves)
    sqlx::query("UPDATE releases SET discovery_job_id = NULL WHERE discovery_job_id = $1")
        .bind(id)
        .execute(&s.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM discovery_jobs WHERE id = $1")
        .bind(id)
        .execute(&s.pool)
        .await
        .ok();
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

// ─── GET /api/discovery/jobs/:id ─────────────────────────────────────────────

pub async fn discovery_job_status(
    State(s): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    #[derive(sqlx::FromRow, serde::Serialize)]
    struct Row {
        id: Uuid,
        status: String,
        country: String,
        country_code: String,
        genres: Vec<String>,
        year_from: i32,
        year_to: i32,
        format_filter: String,
        current_page: i32,
        total_pages: Option<i32>,
        releases_saved: i32,
        missing_count: i32,
        started_at: chrono::DateTime<chrono::Utc>,
        finished_at: Option<chrono::DateTime<chrono::Utc>>,
        error_msg: Option<String>,
        max_pages: Option<i32>,
    }

    let row = sqlx::query_as::<_, Row>(
        "SELECT id, status, country, country_code, genres, year_from, year_to,
                format_filter, current_page, total_pages, releases_saved, missing_count,
                started_at, finished_at, error_msg, max_pages
         FROM discovery_jobs WHERE id = $1"
    )
    .bind(id)
    .fetch_optional(&s.pool)
    .await;

    match row {
        Ok(Some(r)) => (StatusCode::OK, Json(serde_json::to_value(r).unwrap())).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Job not found".to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── DELETE /api/discovery - clear all discovery data ────────────────────────

pub async fn discovery_clear(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    if s.active_job_id.lock().await.is_some() {
        return (StatusCode::CONFLICT, "Discovery job is currently running".to_string()).into_response();
    }
    if s.platform_checker_active.load(Ordering::Relaxed) {
        return (StatusCode::CONFLICT, "Platform checker is running - stop it first".to_string()).into_response();
    }

    let pool = &s.pool;

    if let Err(e) = sqlx::query("DELETE FROM discovery_jobs").execute(pool).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    if let Err(e) = sqlx::query("DELETE FROM platform_checks").execute(pool).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    if let Err(e) = sqlx::query("DELETE FROM releases").execute(pool).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

// ─── Background discovery task ────────────────────────────────────────────────

pub async fn run_discovery(state: Arc<AppState>, job_id: Uuid) {
    let cancelled = match do_discovery(&state, job_id).await {
        Ok(cancelled) => cancelled,
        Err(e) => {
            error!("Discovery job {job_id} failed: {e}");
            let _ = sqlx::query(
                "UPDATE discovery_jobs SET status='failed', error_msg=$1, finished_at=now() WHERE id=$2",
            )
            .bind(e.to_string())
            .bind(job_id)
            .execute(&state.pool)
            .await;
            *state.active_job_id.lock().await = None;
            return;
        }
    };

    // Write 'paused' when cancelled via flag so user can resume.
    let status = if cancelled { "paused" } else { "completed" };
    let _ = sqlx::query(
        "UPDATE discovery_jobs SET status=$1, finished_at=now() WHERE id=$2",
    )
    .bind(status)
    .bind(job_id)
    .execute(&state.pool)
    .await;

    info!("Discovery job {job_id} {status}");
    *state.active_job_id.lock().await = None;
}

/// Returns `Ok(true)` if cancelled/paused, `Ok(false)` if completed normally.
async fn do_discovery(state: &Arc<AppState>, job_id: Uuid) -> anyhow::Result<bool> {
    // Load job filters from DB
    #[derive(sqlx::FromRow)]
    struct JobRow {
        country: String,
        country_code: String,
        genres: Vec<String>,
        year_from: i32,
        year_to: i32,
        format_filter: String,
        current_page: i32,
        max_pages: Option<i32>,
    }

    let job = sqlx::query_as::<_, JobRow>(
        "SELECT country, country_code, genres, year_from, year_to, format_filter, current_page, max_pages
         FROM discovery_jobs WHERE id = $1"
    )
    .bind(job_id)
    .fetch_one(&state.pool)
    .await?;

    // Build a per-job config override
    let mut job_cfg = state.cfg.clone();
    job_cfg.search.country = job.country.clone();
    job_cfg.search.country_code = job.country_code.clone();
    job_cfg.search.genres = job.genres.clone();
    job_cfg.search.year_from = job.year_from as u32;
    job_cfg.search.year_to = job.year_to as u32;
    job_cfg.search.format_filter = job.format_filter.clone();
    let effective_max_pages = job.max_pages.map(|p| p as u32).unwrap_or(state.cfg.search.max_pages);
    job_cfg.search.max_pages = effective_max_pages;

    let discogs = DiscogsClient::new(&state.cfg)?;

    // Resume from the last completed page (current_page is the last page processed)
    let mut page = (job.current_page + 1) as u32;
    let mut total_pages;
    let mut total_saved = 0i64;

    loop {
        if state.discovery_cancel.load(Ordering::Relaxed) {
            return Ok(true);
        }

        let (releases, tp) = discogs.search_releases_page(&job_cfg, page).await?;
        total_pages = tp;

        for dr in &releases {
            if state.discovery_cancel.load(Ordering::Relaxed) {
                return Ok(true);
            }
            let (artist, title) = dr.split_artist_title();
            let year = dr.year_as_i32();
            let (copyright_status, copyright_note) = estimate_copyright(year, &state.cfg.copyright);
            let release = Release {
                id: Uuid::new_v4(),
                discogs_id: dr.id as i32,
                title: title.clone(),
                artists: if artist.is_empty() { vec!["Unknown".to_owned()] } else { vec![artist.clone()] },
                label: dr.primary_label(),
                catalog_number: dr.catno.clone(),
                country: job_cfg.search.country.clone(),
                country_code: job_cfg.search.country_code.clone(),
                year,
                genres: dr.genres_vec(),
                styles: dr.styles_vec(),
                formats: dr.formats_vec(),
                discogs_url: dr.discogs_url(),
                thumb_url: dr.thumb.clone(),
                musicbrainz_id: None,
                copyright_status: copyright_status.to_string(),
                copyright_note: Some(copyright_note),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                discovery_job_id: None,
            };
            let release_id = queries::upsert_release(&state.pool, &release).await?;
            sqlx::query("UPDATE releases SET discovery_job_id = $1 WHERE id = $2")
                .bind(job_id).bind(release_id).execute(&state.pool).await.ok();
            total_saved += 1;
        }

        // Update progress after each page.
        sqlx::query(
            "UPDATE discovery_jobs SET current_page=$1, total_pages=$2, \
             releases_saved=$3 WHERE id=$4",
        )
        .bind(page as i32)
        .bind(total_pages as i32)
        .bind(total_saved as i32)
        .bind(job_id)
        .execute(&state.pool)
        .await?;

        info!("Discovery page {page}/{total_pages}: {total_saved} saved");

        if page >= total_pages || page >= effective_max_pages {
            break;
        }
        page += 1;
    }

    Ok(false)
}

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

async fn enrich_single_release(
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

    // Per-platform accumulator: (found, url, score)
    let mut platform_found: std::collections::HashMap<String, (bool, Option<String>, Option<f64>)> = std::collections::HashMap::new();
    // Platforms that returned transient errors (will be retried)
    let mut platform_errored: std::collections::HashMap<String, bool> = std::collections::HashMap::new();

    // ── Album-level check first (e.g. Spotify uses 1 API call instead of N) ──
    let album_results = coordinator.check_albums(&artist, title, state.cfg.platforms.match_threshold).await;
    for result in &album_results {
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

    // Save album-level aggregate results
    for (platform, (found, url, score)) in &platform_found {
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

    for (track_number, search_title) in search_titles.iter().enumerate() {
        if state.platform_checker_cancel.load(Ordering::Relaxed) {
            break;
        }

        let results = coordinator.check_all(&artist, search_title, state.cfg.platforms.match_threshold).await;

        let mut all_found = true;
        for result in &results {
            // Skip platforms already resolved at album level
            if platform_found.get(&result.platform).map(|e| e.0).unwrap_or(false) {
                continue;
            }

            // Skip saving errored results — they'll be retried
            if result.error {
                // Mark this platform as errored in the aggregate if not already found
                let entry = platform_errored.entry(result.platform.clone()).or_insert(true);
                if !platform_found.contains_key(&result.platform) {
                    all_found = false;
                }
                // Still save an errored aggregate so the UI can show it
                if !platform_found.contains_key(&result.platform) {
                    *entry = true;
                }
                continue;
            }

            // Save per-track result (SELECT ... WHERE EXISTS prevents FK violation if release was deleted)
            sqlx::query(
                "INSERT INTO track_checks (release_id, track_title, track_number, platform, found, match_score, platform_url)
                 SELECT $1, $2, $3, $4, $5, $6, $7
                 WHERE EXISTS (SELECT 1 FROM releases WHERE id = $1)
                 ON CONFLICT (release_id, track_title, platform) DO UPDATE SET
                     found = EXCLUDED.found, match_score = EXCLUDED.match_score,
                     platform_url = EXCLUDED.platform_url, checked_at = now()"
            )
            .bind(release.id)
            .bind(search_title)
            .bind(track_number as i32)
            .bind(&result.platform)
            .bind(result.found)
            .bind(result.match_result.as_ref().map(|m| m.score))
            .bind(result.match_result.as_ref().and_then(|m| m.platform_url.clone()))
            .execute(&state.pool)
            .await.ok();

            // Update aggregate — clear any previous error state
            platform_errored.remove(&result.platform);
            let entry = platform_found.entry(result.platform.clone()).or_insert((false, None, None));
            if result.found && !entry.0 {
                entry.0 = true;
                entry.1 = result.match_result.as_ref().and_then(|m| m.platform_url.clone());
                entry.2 = result.match_result.as_ref().map(|m| m.score);
            }
            if !platform_found.get(&result.platform).map(|e| e.0).unwrap_or(false) {
                all_found = false;
            }
        }

        // Save aggregate platform check after each track
        for (platform, (found, url, score)) in &platform_found {
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
        // Save errored platforms so UI can show them distinctly
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

        if all_found && !platform_found.is_empty() {
            break;
        }
    }

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

// ─── Watchlist automation: price checks + auto-skip ──────────────────────────

pub async fn run_watchlist_automation(state: Arc<AppState>) {
    info!("Watchlist automation started");

    let discogs = match DiscogsClient::new(&state.cfg) {
        Ok(d) => d,
        Err(e) => {
            warn!("Watchlist automation: failed to create Discogs client: {e}");
            return;
        }
    };

    loop {
        // 1) Update marketplace prices for "to_buy" items (oldest price first)
        let to_buy_items = sqlx::query_as::<_, (Uuid, i32)>(
            "SELECT w.id, r.discogs_id FROM watchlist w
             JOIN releases r ON r.id = w.release_id
             WHERE w.status = 'to_buy'
               AND (w.price_checked_at IS NULL OR w.price_checked_at < now() - interval '24 hours')
             ORDER BY w.price_checked_at NULLS FIRST
             LIMIT 10"
        )
        .fetch_all(&state.pool)
        .await;

        if let Ok(items) = to_buy_items {
            for (wl_id, discogs_id) in &items {
                match discogs.get_marketplace_stats(*discogs_id as u32).await {
                    Ok(stats) => {
                        let price = stats.lowest_price.as_ref().map(|p| p.value);
                        sqlx::query(
                            "UPDATE watchlist SET lowest_price_eur = $1, num_for_sale = $2,
                             price_checked_at = now() WHERE id = $3"
                        )
                        .bind(price)
                        .bind(stats.num_for_sale as i32)
                        .bind(wl_id)
                        .execute(&state.pool)
                        .await.ok();
                    }
                    Err(e) => {
                        warn!("Marketplace stats failed for discogs_id {discogs_id}: {e}");
                        // Still update checked_at so we don't hammer it
                        sqlx::query("UPDATE watchlist SET price_checked_at = now() WHERE id = $1")
                            .bind(wl_id).execute(&state.pool).await.ok();
                    }
                }
            }
        }

        // 2) Auto-skip watchlist items that are now on streaming platforms
        let watchlist_release_ids = sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT w.id, w.release_id FROM watchlist w
             WHERE w.status NOT IN ('done', 'skipped')
               AND EXISTS (
                   SELECT 1 FROM platform_checks pc
                   WHERE pc.release_id = w.release_id
                     AND pc.found = true AND pc.error = false
               )"
        )
        .fetch_all(&state.pool)
        .await;

        if let Ok(items) = watchlist_release_ids {
            for (wl_id, release_id) in &items {
                // Get which platforms found it
                let platforms: Vec<String> = sqlx::query_scalar(
                    "SELECT platform FROM platform_checks
                     WHERE release_id = $1 AND found = true AND error = false"
                )
                .bind(release_id)
                .fetch_all(&state.pool)
                .await
                .unwrap_or_default();

                let reason = format!("found_on_streaming: {}", platforms.join(", "));
                info!("Auto-skipping watchlist item {wl_id}: {reason}");

                sqlx::query(
                    "UPDATE watchlist SET status = 'skipped', skip_reason = $1, updated_at = now()
                     WHERE id = $2"
                )
                .bind(&reason)
                .bind(wl_id)
                .execute(&state.pool)
                .await.ok();
            }
        }

        // Run every 5 minutes
        tokio::time::sleep(Duration::from_secs(300)).await;
    }
}

// ─── GET /api/releases/export ─────────────────────────────────────────────────

pub async fn export_releases(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let releases = sqlx::query_as::<_, mm_db::models::Release>(
        "SELECT * FROM releases ORDER BY artists[1], title"
    )
    .fetch_all(&s.pool)
    .await;

    match releases {
        Ok(rows) => {
            let json = serde_json::to_string(&rows).unwrap_or_default();
            (
                StatusCode::OK,
                [
                    ("Content-Type", "application/json"),
                    ("Content-Disposition", "attachment; filename=\"releases.json\""),
                ],
                json,
            ).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── POST /api/releases/import ────────────────────────────────────────────────

pub async fn import_releases(
    State(s): State<Arc<AppState>>,
    Json(releases): Json<Vec<mm_db::models::Release>>,
) -> impl IntoResponse {
    let pool = &s.pool;
    let mut imported = 0usize;

    for r in &releases {
        match queries::upsert_release(pool, r).await {
            Ok(_) => imported += 1,
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string(), "imported": imported }))
                ).into_response()
            }
        }
    }

    (StatusCode::OK, Json(serde_json::json!({ "imported": imported }))).into_response()
}

// ─── DELETE /api/releases/clear ───────────────────────────────────────────────

pub async fn clear_releases(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    if s.active_job_id.lock().await.is_some() {
        return (StatusCode::CONFLICT, "A discovery job is running - pause it first").into_response();
    }
    if s.platform_checker_active.load(Ordering::Relaxed) {
        return (StatusCode::CONFLICT, "Platform checker is running - stop it first").into_response();
    }
    let pool = &s.pool;
    // Must delete dependent tables first
    if let Err(e) = sqlx::query("DELETE FROM platform_checks").execute(pool).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    if let Err(e) = sqlx::query("DELETE FROM watchlist").execute(pool).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    if let Err(e) = sqlx::query("DELETE FROM releases").execute(pool).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

// ─── GET /api/releases/:id/tracks ────────────────────────────────────────────

pub async fn release_tracks(
    State(s): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    #[derive(sqlx::FromRow, serde::Serialize)]
    struct Row {
        track_title:  String,
        track_number: Option<i32>,
        platform:     String,
        found:        bool,
        match_score:  Option<f64>,
        platform_url: Option<String>,
    }

    let rows = sqlx::query_as::<_, Row>(
        "SELECT track_title, track_number, platform, found, match_score, platform_url
         FROM track_checks
         WHERE release_id = $1
         ORDER BY track_number, track_title, platform"
    )
    .bind(id)
    .fetch_all(&s.pool)
    .await;

    match rows {
        Ok(r) => Json(serde_json::to_value(r).unwrap()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── GET /api/rip-jobs ────────────────────────────────────────────────────────

pub async fn rip_jobs(State(s): State<Arc<AppState>>) -> AppResult<Vec<serde_json::Value>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        status: String,
        drive_path: String,
        backend: String,
        track_count: Option<i32>,
        output_dir: Option<String>,
        error_msg: Option<String>,
        accuraterip_status: Option<String>,
        started_at: chrono::DateTime<chrono::Utc>,
        finished_at: Option<chrono::DateTime<chrono::Utc>>,
    }

    let rows = sqlx::query_as::<_, Row>(
        r#"SELECT id, status, drive_path, backend, track_count, output_dir,
                  error_msg, accuraterip_status, started_at, finished_at
           FROM rip_jobs ORDER BY started_at DESC LIMIT 50"#,
    )
    .fetch_all(&s.pool)
    .await
    .map_err(db_err)?;

    let result = rows
        .into_iter()
        .map(|r| serde_json::json!({
            "id": r.id,
            "status": r.status,
            "drive_path": r.drive_path,
            "backend": r.backend,
            "track_count": r.track_count,
            "output_dir": r.output_dir,
            "error_msg": r.error_msg,
            "accuraterip_status": r.accuraterip_status,
            "started_at": r.started_at,
            "finished_at": r.finished_at,
        }))
        .collect();

    Ok(Json(result))
}

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
            .user_agent("music-manager/0.1 +https://github.com/music-manager")
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
