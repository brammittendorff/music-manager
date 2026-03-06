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
    pub async fn get_tracklist(&self, release_id: u32) -> Vec<String> {
        let url = format!("{BASE_URL}/releases/{release_id}");
        match self.get::<serde_json::Value>(&url).await {
            Ok(json) => {
                json["tracklist"]
                    .as_array()
                    .map(|tracks| {
                        tracks
                            .iter()
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

fn urlenccode(s: &str) -> String {
    // Simple percent-encoding for query params
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                c.to_string()
            }
            ' ' => "+".to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}
