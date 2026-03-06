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
                .user_agent("Mozilla/5.0 (compatible; music-manager/0.1)")
                .build()?,
            // Bandcamp scraping: be polite, 10 req/min
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
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}
