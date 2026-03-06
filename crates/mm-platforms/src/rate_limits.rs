//! Documented rate limits for every platform supported by music-manager.
//!
//! All durations represent the *minimum delay between consecutive requests*
//! that should be enforced in the worker to stay well inside official limits
//! and remain a polite client.

use std::time::Duration;

// ─── Discogs ──────────────────────────────────────────────────────────────────
/// Discogs allows 60 authenticated requests per minute (personal access token).
/// Unauthenticated requests are limited to 25/minute.
/// We use 55/min to leave headroom.
/// Source: https://www.discogs.com/developers/
pub const DISCOGS_REQUESTS_PER_MINUTE: u32 = 55;

/// Minimum delay between Discogs API requests (~1.09 s).
pub const DISCOGS_DELAY: Duration = Duration::from_millis(1_091);

// ─── Spotify ──────────────────────────────────────────────────────────────────
/// Spotify Web API rate limit is based on a rolling 30-second window.
/// Spotify does not publish a precise per-endpoint number, but community
/// experience puts it around 180 requests per 30 seconds (~6 req/s).
/// We use 1 req/sec to be conservative and avoid 429s.
/// Source: https://developer.spotify.com/documentation/web-api/concepts/rate-limits
pub const SPOTIFY_DELAY: Duration = Duration::from_secs(1);

// ─── YouTube Data API v3 ──────────────────────────────────────────────────────
/// Default daily quota: 10,000 units.
/// search.list costs 100 units per call → max 100 searches per day.
/// To spread those evenly across a 24-hour day: one search every 14.4 minutes.
/// We round up to 15 minutes (900 seconds) to stay safely under the limit.
/// Source: https://developers.google.com/youtube/v3/getting-started
pub const YOUTUBE_QUOTA_UNITS_PER_DAY: u32 = 10_000;
pub const YOUTUBE_SEARCH_COST_UNITS: u32 = 100;
pub const YOUTUBE_MAX_SEARCHES_PER_DAY: u32 = YOUTUBE_QUOTA_UNITS_PER_DAY / YOUTUBE_SEARCH_COST_UNITS;

/// Conservative minimum delay between YouTube search.list calls (15 minutes).
pub const YOUTUBE_DELAY: Duration = Duration::from_secs(900);

// ─── Deezer ───────────────────────────────────────────────────────────────────
/// Deezer's public API imposes a quota of roughly 50 requests per 5 seconds.
/// No official per-second number is published; we use 1 req/sec to be safe.
/// Source: https://developers.deezer.com/api
pub const DEEZER_REQUESTS_PER_5_SEC: u32 = 50;
pub const DEEZER_DELAY: Duration = Duration::from_millis(100); // ~10 req/sec → well under 50/5s

// ─── MusicBrainz ─────────────────────────────────────────────────────────────
/// MusicBrainz explicitly requires no more than 1 request per second from any
/// single IP. Exceeding this may result in the IP being blocked.
/// Source: https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting
pub const MUSICBRAINZ_REQUESTS_PER_SECOND: u32 = 1;
pub const MUSICBRAINZ_DELAY: Duration = Duration::from_secs(1);

// ─── iTunes / Apple Search API ────────────────────────────────────────────────
/// Apple's iTunes Search API is officially limited to ~20 calls per minute.
/// Exceeding this returns HTTP 429. We use a 3-second delay (~20/min) as
/// observed by the developer community to be reliable.
/// Source: https://performance-partners.apple.com/search-api
pub const ITUNES_REQUESTS_PER_MINUTE: u32 = 20;
pub const ITUNES_DELAY: Duration = Duration::from_secs(3);

// ─── Bandcamp ─────────────────────────────────────────────────────────────────
/// Bandcamp has no public API; we scrape HTML pages.
/// There are no published limits. To be a polite scraper we wait at least
/// 3 seconds between requests. Using random jitter (±1 s) is also advisable.
pub const BANDCAMP_DELAY: Duration = Duration::from_secs(3);
