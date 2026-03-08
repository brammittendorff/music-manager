//! Documented rate limits for every platform supported by music-manager.
//!
//! All durations represent the *minimum delay between consecutive requests*
//! that should be enforced in the worker to stay well inside official limits
//! and remain a polite client.
//!
//! Each platform checker also has its own `governor` rate limiter instance.
//! These constants serve as documentation and can be referenced elsewhere.

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
/// Spotify Web API: rolling 30-second window, exact limit undisclosed.
/// Community reports ~250 req/30s for client credentials, but this varies.
/// Repeated 429s can escalate to 24-hour bans.
/// We use 1 req/sec (30 req/30s) to stay well under the limit.
/// Always honor Retry-After headers; implement exponential backoff.
/// Source: https://developer.spotify.com/documentation/web-api/concepts/rate-limits
pub const SPOTIFY_DELAY: Duration = Duration::from_secs(1);

// ─── YouTube Data API v3 ──────────────────────────────────────────────────────
/// Default daily quota: 10,000 units.
/// search.list costs 100 units per call → max 100 searches per day.
/// videos.list/channels.list cost 1 unit each (prefer these when possible).
/// Quota resets at midnight Pacific Time.
/// Source: https://developers.google.com/youtube/v3/determine_quota_cost
pub const YOUTUBE_QUOTA_UNITS_PER_DAY: u32 = 10_000;
pub const YOUTUBE_SEARCH_COST_UNITS: u32 = 100;
pub const YOUTUBE_MAX_SEARCHES_PER_DAY: u32 = YOUTUBE_QUOTA_UNITS_PER_DAY / YOUTUBE_SEARCH_COST_UNITS;

/// Conservative delay between YouTube search.list calls (15 seconds).
/// With 100 searches/day, spacing them out prevents burning the quota too fast.
pub const YOUTUBE_DELAY: Duration = Duration::from_secs(15);

// ─── Deezer ───────────────────────────────────────────────────────────────────
/// Deezer: ~50 requests per 5 seconds (community-sourced, not in official docs).
/// Returns error code 4 ("Quota limit exceeded") when hit.
/// We use 1 req/sec to stay well under the limit.
/// Source: https://developers.deezer.com/guidelines
pub const DEEZER_REQUESTS_PER_5_SEC: u32 = 50;
pub const DEEZER_DELAY: Duration = Duration::from_secs(1);

// ─── MusicBrainz ─────────────────────────────────────────────────────────────
/// MusicBrainz explicitly requires no more than 1 request per second from any
/// single IP. Exceeding this may result in the IP being blocked.
/// Source: https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting
pub const MUSICBRAINZ_REQUESTS_PER_SECOND: u32 = 1;
pub const MUSICBRAINZ_DELAY: Duration = Duration::from_secs(1);

// ─── iTunes / Apple Search API ────────────────────────────────────────────────
/// Apple's iTunes Search API: ~20 req/min official, but 403s start earlier.
/// Returns 403 Forbidden (not 429) when rate limited, with no Retry-After header.
/// We use 10/min (1 req/6sec) based on real-world developer reports.
/// Source: https://developer.apple.com/forums/thread/66399
pub const ITUNES_REQUESTS_PER_MINUTE: u32 = 10;
pub const ITUNES_DELAY: Duration = Duration::from_secs(6);

// ─── Bandcamp ─────────────────────────────────────────────────────────────────
/// Bandcamp has no public API; we scrape HTML search pages.
/// No published limits. Aggressive scraping triggers Cloudflare blocks.
/// "Keeping Bandcamp Human" policy prohibits scraping (Jan 2026).
/// We use 4 req/min (1 req/15sec) to be as polite as possible.
pub const BANDCAMP_DELAY: Duration = Duration::from_secs(15);
