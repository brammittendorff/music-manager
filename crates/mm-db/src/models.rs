use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

// ─── Copyright status ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CopyrightStatus {
    Unknown,
    PublicDomain,
    LikelyPublicDomain,
    CheckRequired,
    UnderCopyright,
}

impl std::fmt::Display for CopyrightStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => write!(f, "UNKNOWN"),
            Self::PublicDomain => write!(f, "PUBLIC_DOMAIN"),
            Self::LikelyPublicDomain => write!(f, "LIKELY_PUBLIC_DOMAIN"),
            Self::CheckRequired => write!(f, "CHECK_REQUIRED"),
            Self::UnderCopyright => write!(f, "UNDER_COPYRIGHT"),
        }
    }
}

impl From<String> for CopyrightStatus {
    fn from(s: String) -> Self {
        match s.as_str() {
            "PUBLIC_DOMAIN" => Self::PublicDomain,
            "LIKELY_PUBLIC_DOMAIN" => Self::LikelyPublicDomain,
            "CHECK_REQUIRED" => Self::CheckRequired,
            "UNDER_COPYRIGHT" => Self::UnderCopyright,
            _ => Self::Unknown,
        }
    }
}

// ─── Release ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Release {
    pub id: Uuid,
    pub discogs_id: i32,
    pub title: String,
    pub artists: Vec<String>,
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub country: String,
    pub country_code: String,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub styles: Vec<String>,
    pub formats: Vec<String>,
    pub discogs_url: String,
    pub thumb_url: Option<String>,
    pub musicbrainz_id: Option<Uuid>,
    pub copyright_status: String,
    pub copyright_note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub discovery_job_id: Option<Uuid>,
}

impl Release {
    pub fn copyright(&self) -> CopyrightStatus {
        CopyrightStatus::from(self.copyright_status.clone())
    }

    pub fn primary_artist(&self) -> &str {
        self.artists.first().map(String::as_str).unwrap_or("Unknown")
    }
}

// ─── Platform check ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PlatformCheck {
    pub id: Uuid,
    pub release_id: Uuid,
    pub platform: String,
    pub found: bool,
    pub match_score: Option<f64>,
    pub platform_url: Option<String>,
    pub checked_at: DateTime<Utc>,
}

// ─── Watchlist ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchlistStatus {
    ToBuy,
    Ordered,
    Purchased,
    ReadyToRip,
    Ripping,
    Done,
    Skipped,
}

impl std::fmt::Display for WatchlistStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToBuy => write!(f, "to_buy"),
            Self::Ordered => write!(f, "ordered"),
            Self::Purchased => write!(f, "purchased"),
            Self::ReadyToRip => write!(f, "ready_to_rip"),
            Self::Ripping => write!(f, "ripping"),
            Self::Done => write!(f, "done"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

impl From<String> for WatchlistStatus {
    fn from(s: String) -> Self {
        match s.as_str() {
            "ordered" => Self::Ordered,
            "purchased" => Self::Purchased,
            "ready_to_rip" => Self::ReadyToRip,
            "ripping" => Self::Ripping,
            "done" => Self::Done,
            "skipped" => Self::Skipped,
            _ => Self::ToBuy,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WatchlistEntry {
    pub id: Uuid,
    pub release_id: Uuid,
    pub status: String,
    pub buy_url: Option<String>,
    pub price_eur: Option<bigdecimal::BigDecimal>,
    pub seller: Option<String>,
    pub notes: Option<String>,
    pub added_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl WatchlistEntry {
    pub fn status_enum(&self) -> WatchlistStatus {
        WatchlistStatus::from(self.status.clone())
    }
}

// ─── Rip job ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RipJob {
    pub id: Uuid,
    pub watchlist_id: Option<Uuid>,
    pub release_id: Option<Uuid>,
    pub disc_id: Option<String>,
    pub musicbrainz_id: Option<Uuid>,
    pub status: String,
    pub drive_path: String,
    pub backend: String,
    pub temp_dir: String,
    pub output_dir: Option<String>,
    pub track_count: Option<i32>,
    pub error_msg: Option<String>,
    pub accuraterip_status: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

// ─── Track ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Track {
    pub id: Uuid,
    pub rip_job_id: Uuid,
    pub release_id: Option<Uuid>,
    pub track_number: i32,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub file_path: String,
    pub file_format: String,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i32>,
    pub duration_ms: Option<i32>,
    pub file_size_bytes: Option<i64>,
    pub accuraterip_v1: Option<String>,
    pub accuraterip_v2: Option<String>,
    pub accuraterip_ok: Option<bool>,
    pub musicbrainz_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

// ─── View: release with availability summary ──────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseWithAvailability {
    pub release: Release,
    pub checks: Vec<PlatformCheck>,
    pub watchlist: Option<WatchlistEntry>,
}

impl ReleaseWithAvailability {
    pub fn is_on_any_platform(&self) -> bool {
        self.checks.iter().any(|c| c.found)
    }

    pub fn platforms_found(&self) -> Vec<&str> {
        self.checks
            .iter()
            .filter(|c| c.found)
            .map(|c| c.platform.as_str())
            .collect()
    }

    pub fn is_in_watchlist(&self) -> bool {
        self.watchlist.is_some()
    }
}
