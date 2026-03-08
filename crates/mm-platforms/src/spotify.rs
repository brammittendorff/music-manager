use anyhow::{bail, Result};
use governor::{Quota, RateLimiter};
use mm_config::AppConfig;
use mm_matcher::best_match;
use reqwest::Client;
use serde::Deserialize;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::{PlatformChecker, PlatformResult};

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

// ─── Track search types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchResponse {
    tracks: TracksWrapper,
}

#[derive(Debug, Deserialize)]
struct TracksWrapper {
    items: Vec<SpotifyTrack>,
}

#[derive(Debug, Deserialize)]
struct SpotifyTrack {
    #[allow(dead_code)]
    id: String,
    name: String,
    artists: Vec<SpotifyArtist>,
    external_urls: ExternalUrls,
}

#[derive(Debug, Deserialize)]
struct SpotifyArtist {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ExternalUrls {
    spotify: Option<String>,
}

// ─── Album search types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AlbumSearchResponse {
    albums: AlbumsWrapper,
}

#[derive(Debug, Deserialize)]
struct AlbumsWrapper {
    items: Vec<SpotifyAlbum>,
}

#[derive(Debug, Deserialize)]
struct SpotifyAlbum {
    name: String,
    artists: Vec<SpotifyArtist>,
    external_urls: ExternalUrls,
}

// ─── Token cache ─────────────────────────────────────────────────────────────

struct TokenCache {
    token: String,
    expires_at: Instant,
}

pub struct SpotifyChecker {
    http: Client,
    client_id: String,
    client_secret: String,
    token: Mutex<Option<TokenCache>>,
    limiter: Arc<governor::DefaultDirectRateLimiter>,
    /// Epoch millis until which we should not make any requests (backoff from 429).
    backoff_until: AtomicU64,
}

impl SpotifyChecker {
    pub async fn new(cfg: &AppConfig) -> Result<Self> {
        if cfg.api.spotify_client_id.is_empty() {
            bail!("Spotify client_id not configured");
        }
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .local_address("0.0.0.0".parse().ok())
                .build()?,
            client_id: cfg.api.spotify_client_id.clone(),
            client_secret: cfg.api.spotify_client_secret.clone(),
            token: Mutex::new(None),
            // Spotify: ~250 req/30s (unconfirmed). Use 1 req/sec to stay safe.
            // Repeated 429s can escalate to 24h bans, so be conservative.
            limiter: Arc::new(RateLimiter::direct(
                Quota::per_second(NonZeroU32::new(1).unwrap()),
            )),
            backoff_until: AtomicU64::new(0),
        })
    }

    async fn access_token(&self) -> Result<String> {
        // Check cached token
        {
            let guard = self.token.lock().unwrap();
            if let Some(cache) = &*guard {
                if Instant::now() < cache.expires_at {
                    return Ok(cache.token.clone());
                }
            }
        }

        // Request new token via client credentials flow
        tracing::debug!("Spotify: requesting new access token");
        let token_resp = self
            .http
            .post("https://accounts.spotify.com/api/token")
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await?;
        let status = token_resp.status();
        if !status.is_success() {
            let body = token_resp.text().await.unwrap_or_default();
            anyhow::bail!("Spotify token endpoint returned {}: {}", status, body);
        }
        let resp = token_resp.json::<TokenResponse>().await?;

        let token = resp.access_token.clone();
        let expires_at = Instant::now() + Duration::from_secs(resp.expires_in.saturating_sub(30));

        *self.token.lock().unwrap() = Some(TokenCache {
            token: resp.access_token,
            expires_at,
        });

        Ok(token)
    }

    /// Execute a Spotify API GET request with retry on 429/401.
    /// Honors Retry-After headers and implements exponential backoff to avoid
    /// escalating bans (Spotify can block for up to 24h on repeated 429s).
    async fn api_get(&self, url: &str) -> Result<reqwest::Response> {
        // Check if we're in a backoff period from a previous 429
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let backoff = self.backoff_until.load(Ordering::Relaxed);
        if now_ms < backoff {
            let wait_secs = (backoff - now_ms) / 1000;
            anyhow::bail!("Spotify: in backoff period ({wait_secs}s remaining), skipping");
        }

        let mut refreshed_token = false;
        for attempt in 0..3u8 {
            self.limiter.until_ready().await;
            let token = self.access_token().await?;
            let resp = self.http.get(url).bearer_auth(&token).send().await?;
            let status = resp.status();

            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(5);

                // Always honor Retry-After, but if it's very long, set backoff and bail
                if retry_after > 30 {
                    tracing::error!("Spotify 429 with Retry-After: {retry_after}s - entering long backoff");
                    let until = now_ms + (retry_after as u64 * 1000);
                    self.backoff_until.store(until, Ordering::Relaxed);
                    anyhow::bail!("Spotify: rate limited for {retry_after}s, backing off");
                }

                // For shorter waits, honor the header with extra padding
                let wait = retry_after + 2 + (attempt as u64 * 3);
                tracing::warn!("Spotify 429 (attempt {}) - waiting {wait}s (Retry-After: {retry_after})", attempt + 1);
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            if status == reqwest::StatusCode::UNAUTHORIZED && !refreshed_token {
                *self.token.lock().unwrap() = None;
                refreshed_token = true;
                tracing::warn!("Spotify: token expired, refreshing");
                continue;
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Spotify API returned {}: {}", status, body);
            }
            return Ok(resp);
        }
        // If we exhausted retries due to 429s, set a 60s backoff to cool down
        let until = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 60_000;
        self.backoff_until.store(until, Ordering::Relaxed);
        anyhow::bail!("Spotify: failed after retries, backing off 60s")
    }
}

#[async_trait::async_trait]
impl PlatformChecker for SpotifyChecker {
    fn name(&self) -> &str {
        "spotify"
    }

    /// Album-level check: search Spotify for the album by artist + album title.
    /// Uses 1 API call instead of N per-track calls.
    async fn check_album(&self, artist: &str, album: &str, threshold: f64) -> Result<Option<PlatformResult>> {
        let query = format!("artist:{} album:{}", artist, album);
        let url = format!(
            "https://api.spotify.com/v1/search?q={}&type=album&limit=5&market=NL",
            urlenccode(truncate_query(&query))
        );
        tracing::info!(url = %url, "Spotify: album search");

        let resp = self.api_get(&url).await?;
        let data: AlbumSearchResponse = resp.json().await?;

        let candidates: Vec<(String, String, Option<String>)> = data
            .albums
            .items
            .iter()
            .map(|a| {
                let artist_name = a.artists.first().map(|x| x.name.clone()).unwrap_or_default();
                (artist_name, a.name.clone(), a.external_urls.spotify.clone())
            })
            .collect();

        match best_match(artist, album, &candidates, threshold) {
            Some(m) => {
                tracing::info!("Spotify: album found: {} - {}", m.candidate_artist, m.candidate_title);
                Ok(Some(PlatformResult::found("spotify", m)))
            }
            None => Ok(Some(PlatformResult::not_found("spotify"))),
        }
    }

    /// Per-track check (fallback if check_album is not used).
    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult> {
        let query = format!("artist:{} track:{}", artist, title);
        let url = format!(
            "https://api.spotify.com/v1/search?q={}&type=track&limit=5&market=NL",
            urlenccode(truncate_query(&query))
        );
        tracing::info!(url = %url, "Spotify: track search");

        let resp = self.api_get(&url).await?;
        let data: SearchResponse = resp.json().await?;

        let candidates: Vec<(String, String, Option<String>)> = data
            .tracks
            .items
            .iter()
            .map(|t| {
                let a = t.artists.first().map(|a| a.name.clone()).unwrap_or_default();
                (a, t.name.clone(), t.external_urls.spotify.clone())
            })
            .collect();

        match best_match(artist, title, &candidates, threshold) {
            Some(m) => Ok(PlatformResult::found("spotify", m)),
            None => Ok(PlatformResult::not_found("spotify")),
        }
    }
}

/// Truncate a query string to Spotify's 250-character limit on a word boundary.
fn truncate_query(q: &str) -> &str {
    if q.len() <= 250 {
        return q;
    }
    let mut end = 250;
    while end > 0 && !q.is_char_boundary(end) {
        end -= 1;
    }
    // Try to break on a word boundary
    if let Some(pos) = q[..end].rfind(' ') {
        &q[..pos]
    } else {
        &q[..end]
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
