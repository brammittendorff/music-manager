use anyhow::{bail, Result};
use mm_config::AppConfig;
use mm_matcher::best_match;
use reqwest::Client;
use serde::Deserialize;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use crate::{PlatformChecker, PlatformResult};

#[derive(Debug, Deserialize)]
struct YoutubeSearchResponse {
    items: Vec<YoutubeItem>,
}

#[derive(Debug, Deserialize)]
struct YoutubeItem {
    id: ItemId,
    snippet: Snippet,
}

#[derive(Debug, Deserialize)]
struct ItemId {
    #[serde(rename = "videoId")]
    video_id: Option<String>,
    #[serde(rename = "playlistId")]
    playlist_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Snippet {
    title: String,
    #[serde(rename = "channelTitle")]
    channel_title: String,
}

pub struct YoutubeChecker {
    http: Client,
    api_key: String,
    /// Daily quota units consumed (resets at midnight PT).
    quota_used: AtomicU32,
    /// Epoch millis when the quota counter was last reset.
    quota_reset_at: AtomicU64,
}

/// YouTube Data API v3: 10,000 units/day, search.list = 100 units each.
const DAILY_QUOTA: u32 = 10_000;
const SEARCH_COST: u32 = 100;

impl YoutubeChecker {
    pub fn new(cfg: &AppConfig) -> Result<Self> {
        if cfg.api.youtube_api_key.is_empty() {
            bail!("YouTube API key not configured");
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .local_address("0.0.0.0".parse().ok())
                .build()?,
            api_key: cfg.api.youtube_api_key.clone(),
            quota_used: AtomicU32::new(0),
            quota_reset_at: AtomicU64::new(now),
        })
    }

    /// Check if we have enough daily quota remaining. Resets after 24h.
    fn consume_quota(&self, cost: u32) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let reset_at = self.quota_reset_at.load(Ordering::Relaxed);
        // Reset quota after 24 hours
        if now - reset_at > 24 * 60 * 60 * 1000 {
            self.quota_used.store(0, Ordering::Relaxed);
            self.quota_reset_at.store(now, Ordering::Relaxed);
            tracing::info!("YouTube: daily quota reset");
        }

        let used = self.quota_used.fetch_add(cost, Ordering::Relaxed);
        if used + cost > DAILY_QUOTA {
            self.quota_used.fetch_sub(cost, Ordering::Relaxed);
            let remaining_hours = (24 * 60 * 60 * 1000 - (now - self.quota_reset_at.load(Ordering::Relaxed))) / 3_600_000;
            anyhow::bail!(
                "YouTube: daily quota exhausted ({used}/{DAILY_QUOTA} units used). Resets in ~{remaining_hours}h"
            );
        }
        tracing::debug!("YouTube: quota {}/{} units used", used + cost, DAILY_QUOTA);
        Ok(())
    }

    /// Rate-limited API call with quota tracking and pacing.
    async fn rate_limited_search(&self, url: &str) -> Result<YoutubeSearchResponse> {
        self.consume_quota(SEARCH_COST)?;

        // Small delay to avoid bursting through the daily 100-search quota too fast.
        // The main loop already adds 3s between releases; this adds a bit more for YouTube.
        tokio::time::sleep(Duration::from_secs(2)).await;

        let resp = self.http.get(url).send().await?;
        let status = resp.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status == reqwest::StatusCode::FORBIDDEN {
            let body = resp.text().await.unwrap_or_default();
            if body.contains("quotaExceeded") || body.contains("rateLimitExceeded") {
                // Mark quota as exhausted
                self.quota_used.store(DAILY_QUOTA, Ordering::Relaxed);
                anyhow::bail!("YouTube: quota exceeded (API confirmed)");
            }
            anyhow::bail!("YouTube API error {status}: {body}");
        }

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("YouTube API returned {status}: {body}");
        }

        Ok(resp.json().await?)
    }
}

#[async_trait::async_trait]
impl PlatformChecker for YoutubeChecker {
    fn name(&self) -> &str {
        "youtube_music"
    }

    async fn check_album(&self, artist: &str, album: &str, threshold: f64) -> Result<Option<PlatformResult>> {
        let query = format!("{} {}", artist, album);
        let url = format!(
            "https://www.googleapis.com/youtube/v3/search\
             ?part=snippet&q={}&type=playlist&maxResults=5&key={}",
            urlenccode(&query),
            self.api_key
        );
        tracing::info!(url = %url, "YouTube Music: album search");

        let data = self.rate_limited_search(&url).await?;

        let candidates: Vec<(String, String, Option<String>)> = data
            .items
            .iter()
            .filter_map(|item| {
                let playlist_id = item.id.playlist_id.as_ref()?;
                Some((
                    item.snippet.channel_title.clone(),
                    item.snippet.title.clone(),
                    Some(format!("https://music.youtube.com/playlist?list={playlist_id}")),
                ))
            })
            .collect();

        match best_match(artist, album, &candidates, threshold) {
            Some(m) => {
                tracing::info!("YouTube Music: album found: {} - {}", m.candidate_artist, m.candidate_title);
                Ok(Some(PlatformResult::found("youtube_music", m)))
            }
            None => Ok(Some(PlatformResult::not_found("youtube_music"))),
        }
    }

    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult> {
        let query = format!("{} {}", artist, title);
        let url = format!(
            "https://www.googleapis.com/youtube/v3/search\
             ?part=snippet&q={}&type=video&videoCategoryId=10&maxResults=5&key={}",
            urlenccode(&query),
            self.api_key
        );

        let data = self.rate_limited_search(&url).await?;

        let candidates: Vec<(String, String, Option<String>)> = data
            .items
            .iter()
            .filter_map(|item| {
                let video_id = item.id.video_id.as_ref()?;
                Some((
                    item.snippet.channel_title.clone(),
                    item.snippet.title.clone(),
                    Some(format!("https://music.youtube.com/watch?v={video_id}")),
                ))
            })
            .collect();

        match best_match(artist, title, &candidates, threshold) {
            Some(m) => Ok(PlatformResult::found("youtube_music", m)),
            None => Ok(PlatformResult::not_found("youtube_music")),
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
