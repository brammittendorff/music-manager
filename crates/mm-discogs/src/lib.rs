pub mod models;

use anyhow::{bail, Result};
use governor::{Quota, RateLimiter};
use mm_config::AppConfig;
use models::{DiscogsRelease, DiscogsSearchResponse};
use reqwest::{Client, StatusCode};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

const BASE_URL: &str = "https://api.discogs.com";
/// Discogs allows 60 authenticated requests/minute.
const REQUESTS_PER_MINUTE: u32 = 55; // leave headroom

pub struct DiscogsClient {
    http: Client,
    token: String,
    limiter: Arc<governor::DefaultDirectRateLimiter>,
}

impl DiscogsClient {
    pub fn new(cfg: &AppConfig) -> Result<Self> {
        let http = Client::builder()
            .user_agent("music-manager/0.1 +https://github.com/music-manager")
            .timeout(Duration::from_secs(30))
            .local_address("0.0.0.0".parse().ok())
            .build()?;

        let quota = Quota::per_minute(
            NonZeroU32::new(REQUESTS_PER_MINUTE).unwrap(),
        );
        let limiter = Arc::new(RateLimiter::direct(quota));

        Ok(Self {
            http,
            token: cfg.api.discogs_token.clone(),
            limiter,
        })
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        // Block until the rate limiter allows this request
        self.limiter.until_ready().await;

        let resp = self
            .http
            .get(url)
            .header("Authorization", format!("Discogs token={}", self.token))
            .send()
            .await?;

        match resp.status() {
            StatusCode::OK => Ok(resp.json::<T>().await?),
            StatusCode::TOO_MANY_REQUESTS => {
                warn!("Discogs 429 - rate limiter should prevent this");
                bail!("Discogs rate limit exceeded")
            }
            StatusCode::NOT_FOUND => bail!("Discogs: resource not found: {url}"),
            s => bail!("Discogs HTTP {s} for {url}"),
        }
    }

    /// Search Discogs releases with the given filters.
    /// Returns all results across `max_pages` pages.
    pub async fn search_releases(
        &self,
        cfg: &AppConfig,
    ) -> Result<Vec<DiscogsRelease>> {
        let mut all: Vec<DiscogsRelease> = Vec::new();

        for page in 1..=cfg.search.max_pages {
            let url = self.build_search_url(cfg, page);
            debug!("Discogs search page {page}: {url}");

            let resp: DiscogsSearchResponse = self.get(&url).await?;
            let total_pages = resp.pagination.pages;

            all.extend(resp.results);

            if page >= total_pages || page >= cfg.search.max_pages {
                break;
            }
        }

        Ok(all)
    }

    /// Fetch a single page of search results.
    /// Returns `(releases, total_pages)`.
    pub async fn search_releases_page(
        &self,
        cfg: &AppConfig,
        page: u32,
    ) -> Result<(Vec<DiscogsRelease>, u32)> {
        let url = self.build_search_url(cfg, page);
        debug!("Discogs search page {page}: {url}");
        let resp: DiscogsSearchResponse = self.get(&url).await?;
        let total_pages = resp.pagination.pages;
        Ok((resp.results, total_pages))
    }

    fn build_search_url(&self, cfg: &AppConfig, page: u32) -> String {
        let genres: String = cfg
            .search
            .genres
            .iter()
            .map(|g| format!("genre={}", urlenccode(g)))
            .collect::<Vec<_>>()
            .join("&");

        format!(
            "{BASE_URL}/database/search?type=release&country={country}&{genres}\
             &year={year_from}-{year_to}&format={format_filter}&per_page={per_page}&page={page}",
            country = urlenccode(&cfg.search.country),
            year_from = cfg.search.year_from,
            year_to = cfg.search.year_to,
            format_filter = urlenccode(&cfg.search.format_filter),
            per_page = cfg.search.page_size,
        )
    }

    /// Fetch full release details by Discogs release ID.
    pub async fn get_release(&self, release_id: u32) -> Result<serde_json::Value> {
        let url = format!("{BASE_URL}/releases/{release_id}");
        self.get(&url).await
    }

    /// Fetch track titles for a release. Returns empty vec on error (graceful degradation).
    /// Filters out non-track entries (headings like "DVD & CD", index entries, etc.)
    /// by checking the Discogs `type_` field — only `"track"` entries are real music.
    pub async fn get_tracklist(&self, release_id: u32) -> Vec<String> {
        let url = format!("{BASE_URL}/releases/{release_id}");
        match self.get::<serde_json::Value>(&url).await {
            Ok(json) => {
                json["tracklist"]
                    .as_array()
                    .map(|tracks| {
                        tracks
                            .iter()
                            .filter(|t| {
                                // Discogs type_: "track" = real music, "heading"/"index" = not
                                t["type_"].as_str().unwrap_or("track") == "track"
                            })
                            .filter_map(|t| t["title"].as_str())
                            .filter(|t| !t.is_empty())
                            .map(|t| t.to_owned())
                            .collect()
                    })
                    .unwrap_or_default()
            }
            Err(_) => vec![],
        }
    }

    /// Fetch marketplace stats for a release: lowest price + number for sale.
    /// Discogs API: GET /marketplace/stats/{release_id}
    pub async fn get_marketplace_stats(&self, release_id: u32) -> Result<models::MarketplaceStats> {
        let url = format!("{BASE_URL}/marketplace/stats/{release_id}?curr_abbr=EUR");
        self.get(&url).await
    }

    /// Fetch tracklist with durations for a release.
    /// Returns track titles paired with duration in milliseconds (if available).
    /// Duration format from Discogs is "M:SS" or "MM:SS".
    pub async fn get_tracklist_with_durations(&self, release_id: u32) -> Vec<(String, Option<u32>)> {
        let url = format!("{BASE_URL}/releases/{release_id}");
        match self.get::<serde_json::Value>(&url).await {
            Ok(json) => {
                json["tracklist"]
                    .as_array()
                    .map(|tracks| {
                        tracks
                            .iter()
                            .filter(|t| t["type_"].as_str().unwrap_or("track") == "track")
                            .filter_map(|t| {
                                let title = t["title"].as_str()?.to_owned();
                                let duration_ms = t["duration"]
                                    .as_str()
                                    .and_then(|d| parse_duration_to_ms(d));
                                Some((title, duration_ms))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            Err(_) => vec![],
        }
    }

    /// Fetch the primary cover art image URL for a release.
    /// Returns the full-resolution URI of the first "primary" image,
    /// falling back to the first image of any type.
    pub async fn get_cover_art_url(&self, release_id: u32) -> Option<String> {
        let json: serde_json::Value = self.get(&format!("{BASE_URL}/releases/{release_id}")).await.ok()?;
        let images = json["images"].as_array()?;
        // Prefer "primary" type, fall back to first image
        let primary = images.iter().find(|i| i["type"].as_str() == Some("primary"));
        let img = primary.or_else(|| images.first())?;
        img["uri"].as_str().map(|s| s.to_owned())
    }

    /// Download cover art image bytes. Requires Discogs auth to access image URLs.
    pub async fn download_cover_art(&self, image_url: &str) -> Result<Vec<u8>> {
        self.limiter.until_ready().await;
        let resp = self.http
            .get(image_url)
            .header("Authorization", format!("Discogs token={}", self.token))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("Failed to download cover art: HTTP {}", resp.status());
        }
        Ok(resp.bytes().await?.to_vec())
    }

    /// Fetch release metadata needed for ripping: artist, title, year, tracklist, cover art URL.
    pub async fn get_release_for_rip(&self, release_id: u32) -> Result<RipReleaseInfo> {
        let json: serde_json::Value = self.get(&format!("{BASE_URL}/releases/{release_id}")).await?;

        let title = json["title"].as_str().unwrap_or("Unknown Album").to_owned();

        // Artists array
        let artist = json["artists"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|a| a["name"].as_str())
            .unwrap_or("Unknown Artist")
            .to_owned();

        let year = json["year"].as_u64().map(|y| y as i32);

        let tracklist: Vec<(String, Option<u32>)> = json["tracklist"]
            .as_array()
            .map(|tracks| {
                tracks.iter()
                    .filter(|t| t["type_"].as_str().unwrap_or("track") == "track")
                    .filter_map(|t| {
                        let title = t["title"].as_str()?.to_owned();
                        let duration_ms = t["duration"].as_str().and_then(|d| parse_duration_to_ms(d));
                        Some((title, duration_ms))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let cover_art_url = json["images"]
            .as_array()
            .and_then(|images| {
                let primary = images.iter().find(|i| i["type"].as_str() == Some("primary"));
                let img = primary.or_else(|| images.first())?;
                img["uri"].as_str().map(|s| s.to_owned())
            });

        Ok(RipReleaseInfo {
            discogs_id: release_id,
            title,
            artist,
            year,
            tracklist,
            cover_art_url,
        })
    }

    /// Generate the Discogs marketplace buy URL for a release.
    pub fn buy_url(release_id: u32) -> String {
        format!("https://www.discogs.com/sell/list?release_id={release_id}&ships_from=Netherlands")
    }

    /// Generate the Discogs advanced search URL for the configured country.
    pub fn advanced_search_url(cfg: &AppConfig) -> String {
        format!(
            "https://www.discogs.com/search/advanced?country_exact={}&format_exact=Vinyl",
            urlenccode(&cfg.search.country)
        )
    }
}

/// Release metadata needed for the rip pipeline.
#[derive(Debug, Clone)]
pub struct RipReleaseInfo {
    pub discogs_id: u32,
    pub title: String,
    pub artist: String,
    pub year: Option<i32>,
    pub tracklist: Vec<(String, Option<u32>)>, // (title, duration_ms)
    pub cover_art_url: Option<String>,
}

/// Parse Discogs duration string "M:SS" or "MM:SS" or "H:MM:SS" to milliseconds.
fn parse_duration_to_ms(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    match parts.len() {
        2 => {
            let mins: u32 = parts[0].parse().ok()?;
            let secs: u32 = parts[1].parse().ok()?;
            Some((mins * 60 + secs) * 1000)
        }
        3 => {
            let hours: u32 = parts[0].parse().ok()?;
            let mins: u32 = parts[1].parse().ok()?;
            let secs: u32 = parts[2].parse().ok()?;
            Some((hours * 3600 + mins * 60 + secs) * 1000)
        }
        _ => None,
    }
}

fn urlenccode(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push_str(&format!("%{:02X}", b));
            }
        }
    }
    result
}
