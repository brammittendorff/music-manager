use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub total_releases: i64,
    pub missing_from_streaming: i64,
    pub public_domain: i64,
    pub watchlist_total: i64,
    pub watchlist_to_buy: i64,
    pub watchlist_done: i64,
    pub tracks_digitized: i64,
    pub rip_jobs_done: i64,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ReleaseRow {
    pub id: Uuid,
    pub discogs_id: i32,
    pub title: String,
    pub artists: Vec<String>,
    pub label: Option<String>,
    pub country: String,
    pub country_code: String,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub formats: Vec<String>,
    pub discogs_url: String,
    pub copyright_status: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PlatformCheckRow {
    pub platform: String,
    pub found: bool,
    pub match_score: Option<f64>,
    pub platform_url: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct ReleaseDetail {
    #[serde(flatten)]
    pub release: ReleaseRow,
    pub platforms: Vec<PlatformCheckRow>,
    pub in_watchlist: bool,
    pub watchlist_id: Option<Uuid>,
    pub watchlist_status: Option<String>,
    pub buy_url: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct WatchlistRow {
    pub id: Uuid,
    pub release_id: Uuid,
    pub status: String,
    pub buy_url: Option<String>,
    pub notes: Option<String>,
    pub added_at: DateTime<Utc>,
    pub title: String,
    pub artists: Vec<String>,
    pub year: Option<i32>,
    pub label: Option<String>,
    pub copyright_status: String,
    pub discogs_url: String,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RipJobRow {
    pub id: Uuid,
    pub status: String,
    pub drive_path: String,
    pub backend: String,
    pub track_count: Option<i32>,
    pub output_dir: Option<String>,
    pub error_msg: Option<String>,
    pub accuraterip_status: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub release_title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ReleasesQuery {
    pub country: Option<String>,
    pub missing_only: Option<bool>,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    pub copyright_status: Option<String>,
    pub format_type: Option<String>,
    pub media: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub platform_status: Option<String>, // "unchecked" | "missing" | "found"
    pub platforms: Option<String>,       // comma-separated list: "spotify,deezer" (missing on all listed)
}

#[derive(Debug, Deserialize)]
pub struct UpdateStatusBody {
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct AddToWatchlistBody {
    pub discogs_id: i32,
    pub notes: Option<String>,
}
