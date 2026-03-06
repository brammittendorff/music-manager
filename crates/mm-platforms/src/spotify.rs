use anyhow::{bail, Result};
use mm_config::AppConfig;
use mm_matcher::best_match;
use reqwest::Client;
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::{PlatformChecker, PlatformResult};

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

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

struct TokenCache {
    token: String,
    expires_at: Instant,
}

pub struct SpotifyChecker {
    http: Client,
    client_id: String,
    client_secret: String,
    token: Mutex<Option<TokenCache>>,
}

impl SpotifyChecker {
    pub async fn new(cfg: &AppConfig) -> Result<Self> {
        if cfg.api.spotify_client_id.is_empty() {
            bail!("Spotify client_id not configured");
        }
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            client_id: cfg.api.spotify_client_id.clone(),
            client_secret: cfg.api.spotify_client_secret.clone(),
            token: Mutex::new(None),
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
        let expires_at = Instant::now() + Duration::from_secs(resp.expires_in - 30);

        *self.token.lock().unwrap() = Some(TokenCache {
            token: resp.access_token,
            expires_at,
        });

        Ok(token)
    }
}

#[async_trait::async_trait]
impl PlatformChecker for SpotifyChecker {
    fn name(&self) -> &str {
        "spotify"
    }

    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult> {
        let token = self.access_token().await?;

        let query = format!("artist:{} track:{}", artist, title);
        let url = format!(
            "https://api.spotify.com/v1/search?q={}&type=track&limit=5&market=NL",
            urlenccode(&query)
        );

        tracing::info!(url = %url, "Spotify: searching");
        let search_resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await?;
        let status = search_resp.status();
        if !status.is_success() {
            let body = search_resp.text().await.unwrap_or_default();
            tracing::warn!(status = %status, body = %body, "Spotify search failed");
            anyhow::bail!("Spotify search returned {}: {}", status, body);
        }
        let resp = search_resp.json::<SearchResponse>().await?;

        let candidates: Vec<(String, String, Option<String>)> = resp
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

fn urlenccode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}
