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

pub struct DeezerChecker {
    http: Client,
    limiter: Arc<governor::DefaultDirectRateLimiter>,
}

impl DeezerChecker {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Client::builder().timeout(Duration::from_secs(30)).build()?,
            // Deezer: 50 req/5s officially, we use 40/min to be safe
            limiter: Arc::new(RateLimiter::direct(
                Quota::per_minute(NonZeroU32::new(40).unwrap()),
            )),
        })
    }
}

#[async_trait::async_trait]
impl PlatformChecker for DeezerChecker {
    fn name(&self) -> &str {
        "deezer"
    }

    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult> {
        self.limiter.until_ready().await;
        let query = format!("artist:\"{}\" track:\"{}\"", artist, title);
        let url = format!(
            "https://api.deezer.com/search?q={}&limit=5",
            urlenccode(&query)
        );
        tracing::info!(url = %url, "Deezer: searching");

        let resp = self
            .http
            .get(&url)
            .send()
            .await?
            .json::<DeezerSearchResponse>()
            .await?;

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
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}
