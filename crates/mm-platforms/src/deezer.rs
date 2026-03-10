use anyhow::Result;
use governor::{Quota, RateLimiter};
use mm_matcher::best_match;
use reqwest::Client;
use serde::Deserialize;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use crate::{AlbumTracksResult, PlatformChecker, PlatformResult, TrackMatch};

#[derive(Debug, Deserialize)]
struct DeezerSearchResponse {
    data: Vec<DeezerTrack>,
}

#[derive(Debug, Deserialize)]
struct DeezerTrack {
    #[allow(dead_code)]
    id: u64,
    title: String,
    artist: DeezerArtist,
    link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeezerArtist {
    name: String,
}

// ─── Album search types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DeezerAlbumSearchResponse {
    data: Vec<DeezerAlbum>,
}

#[derive(Debug, Deserialize)]
struct DeezerAlbum {
    id: u64,
    title: String,
    artist: DeezerArtist,
    link: Option<String>,
}

// ─── Album tracks types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DeezerAlbumTracksResponse {
    data: Vec<DeezerAlbumTrack>,
}

#[derive(Debug, Deserialize)]
struct DeezerAlbumTrack {
    title: String,
    artist: DeezerArtist,
    link: Option<String>,
}

pub struct DeezerChecker {
    http: Client,
    limiter: Arc<governor::DefaultDirectRateLimiter>,
}

impl DeezerChecker {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Client::builder().timeout(Duration::from_secs(30)).local_address("0.0.0.0".parse().ok()).build()?,
            // Deezer: ~50 req/5s (community-sourced). Use 1 req/sec to be safe.
            limiter: Arc::new(RateLimiter::direct(
                Quota::per_second(NonZeroU32::new(1).unwrap()),
            )),
        })
    }

    /// Fetch all tracks of an album by its Deezer ID.
    async fn get_album_tracks(&self, album_id: u64) -> Result<Vec<DeezerAlbumTrack>> {
        let url = format!("https://api.deezer.com/album/{}/tracks?limit=100", album_id);
        tracing::info!(url = %url, "Deezer: fetching album tracks");
        let body = self.api_get(&url).await?;
        let resp: DeezerAlbumTracksResponse = serde_json::from_str(&body)?;
        Ok(resp.data)
    }

    /// Rate-limited GET with Deezer error handling.
    /// Deezer returns error code 4 ("Quota limit exceeded") in the JSON body.
    async fn api_get(&self, url: &str) -> Result<String> {
        self.limiter.until_ready().await;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Deezer API returned {status}: {body}");
        }
        let body = resp.text().await?;
        // Check for Deezer-specific error in JSON response
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(err) = v.get("error") {
                let code = err.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown");
                if code == 4 {
                    anyhow::bail!("Deezer: quota limit exceeded, backing off");
                }
                anyhow::bail!("Deezer error {code}: {msg}");
            }
        }
        Ok(body)
    }
}

#[async_trait::async_trait]
impl PlatformChecker for DeezerChecker {
    fn name(&self) -> &str {
        "deezer"
    }

    /// Album-level check: search Deezer for the album by artist + album title.
    /// Uses 1 API call instead of N per-track calls.
    async fn check_album(&self, artist: &str, album: &str, threshold: f64) -> Result<Option<PlatformResult>> {
        let query = format!("artist:\"{}\" album:\"{}\"", artist, album);
        let url = format!(
            "https://api.deezer.com/search/album?q={}&limit=5",
            urlenccode(&query)
        );
        tracing::info!(url = %url, "Deezer: album search");

        let body = self.api_get(&url).await?;
        let resp: DeezerAlbumSearchResponse = serde_json::from_str(&body)?;

        let candidates: Vec<(String, String, Option<String>)> = resp
            .data
            .iter()
            .map(|a| (a.artist.name.clone(), a.title.clone(), a.link.clone()))
            .collect();

        match best_match(artist, album, &candidates, threshold) {
            Some(m) => {
                tracing::info!("Deezer: album found: {} - {}", m.candidate_artist, m.candidate_title);
                Ok(Some(PlatformResult::found("deezer", m)))
            }
            None => Ok(Some(PlatformResult::not_found("deezer"))),
        }
    }

    /// Album search + tracklist fetch: 2 API calls instead of N per-track searches.
    async fn check_album_tracks(
        &self,
        artist: &str,
        album: &str,
        track_titles: &[String],
        threshold: f64,
    ) -> Result<Option<AlbumTracksResult>> {
        // 1. Album search
        let query = format!("artist:\"{}\" album:\"{}\"", artist, album);
        let url = format!(
            "https://api.deezer.com/search/album?q={}&limit=5",
            urlenccode(&query)
        );
        tracing::info!(url = %url, "Deezer: album search (with tracks)");

        let body = self.api_get(&url).await?;
        let resp: DeezerAlbumSearchResponse = serde_json::from_str(&body)?;

        let best_album = resp.data.iter().filter_map(|a| {
            let s = mm_matcher::score(artist, album, &a.artist.name, &a.title);
            if s >= threshold { Some((s, a)) } else { None }
        }).max_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap());

        let (_, matched_album) = match best_album {
            Some(a) => a,
            None => {
                return Ok(Some(AlbumTracksResult {
                    platform: "deezer".to_owned(),
                    album_found: false,
                    album_url: None,
                    track_matches: Vec::new(),
                }));
            }
        };

        let album_url = matched_album.link.clone();
        tracing::info!("Deezer: album found, fetching tracklist for {}", matched_album.id);

        // 2. Fetch tracklist
        let album_tracks = self.get_album_tracks(matched_album.id).await?;
        tracing::info!("Deezer: album tracks fetched ({} tracks)", album_tracks.len());

        // 3. Fuzzy-match each expected track
        let track_matches: Vec<TrackMatch> = track_titles.iter().map(|expected| {
            let best = album_tracks.iter().filter_map(|at| {
                let s = mm_matcher::score(artist, expected, &at.artist.name, &at.title);
                if s >= threshold { Some((s, at)) } else { None }
            }).max_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap());

            match best {
                Some((s, at)) => TrackMatch {
                    track_title: expected.clone(),
                    found: true,
                    score: Some(s),
                    platform_url: at.link.clone(),
                },
                None => TrackMatch {
                    track_title: expected.clone(),
                    found: false,
                    score: None,
                    platform_url: None,
                },
            }
        }).collect();

        Ok(Some(AlbumTracksResult {
            platform: "deezer".to_owned(),
            album_found: true,
            album_url,
            track_matches,
        }))
    }

    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult> {
        let query = format!("artist:\"{}\" track:\"{}\"", artist, title);
        let url = format!(
            "https://api.deezer.com/search?q={}&limit=5",
            urlenccode(&query)
        );
        tracing::info!(url = %url, "Deezer: searching");

        let body = self.api_get(&url).await?;
        let resp: DeezerSearchResponse = serde_json::from_str(&body)?;

        let candidates: Vec<(String, String, Option<String>)> = resp
            .data
            .iter()
            .map(|t| (t.artist.name.clone(), t.title.clone(), t.link.clone()))
            .collect();

        match best_match(artist, title, &candidates, threshold) {
            Some(m) => Ok(PlatformResult::found("deezer", m)),
            None => Ok(PlatformResult::not_found("deezer")),
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
