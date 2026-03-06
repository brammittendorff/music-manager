//! iTunes Search API - free, no API key required.
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

pub struct ItunesChecker {
    http: Client,
    limiter: Arc<governor::DefaultDirectRateLimiter>,
}

impl ItunesChecker {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Client::builder().timeout(Duration::from_secs(30)).build()?,
            // iTunes Search API: ~20 req/min recommended
            limiter: Arc::new(RateLimiter::direct(
                Quota::per_minute(NonZeroU32::new(20).unwrap()),
            )),
        })
    }
}

#[async_trait::async_trait]
impl PlatformChecker for ItunesChecker {
    fn name(&self) -> &str {
        "apple_music"
    }

    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult> {
        self.limiter.until_ready().await;
        let query = format!("{} {}", artist, title);
        let url = format!(
            "https://itunes.apple.com/search?term={}&entity=song&limit=5&country=NL",
            urlenccode(&query)
        );
        tracing::info!(url = %url, "Apple Music: searching");

        let resp = self
            .http
            .get(&url)
            .send()
            .await?
            .json::<ItunesResponse>()
            .await?;

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
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}
