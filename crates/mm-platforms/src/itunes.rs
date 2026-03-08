//! iTunes Search API - free, no API key required.
//! Rate limit: ~20 req/min per IP, returns 403 (not 429) when exceeded.
use anyhow::Result;
use governor::{Quota, RateLimiter};
use mm_matcher::best_match;
use reqwest::Client;
use serde::Deserialize;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use crate::{PlatformChecker, PlatformResult};

#[derive(Debug, Deserialize)]
struct ItunesResponse {
    results: Vec<ItunesTrack>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesTrack {
    track_name: Option<String>,
    artist_name: Option<String>,
    track_view_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesAlbum {
    collection_name: Option<String>,
    artist_name: Option<String>,
    collection_view_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ItunesAlbumResponse {
    results: Vec<ItunesAlbum>,
}

pub struct ItunesChecker {
    http: Client,
    limiter: Arc<governor::DefaultDirectRateLimiter>,
}

impl ItunesChecker {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Client::builder().timeout(Duration::from_secs(30)).local_address("0.0.0.0".parse().ok()).build()?,
            // iTunes Search API: ~20 req/min official, but 403s happen near the limit.
            // Use 10/min (1 req/6sec) for safety.
            limiter: Arc::new(RateLimiter::direct(
                Quota::per_minute(NonZeroU32::new(10).unwrap()),
            )),
        })
    }

    /// Make a rate-limited GET request, handling 403 as rate-limit (iTunes doesn't use 429).
    async fn api_get(&self, url: &str) -> Result<String> {
        self.limiter.until_ready().await;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();

        if status == reqwest::StatusCode::FORBIDDEN {
            anyhow::bail!("iTunes: rate limited (403 Forbidden). Backing off.");
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("iTunes API returned {status}: {body}");
        }
        Ok(resp.text().await?)
    }
}

#[async_trait::async_trait]
impl PlatformChecker for ItunesChecker {
    fn name(&self) -> &str {
        "apple_music"
    }

    /// Album-level check: search iTunes for the album by artist + album title.
    /// Uses 1 API call instead of N per-track calls.
    async fn check_album(&self, artist: &str, album: &str, threshold: f64) -> Result<Option<PlatformResult>> {
        let query = format!("{} {}", artist, album);
        let url = format!(
            "https://itunes.apple.com/search?term={}&entity=album&limit=5&country=NL",
            urlenccode(&query)
        );
        tracing::info!(url = %url, "Apple Music: album search");

        let body = self.api_get(&url).await?;
        let resp: ItunesAlbumResponse = serde_json::from_str(&body)?;

        let candidates: Vec<(String, String, Option<String>)> = resp
            .results
            .iter()
            .filter_map(|a| {
                Some((
                    a.artist_name.clone()?,
                    a.collection_name.clone()?,
                    a.collection_view_url.clone(),
                ))
            })
            .collect();

        match best_match(artist, album, &candidates, threshold) {
            Some(m) => {
                tracing::info!("Apple Music: album found: {} - {}", m.candidate_artist, m.candidate_title);
                Ok(Some(PlatformResult::found("apple_music", m)))
            }
            None => Ok(Some(PlatformResult::not_found("apple_music"))),
        }
    }

    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult> {
        let query = format!("{} {}", artist, title);
        let url = format!(
            "https://itunes.apple.com/search?term={}&entity=song&limit=5&country=NL",
            urlenccode(&query)
        );
        tracing::info!(url = %url, "Apple Music: searching");

        let body = self.api_get(&url).await?;
        let resp: ItunesResponse = serde_json::from_str(&body)?;

        let candidates: Vec<(String, String, Option<String>)> = resp
            .results
            .iter()
            .filter_map(|t| {
                Some((
                    t.artist_name.clone()?,
                    t.track_name.clone()?,
                    t.track_view_url.clone(),
                ))
            })
            .collect();

        match best_match(artist, title, &candidates, threshold) {
            Some(m) => Ok(PlatformResult::found("apple_music", m)),
            None => Ok(PlatformResult::not_found("apple_music")),
        }
    }
}

fn urlenccode(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => result.push(b as char),
            b' ' => result.push('+'),
            _ => result.push_str(&format!("%{:02X}", b)),
        }
    }
    result
}
