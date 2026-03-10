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

use crate::{AlbumTracksResult, PlatformChecker, PlatformResult, TrackMatch};

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
    collection_id: Option<u64>,
    collection_name: Option<String>,
    artist_name: Option<String>,
    collection_view_url: Option<String>,
}

// ─── Album tracks (lookup) types ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ItunesLookupResponse {
    results: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesLookupTrack {
    track_name: Option<String>,
    artist_name: Option<String>,
    track_view_url: Option<String>,
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

    /// Fetch all tracks of an album by its iTunes collection ID.
    /// Uses the lookup API: /lookup?id={collectionId}&entity=song
    async fn get_album_tracks(&self, collection_id: u64) -> Result<Vec<ItunesLookupTrack>> {
        let url = format!(
            "https://itunes.apple.com/lookup?id={}&entity=song&country=NL",
            collection_id
        );
        tracing::info!(url = %url, "Apple Music: fetching album tracks");
        let body = self.api_get(&url).await?;
        let resp: ItunesLookupResponse = serde_json::from_str(&body)?;

        // The first result is the album itself (wrapperType=collection),
        // subsequent results are tracks (wrapperType=track).
        let tracks: Vec<ItunesLookupTrack> = resp.results.into_iter()
            .filter(|v| v.get("wrapperType").and_then(|w| w.as_str()) == Some("track"))
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        Ok(tracks)
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

    /// Album search + tracklist fetch: 2 API calls instead of N per-track searches.
    async fn check_album_tracks(
        &self,
        artist: &str,
        album: &str,
        track_titles: &[String],
        threshold: f64,
    ) -> Result<Option<AlbumTracksResult>> {
        // 1. Album search
        let query = format!("{} {}", artist, album);
        let url = format!(
            "https://itunes.apple.com/search?term={}&entity=album&limit=5&country=NL",
            urlenccode(&query)
        );
        tracing::info!(url = %url, "Apple Music: album search (with tracks)");

        let body = self.api_get(&url).await?;
        let resp: ItunesAlbumResponse = serde_json::from_str(&body)?;

        let best_album = resp.results.iter().filter_map(|a| {
            let artist_name = a.artist_name.as_deref()?;
            let collection_name = a.collection_name.as_deref()?;
            let s = mm_matcher::score(artist, album, artist_name, collection_name);
            if s >= threshold { Some((s, a)) } else { None }
        }).max_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap());

        let (_, matched_album) = match best_album {
            Some(a) => a,
            None => {
                return Ok(Some(AlbumTracksResult {
                    platform: "apple_music".to_owned(),
                    album_found: false,
                    album_url: None,
                    track_matches: Vec::new(),
                }));
            }
        };

        let collection_id = match matched_album.collection_id {
            Some(id) => id,
            None => {
                // No collection ID available, can't fetch tracks
                return Ok(None);
            }
        };

        let album_url = matched_album.collection_view_url.clone();
        tracing::info!("Apple Music: album found, fetching tracklist for {}", collection_id);

        // 2. Fetch tracklist
        let album_tracks = self.get_album_tracks(collection_id).await?;
        tracing::info!("Apple Music: album tracks fetched ({} tracks)", album_tracks.len());

        // 3. Fuzzy-match each expected track
        let track_matches: Vec<TrackMatch> = track_titles.iter().map(|expected| {
            let best = album_tracks.iter().filter_map(|at| {
                let at_artist = at.artist_name.as_deref().unwrap_or_default();
                let at_title = at.track_name.as_deref().unwrap_or_default();
                let s = mm_matcher::score(artist, expected, at_artist, at_title);
                if s >= threshold { Some((s, at)) } else { None }
            }).max_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap());

            match best {
                Some((s, at)) => TrackMatch {
                    track_title: expected.clone(),
                    found: true,
                    score: Some(s),
                    platform_url: at.track_view_url.clone(),
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
            platform: "apple_music".to_owned(),
            album_found: true,
            album_url,
            track_matches,
        }))
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
