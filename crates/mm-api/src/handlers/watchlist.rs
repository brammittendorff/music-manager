use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use mm_discogs::DiscogsClient;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::models::*;

use super::AppResult;

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
    .map_err(super::db_err)?;

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
