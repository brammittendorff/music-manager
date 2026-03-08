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
    /// True when the check failed due to a transient error (429, timeout, etc.)
    /// The UI should show this as "error" (yellow) instead of "missing" (red).
    pub error: bool,
    pub match_result: Option<MatchResult>,
}

impl PlatformResult {
    pub fn not_found(platform: &str) -> Self {
        Self {
            platform: platform.to_owned(),
            found: false,
            error: false,
            match_result: None,
        }
    }

    pub fn found(platform: &str, m: MatchResult) -> Self {
        Self {
            platform: platform.to_owned(),
            found: true,
            error: false,
            match_result: Some(m),
        }
    }

    pub fn errored(platform: &str) -> Self {
        Self {
            platform: platform.to_owned(),
            found: false,
            error: true,
            match_result: None,
        }
    }
}

// ─── Platform checker trait ───────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait PlatformChecker: Send + Sync {
    fn name(&self) -> &str;
    async fn check(&self, artist: &str, title: &str, threshold: f64) -> Result<PlatformResult>;

    /// Album-level check: search for the album directly instead of track-by-track.
    /// Returns Some(result) if the platform supports album search, None to fall back
    /// to per-track checking. Default: None (not supported).
    async fn check_album(&self, _artist: &str, _album: &str, _threshold: f64) -> Result<Option<PlatformResult>> {
        Ok(None)
    }
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

    /// Album-level check: try each platform's check_album method concurrently.
    /// Returns results only for platforms that support album search (e.g. Spotify).
    /// Platforms that return None are not included in the result.
    pub async fn check_albums(
        &self,
        artist: &str,
        album: &str,
        threshold: f64,
    ) -> Vec<PlatformResult> {
        let tasks: Vec<_> = self.checkers.iter().map(|checker| {
            let fut = checker.check_album(artist, album, threshold);
            let name = checker.name().to_owned();
            async move {
                match tokio::time::timeout(std::time::Duration::from_secs(15), fut).await {
                    Ok(Ok(Some(r))) => {
                        tracing::info!(platform = %name, found = r.found, "Album-level check result");
                        Some(r)
                    }
                    Ok(Ok(None)) => None, // Platform doesn't support album search
                    Ok(Err(e)) => {
                        tracing::warn!(platform = %name, error = %e, "Album check failed");
                        Some(PlatformResult::errored(&name))
                    }
                    Err(_) => {
                        tracing::warn!(platform = %name, "Album check timed out after 15s");
                        Some(PlatformResult::errored(&name))
                    }
                }
            }
        }).collect();

        futures::future::join_all(tasks).await.into_iter().flatten().collect()
    }

    /// Check all configured platforms concurrently for a given artist + title.
    /// Each platform gets its own 15-second timeout so a slow/blocked platform
    /// (e.g. Spotify 429) never holds up the others.
    pub async fn check_all(
        &self,
        artist: &str,
        title: &str,
        threshold: f64,
    ) -> Vec<PlatformResult> {
        let tasks: Vec<_> = self.checkers.iter().map(|checker| {
            let fut = checker.check(artist, title, threshold);
            let name = checker.name().to_owned();
            async move {
                match tokio::time::timeout(std::time::Duration::from_secs(15), fut).await {
                    Ok(Ok(r)) => {
                        tracing::info!(platform = %name, found = r.found, "Platform check result");
                        r
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(platform = %name, error = %e, "Check failed");
                        PlatformResult::errored(&name)
                    }
                    Err(_) => {
                        tracing::warn!(platform = %name, "Check timed out after 15s");
                        PlatformResult::errored(&name)
                    }
                }
            }
        }).collect();

        futures::future::join_all(tasks).await
    }
}
