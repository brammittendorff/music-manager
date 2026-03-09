use anyhow::Result;
use governor::{Quota, RateLimiter};
use mm_matcher::best_match;
use rusty_ytdl::search::{SearchOptions, SearchResult, SearchType, YouTube};
use std::num::NonZeroU32;
use std::sync::Arc;

use crate::{PlatformChecker, PlatformResult};

pub struct YoutubeChecker {
    yt: YouTube,
    limiter: Arc<governor::DefaultDirectRateLimiter>,
}

impl YoutubeChecker {
    pub fn new() -> Result<Self> {
        Ok(Self {
            yt: YouTube::new().map_err(|e| anyhow::anyhow!("{e}"))?,
            // 1 req/sec to avoid IP-level 429s from YouTube
            limiter: Arc::new(RateLimiter::direct(
                Quota::per_second(NonZeroU32::new(1).unwrap()),
            )),
        })
    }
}

#[async_trait::async_trait]
impl PlatformChecker for YoutubeChecker {
    fn name(&self) -> &str {
        "youtube_music"
    }

    async fn check_album(
        &self,
        artist: &str,
        album: &str,
        threshold: f64,
    ) -> Result<Option<PlatformResult>> {
        let query = sanitize_query(&format!("{} {}", artist, album));
        tracing::info!(query = %query, "YouTube Music: album search");

        self.limiter.until_ready().await;

        let opts = SearchOptions {
            limit: 5,
            search_type: SearchType::Playlist,
            ..Default::default()
        };

        let results = self
            .yt
            .search(&query, Some(&opts))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let candidates: Vec<(String, String, Option<String>)> = results
            .iter()
            .filter_map(|r| match r {
                SearchResult::Playlist(p) => Some((
                    strip_topic_suffix(&p.channel.name),
                    p.name.clone(),
                    Some(format!(
                        "https://music.youtube.com/playlist?list={}",
                        p.id
                    )),
                )),
                _ => None,
            })
            .collect();

        match best_match(artist, album, &candidates, threshold) {
            Some(m) => {
                tracing::info!(
                    "YouTube Music: album found: {} - {}",
                    m.candidate_artist,
                    m.candidate_title
                );
                Ok(Some(PlatformResult::found("youtube_music", m)))
            }
            None => Ok(Some(PlatformResult::not_found("youtube_music"))),
        }
    }

    async fn check(
        &self,
        artist: &str,
        title: &str,
        threshold: f64,
    ) -> Result<PlatformResult> {
        let query = sanitize_query(&format!("{} {}", artist, title));
        tracing::info!(query = %query, "YouTube Music: track search");

        self.limiter.until_ready().await;

        let opts = SearchOptions {
            limit: 5,
            search_type: SearchType::Video,
            ..Default::default()
        };

        let results = self
            .yt
            .search(&query, Some(&opts))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let candidates: Vec<(String, String, Option<String>)> = results
            .iter()
            .filter_map(|r| match r {
                SearchResult::Video(v) => Some((
                    strip_topic_suffix(&v.channel.name),
                    v.title.clone(),
                    Some(format!(
                        "https://music.youtube.com/watch?v={}",
                        v.id
                    )),
                )),
                _ => None,
            })
            .collect();

        match best_match(artist, title, &candidates, threshold) {
            Some(m) => Ok(PlatformResult::found("youtube_music", m)),
            None => Ok(PlatformResult::not_found("youtube_music")),
        }
    }
}

/// YouTube Music auto-generated channels use "Artist - Topic" as the channel
/// name. Strip the suffix so it matches the raw artist name.
fn strip_topic_suffix(name: &str) -> String {
    name.strip_suffix(" - Topic")
        .unwrap_or(name)
        .to_owned()
}

/// Sanitize the query before passing it to rusty_ytdl.
/// Double quotes in the query cause a JSON parse panic inside the library.
fn sanitize_query(query: &str) -> String {
    query.replace('"', "")
}
