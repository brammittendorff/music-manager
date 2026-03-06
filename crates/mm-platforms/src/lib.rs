pub mod bandcamp;
pub mod deezer;
pub mod itunes;
pub mod rate_limits;
pub mod spotify;
pub mod youtube;

use anyhow::Result;
use mm_config::AppConfig;
use mm_matcher::MatchResult;

// ─── Platform check result ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PlatformResult {
    pub platform: String,
    pub found: bool,
    pub match_result: Option<MatchResult>,
}

impl PlatformResult {
    pub fn not_found(platform: &str) -> Self {
        Self {
            platform: platform.to_owned(),
            found: false,
            match_result: None,
        }
    }

    pub fn found(platform: &str, m: MatchResult) -> Self {
        Self {
            platform: platform.to_owned(),
            found: true,
            match_result: Some(m),
        }
    }
}

// ─── Platform checker trait ───────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait PlatformChecker: Send + Sync {
    fn name(&self) -> &str;
    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult>;
}

// ─── Checker coordinator ──────────────────────────────────────────────────────

pub struct PlatformCoordinator {
    checkers: Vec<Box<dyn PlatformChecker>>,
}

impl PlatformCoordinator {
    pub async fn new(cfg: &AppConfig) -> Result<Self> {
        let mut checkers: Vec<Box<dyn PlatformChecker>> = Vec::new();

        // Spotify - requires client_id + client_secret
        if cfg.api.spotify_client_id.is_empty() {
            tracing::warn!(
                "Spotify checker skipped: set MMGR_API__SPOTIFY_CLIENT_ID and \
                 MMGR_API__SPOTIFY_CLIENT_SECRET to enable it"
            );
        } else {
            let c = spotify::SpotifyChecker::new(cfg).await?;
            checkers.push(Box::new(c));
        }

        // YouTube - requires an API key (10,000 quota units/day free)
        if cfg.api.youtube_api_key.is_empty() {
            tracing::warn!(
                "YouTube checker skipped: set MMGR_API__YOUTUBE_API_KEY to enable it \
                 (free key at https://console.cloud.google.com)"
            );
        } else {
            let c = youtube::YoutubeChecker::new(cfg)?;
            checkers.push(Box::new(c));
        }

        // Deezer - free, no key required
        {
            let c = deezer::DeezerChecker::new()?;
            checkers.push(Box::new(c));
        }

        // iTunes / Apple Music - free, no key required
        {
            let c = itunes::ItunesChecker::new()?;
            checkers.push(Box::new(c));
        }

        // Bandcamp - free scraping, no key required
        {
            let c = bandcamp::BandcampChecker::new()?;
            checkers.push(Box::new(c));
        }

        let names: Vec<&str> = checkers.iter().map(|c| c.name()).collect();
        tracing::info!(
            "Loaded {} platform checker(s): {}",
            names.len(),
            names.join(", ")
        );

        Ok(Self { checkers })
    }

    /// Check all configured platforms concurrently for a given artist + title.
    pub async fn check_all(
        &self,
        artist: &str,
        title: &str,
        threshold: f64,
    ) -> Vec<PlatformResult> {
        let tasks: Vec<_> = self.checkers.iter().map(|checker| {
            checker.check(artist, title, threshold)
        }).collect();

        futures::future::join_all(tasks)
            .await
            .into_iter()
            .zip(self.checkers.iter())
            .map(|(result, checker)| match result {
                Ok(r) => {
                    tracing::info!(platform = checker.name(), found = r.found, "Platform check result");
                    r
                }
                Err(e) => {
                    tracing::warn!(platform = checker.name(), error = %e, "Check failed");
                    PlatformResult::not_found(checker.name())
                }
            })
            .collect()
    }
}
