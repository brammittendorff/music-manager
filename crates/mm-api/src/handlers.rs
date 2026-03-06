use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use mm_copyright::estimate as estimate_copyright;
use mm_db::{models::Release, queries};
use mm_discogs::DiscogsClient;
use mm_platforms::PlatformCoordinator;
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

    let rows = sqlx::query_as::<_, ReleaseRow>(
        r#"
        SELECT r.id, r.discogs_id, r.title, r.artists, r.label,
               r.country, r.country_code, r.year, r.genres, r.formats,
               r.discogs_url, r.copyright_status
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
        ORDER BY r.artists[1], r.title
        LIMIT $9 OFFSET $10
        "#,
    )
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

    let checks = sqlx::query_as::<_, (Uuid, String, bool, Option<f64>, Option<String>)>(
        "SELECT release_id, platform, found, match_score, platform_url
         FROM platform_checks WHERE release_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    // Group checks by release_id
    use std::collections::HashMap;
    let mut checks_map: HashMap<Uuid, Vec<serde_json::Value>> = HashMap::new();
    for (rid, platform, found, score, url) in checks {
        checks_map.entry(rid).or_default().push(serde_json::json!({
            "platform": platform,
            "found": found,
            "match_score": score,
            "platform_url": url,
        }));
    }

    let result: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            let platforms = checks_map.remove(&r.id).unwrap_or_default();
            let buy_url = DiscogsClient::buy_url(r.discogs_id as u32);
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
                year, genres, formats, discogs_url, copyright_status
         FROM releases WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?
    .ok_or((StatusCode::NOT_FOUND, "Release not found".to_string()))?;

    let platforms = sqlx::query_as::<_, PlatformCheckRow>(
        "SELECT platform, found, match_score, platform_url
         FROM platform_checks WHERE release_id = $1 ORDER BY platform",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    #[derive(sqlx::FromRow)]
    struct WlRow { id: Uuid, status: String }

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
        "watchlist_status": wl.map(|w| w.status),
    })))
}

// ─── GET /api/watchlist ───────────────────────────────────────────────────────

pub async fn watchlist(State(s): State<Arc<AppState>>) -> AppResult<Vec<WatchlistRow>> {
    let rows = sqlx::query_as::<_, WatchlistRow>(
        r#"
        SELECT w.id, w.release_id, w.status, w.buy_url, w.notes, w.added_at,
               r.title, r.artists, r.year, r.label,
               r.copyright_status, r.discogs_url
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
                Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
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
    tokio::spawn(run_platform_checker(s.clone()));
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

// ─── POST /api/platform-checker/stop ─────────────────────────────────────────

pub async fn platform_checker_stop(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    s.platform_checker_cancel.store(true, Ordering::Relaxed);
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

// ─── Background platform checker task ────────────────────────────────────────

pub async fn run_platform_checker(state: Arc<AppState>) {
    info!("Platform checker started");

    loop {
        if state.platform_checker_cancel.load(Ordering::Relaxed) {
            break;
        }

        // Find next unchecked release
        let release = sqlx::query_as::<_, mm_db::models::Release>(
            "SELECT * FROM releases r
             WHERE NOT EXISTS (SELECT 1 FROM platform_checks pc WHERE pc.release_id = r.id)
             ORDER BY r.created_at ASC
             LIMIT 1"
        )
        .fetch_optional(&state.pool)
        .await;

        match release {
            Ok(Some(r)) => {
                if let Err(e) = check_release_platforms(&state, &r).await {
                    warn!("Platform check failed for {}: {e}", r.id);
                }
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

async fn check_release_platforms(state: &Arc<AppState>, release: &mm_db::models::Release) -> anyhow::Result<()> {
    let coordinator = PlatformCoordinator::new(&state.cfg).await?;
    let discogs = DiscogsClient::new(&state.cfg)?;

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

    for (track_number, search_title) in search_titles.iter().enumerate() {
        if state.platform_checker_cancel.load(Ordering::Relaxed) {
            break;
        }

        let results = coordinator.check_all(&artist, search_title, state.cfg.platforms.match_threshold).await;

        let mut all_found = true;
        for result in &results {
            // Save per-track result
            sqlx::query(
                "INSERT INTO track_checks (release_id, track_title, track_number, platform, found, match_score, platform_url)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
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

            // Update aggregate
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
                match_score: *score,
                platform_url: url.clone(),
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

// ─── GET /api/rip-jobs ────────────────────────────────────────────────────────

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
