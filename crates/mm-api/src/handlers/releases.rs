use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use mm_db::queries;
use mm_discogs::DiscogsClient;
use std::sync::{Arc, atomic::Ordering};
use uuid::Uuid;

use crate::AppState;
use crate::models::*;

use super::{AppResult, db_err};

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
        release_title: Option<String>,
    }

    let rows = sqlx::query_as::<_, Row>(
        r#"SELECT rj.id, rj.status, rj.drive_path, rj.backend, rj.track_count, rj.output_dir,
                  rj.error_msg, rj.accuraterip_status, rj.started_at, rj.finished_at,
                  r.title as release_title
           FROM rip_jobs rj
           LEFT JOIN watchlist w ON rj.watchlist_id = w.id
           LEFT JOIN releases r ON w.release_id = r.id
           ORDER BY rj.started_at DESC LIMIT 50"#,
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
            "release_title": r.release_title,
        }))
        .collect();

    Ok(Json(result))
}
