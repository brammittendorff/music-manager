use anyhow::{bail, Result};
use mm_config::AppConfig;
use mm_matcher::best_match;
use reqwest::Client;
use serde::Deserialize;
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
}

impl YoutubeChecker {
    pub fn new(cfg: &AppConfig) -> Result<Self> {
        if cfg.api.youtube_api_key.is_empty() {
            bail!("YouTube API key not configured");
        }
        Ok(Self {
            http: Client::builder().timeout(Duration::from_secs(30)).build()?,
            api_key: cfg.api.youtube_api_key.clone(),
        })
    }
}

#[async_trait::async_trait]
impl PlatformChecker for YoutubeChecker {
    fn name(&self) -> &str {
        "youtube_music"
    }

    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult> {
        let query = format!("{} {}", artist, title);
        let url = format!(
            "https://www.googleapis.com/youtube/v3/search\
             ?part=snippet&q={}&type=video&videoCategoryId=10&maxResults=5&key={}",
            urlenccode(&query),
            self.api_key
        );

        let resp = self
            .http
            .get(&url)
            .send()
            .await?
            .json::<YoutubeSearchResponse>()
            .await?;

        let candidates: Vec<(String, String, Option<String>)> = resp
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
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}
