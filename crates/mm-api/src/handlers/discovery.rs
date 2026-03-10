use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use mm_copyright::estimate as estimate_copyright;
use mm_db::{models::Release, queries};
use mm_discogs::DiscogsClient;
use std::sync::{Arc, atomic::Ordering};
use tracing::{error, info};
use uuid::Uuid;

use crate::AppState;

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
