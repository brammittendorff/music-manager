use anyhow::Result;
use serde::Deserialize;
use std::time::Duration;
use tracing::debug;

// ─── Match score ──────────────────────────────────────────────────────────────

/// Combined artist + title similarity score using Jaro-Winkler.
/// Artist is weighted 40%, title 60%.
pub fn score(
    query_artist: &str,
    query_title: &str,
    candidate_artist: &str,
    candidate_title: &str,
) -> f64 {
    let artist_score = strsim::jaro_winkler(
        &normalize(query_artist),
        &normalize(candidate_artist),
    );
    let title_score = strsim::jaro_winkler(
        &normalize(query_title),
        &normalize(candidate_title),
    );
    0.4 * artist_score + 0.6 * title_score
}

/// A match result from a platform search.
#[derive(Debug, Clone)]
pub struct MatchResult {
    pub score: f64,
    pub platform_url: Option<String>,
    pub candidate_artist: String,
    pub candidate_title: String,
}

/// Find the best match from a list of candidates.
pub fn best_match(
    query_artist: &str,
    query_title: &str,
    candidates: &[(String, String, Option<String>)], // (artist, title, url)
    threshold: f64,
) -> Option<MatchResult> {
    candidates
        .iter()
        .map(|(a, t, url)| MatchResult {
            score: score(query_artist, query_title, a, t),
            platform_url: url.clone(),
            candidate_artist: a.clone(),
            candidate_title: t.clone(),
        })
        .filter(|m| m.score >= threshold)
        .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap())
}

/// Normalize a string for comparison: lowercase, strip punctuation, collapse spaces.
pub fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ─── MusicBrainz resolver ─────────────────────────────────────────────────────

const MB_BASE: &str = "https://musicbrainz.org/ws/2";

#[derive(Debug, Deserialize)]
struct MbRecordingResponse {
    recordings: Vec<MbRecording>,
}

#[derive(Debug, Deserialize)]
pub struct MbRecording {
    pub id: String,
    pub title: String,
    #[serde(rename = "artist-credit")]
    pub artist_credit: Option<Vec<MbArtistCredit>>,
    pub releases: Option<Vec<MbRelease>>,
    pub length: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct MbArtistCredit {
    pub artist: MbArtist,
}

#[derive(Debug, Deserialize)]
pub struct MbArtist {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct MbRelease {
    pub id: String,
    pub title: String,
    pub date: Option<String>,
}

pub struct MusicBrainzResolver {
    http: reqwest::Client,
}

impl MusicBrainzResolver {
    pub fn new(user_agent: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            // MusicBrainz requires at most 1 request/second
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http })
    }

    /// Search MusicBrainz for a recording by artist + title.
    /// Returns the best-matching recording ID (MBID), or None.
    pub async fn resolve_recording(
        &self,
        artist: &str,
        title: &str,
        threshold: f64,
    ) -> Result<Option<MbRecording>> {
        // Throttle: MusicBrainz allows 1 req/sec
        tokio::time::sleep(Duration::from_millis(1100)).await;

        let query = format!(
            "recording:\"{}\" AND artist:\"{}\"",
            escape_mb(title),
            escape_mb(artist)
        );
        let url = format!(
            "{MB_BASE}/recording/?query={}&fmt=json&limit=5",
            urlenccode(&query)
        );

        debug!("MusicBrainz query: {url}");

        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            return Ok(None);
        }

        let data: MbRecordingResponse = resp.json().await?;

        let best = data
            .recordings
            .into_iter()
            .filter_map(|rec| {
                let mb_artist = rec
                    .artist_credit
                    .as_ref()?
                    .first()
                    .map(|ac| ac.artist.name.clone())
                    .unwrap_or_default();
                let s = score(artist, title, &mb_artist, &rec.title);
                if s >= threshold { Some((s, rec)) } else { None }
            })
            .max_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap())
            .map(|(_, rec)| rec);

        Ok(best)
    }

    /// Look up a disc by MusicBrainz disc ID (computed from TOC during ripping).
    pub async fn lookup_disc(&self, disc_id: &str) -> Result<serde_json::Value> {
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let url = format!("{MB_BASE}/discid/{disc_id}?inc=recordings+artists&fmt=json");
        let resp = self.http.get(&url).send().await?;
        Ok(resp.json().await?)
    }
}

fn escape_mb(s: &str) -> String {
    s.replace('"', "\\\"")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize() {
        assert_eq!(normalize("André van Duin!"), "andré van duin");
        assert_eq!(normalize("De  Dijk"), "de dijk");
    }

    #[test]
    fn test_score_exact() {
        let s = score("Doe Maar", "Is Dit Alles", "Doe Maar", "Is Dit Alles");
        assert!(s > 0.99);
    }

    #[test]
    fn test_score_typo() {
        let s = score("Doe Maar", "Is Dit Alles", "Doe Maar", "Is Dit Alles?");
        assert!(s > 0.85);
    }
}
