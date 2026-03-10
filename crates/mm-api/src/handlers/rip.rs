use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
};
use mm_discogs::DiscogsClient;
use std::sync::Arc;
use tracing::{error, info};
use uuid::Uuid;

use crate::AppState;

// ─── GET /api/drives ─────────────────────────────────────────────────────────

/// Detect available CD/DVD drives and whether a disc is inserted.
pub async fn detect_drives() -> impl IntoResponse {
    let drives = scan_optical_drives().await;
    (StatusCode::OK, Json(drives))
}

#[derive(serde::Serialize)]
pub struct DriveInfo {
    pub path: String,
    pub has_media: bool,
    pub label: Option<String>,
}

async fn scan_optical_drives() -> Vec<DriveInfo> {
    let mut drives = Vec::new();

    // Check common Linux optical drive paths
    for path in &["/dev/cdrom", "/dev/sr0", "/dev/sr1", "/dev/dvd"] {
        if std::path::Path::new(path).exists() {
            // Check if media is present by trying to read block count
            let has_media = tokio::process::Command::new("blockdev")
                .args(["--getsize64", path])
                .output()
                .await
                .map(|o| o.status.success() && !o.stdout.is_empty())
                .unwrap_or(false);

            // Try to get volume label via blkid
            let label = if has_media {
                tokio::process::Command::new("blkid")
                    .args(["-s", "LABEL", "-o", "value", path])
                    .output()
                    .await
                    .ok()
                    .and_then(|o| {
                        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                        if s.is_empty() { None } else { Some(s) }
                    })
            } else {
                None
            };

            drives.push(DriveInfo {
                path: path.to_string(),
                has_media,
                label,
            });
        }
    }

    // Deduplicate (e.g. /dev/cdrom and /dev/sr0 may be the same device)
    // Keep unique by resolving symlinks
    let mut seen = std::collections::HashSet::new();
    drives.retain(|d| {
        let real = std::fs::canonicalize(&d.path)
            .unwrap_or_else(|_| std::path::PathBuf::from(&d.path));
        seen.insert(real)
    });

    drives
}

// ─── GET /api/watchlist/ready-to-rip ────────────────────────────────────────

/// List watchlist entries with status "ready_to_rip" — releases the user has
/// purchased and can now rip. Returns release info so user can select one.
pub async fn ready_to_rip(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    #[derive(sqlx::FromRow, serde::Serialize)]
    struct ReadyRelease {
        watchlist_id: Uuid,
        release_id: Uuid,
        discogs_id: i32,
        title: String,
        artists: Vec<String>,
        year: Option<i32>,
        label: Option<String>,
        thumb_url: Option<String>,
    }

    let rows = sqlx::query_as::<_, ReadyRelease>(
        r#"SELECT w.id as watchlist_id, r.id as release_id, r.discogs_id,
                  r.title, r.artists, r.year, r.label, r.thumb_url
           FROM watchlist w
           JOIN releases r ON w.release_id = r.id
           WHERE w.status = 'ready_to_rip'
           ORDER BY w.updated_at DESC"#,
    )
    .fetch_all(&s.pool)
    .await;

    match rows {
        Ok(r) => (StatusCode::OK, Json(serde_json::json!(r))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ─── POST /api/rip-jobs ──────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct StartRipRequest {
    /// Drive path, e.g. "/dev/sr0"
    pub drive_path: String,
    /// Discogs release ID for metadata (tracklist, cover art, etc.)
    pub discogs_id: i32,
    /// Optional watchlist entry to link and advance status
    pub watchlist_id: Option<Uuid>,
}

/// Start a rip job: rip the disc in the given drive using Discogs metadata.
pub async fn start_rip(
    State(s): State<Arc<AppState>>,
    Json(req): Json<StartRipRequest>,
) -> impl IntoResponse {
    let cfg = s.cfg.clone();
    let pool = s.pool.clone();

    // Validate drive exists
    if !std::path::Path::new(&req.drive_path).exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Drive not found: {}", req.drive_path) })),
        ).into_response();
    }

    let discogs_id = req.discogs_id as u32;
    let watchlist_id = req.watchlist_id;
    let drive_path = req.drive_path.clone();

    // Create the rip job record immediately so frontend can track it
    let job_id = Uuid::new_v4();
    let backend = cfg.ripper.backend_linux.clone();

    let insert_result = sqlx::query(
        r#"INSERT INTO rip_jobs (id, watchlist_id, status, drive_path, backend, temp_dir, output_dir, started_at)
           VALUES ($1, $2, 'detected', $3, $4, $5, $6, now())"#,
    )
    .bind(job_id)
    .bind(watchlist_id)
    .bind(&drive_path)
    .bind(&backend)
    .bind(&cfg.ripper.temp_dir)
    .bind(&cfg.storage.local_path)
    .execute(&pool)
    .await;

    if let Err(e) = insert_result {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response();
    }

    // Update watchlist to "ripping" if linked
    if let Some(wl_id) = watchlist_id {
        let _ = sqlx::query("UPDATE watchlist SET status = 'ripping', updated_at = now() WHERE id = $1")
            .bind(wl_id)
            .execute(&pool)
            .await;
    }

    // Spawn background rip task
    tokio::spawn(async move {
        let discogs = match DiscogsClient::new(&cfg) {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to create Discogs client: {e}");
                let _ = sqlx::query(
                    "UPDATE rip_jobs SET status = 'failed', error_msg = $1, finished_at = now() WHERE id = $2"
                )
                .bind(e.to_string())
                .bind(job_id)
                .execute(&pool)
                .await;
                return;
            }
        };

        // Fetch release metadata from Discogs
        let release_info = match discogs.get_release_for_rip(discogs_id).await {
            Ok(r) => r,
            Err(e) => {
                error!("Failed to fetch Discogs release {discogs_id}: {e}");
                let _ = sqlx::query(
                    "UPDATE rip_jobs SET status = 'failed', error_msg = $1, finished_at = now() WHERE id = $2"
                )
                .bind(format!("Discogs fetch failed: {e}"))
                .bind(job_id)
                .execute(&pool)
                .await;
                return;
            }
        };

        info!(
            "Starting rip: {} - {} ({} tracks) from {}",
            release_info.artist, release_info.title,
            release_info.tracklist.len(), drive_path
        );

        // Update track count now that we know it
        let _ = sqlx::query("UPDATE rip_jobs SET track_count = $1 WHERE id = $2")
            .bind(release_info.tracklist.len() as i32)
            .bind(job_id)
            .execute(&pool)
            .await;

        let pipeline = mm_ripper::RipPipeline::new(cfg.clone(), job_id, pool.clone());

        match pipeline.run_with_discogs(&drive_path, &release_info, &discogs).await {
            Ok(result) => {
                info!(
                    "Rip complete: {} tracks → {}",
                    result.tracks.len(), result.output_dir
                );
                let _ = sqlx::query(
                    "UPDATE rip_jobs SET status = 'done', output_dir = $1, finished_at = now() WHERE id = $2"
                )
                .bind(&result.output_dir)
                .bind(job_id)
                .execute(&pool)
                .await;

                // Advance watchlist to "done"
                if let Some(wl_id) = watchlist_id {
                    let _ = sqlx::query("UPDATE watchlist SET status = 'done', updated_at = now() WHERE id = $1")
                        .bind(wl_id)
                        .execute(&pool)
                        .await;
                }
            }
            Err(e) => {
                error!("Rip failed: {e:#}");
                let _ = sqlx::query(
                    "UPDATE rip_jobs SET status = 'failed', error_msg = $1, finished_at = now() WHERE id = $2"
                )
                .bind(e.to_string())
                .bind(job_id)
                .execute(&pool)
                .await;

                // Revert watchlist back to ready_to_rip on failure
                if let Some(wl_id) = watchlist_id {
                    let _ = sqlx::query("UPDATE watchlist SET status = 'ready_to_rip', updated_at = now() WHERE id = $1")
                        .bind(wl_id)
                        .execute(&pool)
                        .await;
                }
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "id": job_id,
            "status": "detected",
            "message": "Rip job started"
        })),
    ).into_response()
}
