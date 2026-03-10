//! Bandcamp has no public API. We use their search endpoint with polite scraping.
//! Bandcamp search: https://bandcamp.com/search?q=QUERY&item_type=t (tracks)
use anyhow::Result;
use governor::{Quota, RateLimiter};
use mm_matcher::best_match;
use reqwest::Client;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use crate::{PlatformChecker, PlatformResult};

pub struct BandcampChecker {
    http: Client,
    limiter: Arc<governor::DefaultDirectRateLimiter>,
}

impl BandcampChecker {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .local_address("0.0.0.0".parse().ok())
                .user_agent("Mozilla/5.0 (compatible; music-manager/0.1)")
                .build()?,
            // Bandcamp: no public API, scraping only. 10 req/min (~6s between requests).
            // No documented rate limit; Cloudflare blocks are site-configured.
            // Typical Cloudflare thresholds are ~20 req/10s; 10/min is still very polite.
            limiter: Arc::new(RateLimiter::direct(
                Quota::per_minute(NonZeroU32::new(10).unwrap()),
            )),
        })
    }

    /// Parse track results from Bandcamp search HTML.
    /// Returns vec of (artist, title, url).
    fn parse_results(html: &str) -> Vec<(String, String, Option<String>)> {
        let mut results = Vec::new();

        // Bandcamp search results contain structured data in the HTML.
        // We look for <li class="searchresult track"> blocks.
        for block in html.split(r#"class="searchresult track""#).skip(1) {
            let title = extract_between(block, r#"class="heading">"#, "</");
            let artist = extract_between(block, r#"class="subhead">"#, "</");
            let url = extract_between(block, r#"class="itemurl">"#, "<");

            if let (Some(t), Some(a)) = (title, artist) {
                // Strip "by " prefix from artist field
                let artist = a.trim_start_matches("by ").trim().to_owned();
                results.push((artist, t.trim().to_owned(), url.map(|u| u.trim().to_owned())));
            }
        }

        results
    }

    /// Parse album results from Bandcamp search HTML.
    /// Returns vec of (artist, album_title, url).
    fn parse_album_results(html: &str) -> Vec<(String, String, Option<String>)> {
        let mut results = Vec::new();

        // Album results use <li class="searchresult album"> blocks.
        for block in html.split(r#"class="searchresult album""#).skip(1) {
            let title = extract_between(block, r#"class="heading">"#, "</");
            let artist = extract_between(block, r#"class="subhead">"#, "</");
            let url = extract_between(block, r#"class="itemurl">"#, "<");

            if let (Some(t), Some(a)) = (title, artist) {
                let artist = a.trim_start_matches("by ").trim().to_owned();
                results.push((artist, t.trim().to_owned(), url.map(|u| u.trim().to_owned())));
            }
        }

        results
    }
}

fn extract_between<'a>(s: &'a str, start: &str, end: &str) -> Option<String> {
    let i = s.find(start)?;
    let rest = &s[i + start.len()..];
    let j = rest.find(end)?;
    Some(rest[..j].to_owned())
}

#[async_trait::async_trait]
impl PlatformChecker for BandcampChecker {
    fn name(&self) -> &str {
        "bandcamp"
    }

    fn skip_tracks_if_album_not_found(&self) -> bool {
        true
    }

    /// Album-level check: search Bandcamp for the album by artist + album title.
    /// Uses item_type=a to search for albums instead of tracks.
    async fn check_album(&self, artist: &str, album: &str, threshold: f64) -> Result<Option<PlatformResult>> {
        self.limiter.until_ready().await;
        let query = format!("{} {}", artist, album);
        let url = format!(
            "https://bandcamp.com/search?q={}&item_type=a",
            urlenccode(&query)
        );
        tracing::info!(url = %url, "Bandcamp: album search");

        let html = self.http.get(&url).send().await?.text().await?;
        let candidates = Self::parse_album_results(&html);

        match best_match(artist, album, &candidates, threshold) {
            Some(m) => {
                tracing::info!("Bandcamp: album found: {} - {}", m.candidate_artist, m.candidate_title);
                Ok(Some(PlatformResult::found("bandcamp", m)))
            }
            None => Ok(Some(PlatformResult::not_found("bandcamp"))),
        }
    }

    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult> {
        self.limiter.until_ready().await;
        let query = format!("{} {}", artist, title);
        let url = format!(
            "https://bandcamp.com/search?q={}&item_type=t",
            urlenccode(&query)
        );
        tracing::info!(url = %url, "Bandcamp: searching");

        let html = self.http.get(&url).send().await?.text().await?;
        let candidates = Self::parse_results(&html);

        match best_match(artist, title, &candidates, threshold) {
            Some(m) => Ok(PlatformResult::found("bandcamp", m)),
            None => Ok(PlatformResult::not_found("bandcamp")),
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
