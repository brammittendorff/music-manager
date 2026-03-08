use anyhow::Result;
use config::{Config, Environment, File};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub search: SearchConfig,
    pub platforms: PlatformsConfig,
    pub copyright: CopyrightConfig,
    pub ripper: RipperConfig,
    pub storage: StorageConfig,
    pub api: ApiConfig,
    pub database: DatabaseConfig,
    pub notifications: NotificationConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SearchConfig {
    /// Human-readable country name used in Discogs search
    pub country: String,
    /// ISO 3166-1 alpha-2 country code (NL, DE, BE, ...)
    pub country_code: String,
    pub genres: Vec<String>,
    pub year_from: u32,
    pub year_to: u32,
    /// Known labels from this country to prioritize
    pub labels: Vec<String>,
    /// Max results per Discogs search page
    pub page_size: u32,
    /// How many pages to fetch per search run
    pub max_pages: u32,
    /// Format filter passed to Discogs search (e.g. "Album", "Single", "EP")
    pub format_filter: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PlatformsConfig {
    /// Platforms to check for availability
    pub check: Vec<String>,
    /// Jaro-Winkler threshold 0.0–1.0 to consider a match found
    pub match_threshold: f64,
    /// Wait ms between requests to the same platform
    pub rate_limit_ms: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CopyrightConfig {
    /// EU: sound recordings expire 70 years after publication
    pub sound_recording_term_years: u32,
    /// Flag for manual review if this many years from expiry
    pub review_buffer_years: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RipperConfig {
    /// Poll interval in seconds for drive detection fallback
    pub poll_interval_secs: u64,
    /// Output format: "mp3" or "flac"
    pub format: String,
    /// MP3 bitrate in kbps (used when format = "mp3")
    pub bitrate_kbps: u32,
    /// Keep a lossless FLAC master alongside the MP3
    pub keep_flac: bool,
    /// Ripping backend on Linux: "cdparanoia" or "ffmpeg"
    pub backend_linux: String,
    /// Ripping backend on Windows: "ffmpeg" or "cdparanoia"
    pub backend_windows: String,
    /// Directory for in-progress rip temp files
    pub temp_dir: String,
    /// Verify rip accuracy via AccurateRip
    pub accuraterip: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StorageConfig {
    /// Root path for organized digitized music
    pub local_path: String,
    /// Folder naming template: {country}/{artist}/{year} - {album}
    pub folder_template: String,
    /// File naming template: {track:02} - {title}
    pub file_template: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ApiConfig {
    pub discogs_token: String,
    pub spotify_client_id: String,
    pub spotify_client_secret: String,
    pub youtube_api_key: String,
    pub musicbrainz_user_agent: String,
    #[serde(default)]
    pub lastfm_api_key: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    /// Max connections in pool
    pub max_connections: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub discord_webhook: Option<String>,
    pub slack_webhook: Option<String>,
}

impl AppConfig {
    /// Load config from layered sources:
    ///   1. config/default.toml
    ///   2. config/local.toml (gitignored overrides)
    ///   3. Environment variables prefixed with MMGR_
    ///      e.g. MMGR_SEARCH__COUNTRY=Germany
    pub fn load() -> Result<Self> {
        Self::load_from("config")
    }

    pub fn load_from(config_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = config_dir.as_ref();
        let cfg = Config::builder()
            .add_source(File::from(dir.join("default.toml")).required(true))
            .add_source(File::from(dir.join("local.toml")).required(false))
            .add_source(
                Environment::with_prefix("MMGR")
                    .prefix_separator("_")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()?;

        let app: AppConfig = cfg.try_deserialize()?;
        Ok(app)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputFormat {
    Mp3,
    Flac,
}

impl AppConfig {
    pub fn output_format(&self) -> OutputFormat {
        match self.ripper.format.to_lowercase().as_str() {
            "flac" => OutputFormat::Flac,
            _ => OutputFormat::Mp3,
        }
    }
}
