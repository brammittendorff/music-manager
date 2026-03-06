//! Compute disc TOC and look up MusicBrainz disc ID.
//!
//! The disc ID is a base64-encoded SHA-1 hash of the track offsets.
//! We compute it by running `cd-discid` (Linux) or reading the TOC via ffprobe (Windows).

use anyhow::{Context, Result};
use mm_config::AppConfig;
use serde::Deserialize;
use tokio::process::Command;
#[allow(unused_imports)]
use tracing::{debug, info};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct DiscInfo {
    pub disc_id: Option<String>,
    pub musicbrainz_release_id: Option<Uuid>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub track_count: Option<u32>,
    pub tracks: Vec<MbTrack>,
}

#[derive(Debug, Clone)]
pub struct MbTrack {
    pub number: u32,
    pub title: String,
    pub artist: String,
    pub length_ms: Option<u32>,
    pub mbid: Option<Uuid>,
}

pub async fn lookup_disc_toc(drive_path: &str, cfg: &AppConfig) -> Result<DiscInfo> {
    let disc_id = compute_disc_id(drive_path).await?;

    info!("Disc ID: {disc_id:?}");

    match &disc_id {
        Some(id) => lookup_musicbrainz(id, &cfg.api.musicbrainz_user_agent).await,
        None => Ok(DiscInfo {
            disc_id: None,
            musicbrainz_release_id: None,
            title: None,
            artist: None,
            track_count: None,
            tracks: vec![],
        }),
    }
}

/// Compute the MusicBrainz disc ID from the optical drive.
/// Uses `cd-discid` on Linux or `ffprobe` on Windows.
async fn compute_disc_id(drive_path: &str) -> Result<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        // cd-discid outputs: <discid> <track_count> <offset1> <offset2> ... <total_secs>
        let out = Command::new("cd-discid")
            .arg(drive_path)
            .output()
            .await
            .context("cd-discid not found - install: apt install cd-discid")?;

        if !out.status.success() {
            return Ok(None);
        }

        // Convert CDDB disc ID to MusicBrainz disc ID format
        // For now we return the CDDB ID - a proper MB disc ID requires
        // computing the SHA-1 from TOC offsets (see libdiscid)
        let cddb_id = String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .map(str::to_owned);
        return Ok(cddb_id);
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, use ffprobe to read the TOC
        let out = Command::new("ffprobe")
            .args([
                "-i",
                &format!("cdda:{}", drive_path),
                "-show_chapters",
                "-print_format",
                "json",
                "-v",
                "quiet",
            ])
            .output()
            .await
            .context("ffprobe not found - install ffmpeg")?;

        if !out.status.success() {
            return Ok(None);
        }

        // Extract chapter count as a proxy - real disc ID needs TOC offsets
        let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
        let chapters = json["chapters"].as_array().map(|a| a.len());
        debug!("Windows disc chapters: {chapters:?}");

        // Return None here - MB lookup not available without proper disc ID
        return Ok(None);
    }

    #[allow(unreachable_code)]
    Ok(None)
}

#[derive(Debug, Deserialize)]
struct MbDiscResponse {
    releases: Option<Vec<MbRelease>>,
    #[serde(rename = "release-list")]
    release_list: Option<Vec<MbRelease>>,
}

#[derive(Debug, Deserialize)]
struct MbRelease {
    id: String,
    title: String,
    #[serde(rename = "artist-credit")]
    artist_credit: Option<Vec<MbArtistCredit>>,
    media: Option<Vec<MbMedia>>,
}

#[derive(Debug, Deserialize)]
struct MbArtistCredit {
    artist: MbArtist,
}

#[derive(Debug, Deserialize)]
struct MbArtist {
    name: String,
}

#[derive(Debug, Deserialize)]
struct MbMedia {
    tracks: Option<Vec<MbTrackRaw>>,
    #[serde(rename = "track-count")]
    track_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct MbTrackRaw {
    number: String,
    title: String,
    id: String,
    length: Option<u32>,
    recording: Option<MbRecording>,
}

#[derive(Debug, Deserialize)]
struct MbRecording {
    #[allow(dead_code)]
    id: String,
    #[serde(rename = "artist-credit")]
    artist_credit: Option<Vec<MbArtistCredit>>,
}

async fn lookup_musicbrainz(disc_id: &str, user_agent: &str) -> Result<DiscInfo> {
    // Rate limit: 1 req/s
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let http = reqwest::Client::builder()
        .user_agent(user_agent)
        .build()?;

    let url = format!(
        "https://musicbrainz.org/ws/2/discid/{disc_id}?inc=recordings+artists&fmt=json"
    );

    let resp = http.get(&url).send().await?;

    if resp.status() == 404 {
        info!("Disc not found in MusicBrainz: {disc_id}");
        return Ok(DiscInfo {
            disc_id: Some(disc_id.to_owned()),
            musicbrainz_release_id: None,
            title: None,
            artist: None,
            track_count: None,
            tracks: vec![],
        });
    }

    let data: MbDiscResponse = resp.json().await?;

    let release = data
        .releases
        .as_ref()
        .or(data.release_list.as_ref())
        .and_then(|r| r.first());

    let Some(release) = release else {
        return Ok(DiscInfo {
            disc_id: Some(disc_id.to_owned()),
            musicbrainz_release_id: None,
            title: None,
            artist: None,
            track_count: None,
            tracks: vec![],
        });
    };

    let release_id = release.id.parse::<Uuid>().ok();
    let artist = release
        .artist_credit
        .as_ref()
        .and_then(|ac| ac.first())
        .map(|ac| ac.artist.name.clone());

    let media = release.media.as_ref().and_then(|m| m.first());
    let track_count = media.and_then(|m| m.track_count);

    let tracks = media
        .and_then(|m| m.tracks.as_ref())
        .map(|raw_tracks| {
            raw_tracks
                .iter()
                .map(|t| {
                    let track_artist = t
                        .recording
                        .as_ref()
                        .and_then(|r| r.artist_credit.as_ref())
                        .and_then(|ac| ac.first())
                        .map(|ac| ac.artist.name.clone())
                        .or_else(|| artist.clone())
                        .unwrap_or_default();

                    let mbid = t.id.parse::<Uuid>().ok();

                    MbTrack {
                        number: t.number.parse().unwrap_or(0),
                        title: t.title.clone(),
                        artist: track_artist,
                        length_ms: t.length,
                        mbid,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(DiscInfo {
        disc_id: Some(disc_id.to_owned()),
        musicbrainz_release_id: release_id,
        title: Some(release.title.clone()),
        artist,
        track_count,
        tracks,
    })
}
