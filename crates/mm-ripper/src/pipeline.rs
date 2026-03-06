//! Rip pipeline: rip WAV tracks → encode to MP3 → tag → organize.

use anyhow::{bail, Context, Result};
use mm_config::AppConfig;
use mm_db::{models::Track, queries, Db};
use mm_encoder::encode_wav_to_mp3;
use mm_tagger::{TagInfo, tag_mp3};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{info, warn};
use uuid::Uuid;

use crate::musicbrainz::DiscInfo;

pub struct RipResult {
    pub output_dir: String,
    pub tracks: Vec<RippedTrack>,
}

pub struct RippedTrack {
    pub number: u32,
    pub title: String,
    pub mp3_path: PathBuf,
    pub flac_path: Option<PathBuf>,
    pub duration_ms: Option<u32>,
}

pub struct RipPipeline {
    cfg: AppConfig,
    job_id: Uuid,
    pool: Db,
}

impl RipPipeline {
    pub fn new(cfg: AppConfig, job_id: Uuid, pool: Db) -> Self {
        Self { cfg, job_id, pool }
    }

    pub async fn run(&self, drive_path: &str, disc: &DiscInfo) -> Result<RipResult> {
        let temp_dir = PathBuf::from(&self.cfg.ripper.temp_dir).join(self.job_id.to_string());
        tokio::fs::create_dir_all(&temp_dir).await?;

        queries::update_rip_job_status(&self.pool, self.job_id, "ripping", None).await?;

        // Step 1: Rip all tracks to WAV
        let wav_files = self.rip_to_wav(drive_path, &temp_dir).await?;
        info!("Ripped {} WAV tracks", wav_files.len());

        queries::update_rip_job_status(&self.pool, self.job_id, "encoding", None).await?;

        // Step 2: Build output directory path
        let output_dir = self.build_output_dir(disc);
        tokio::fs::create_dir_all(&output_dir).await?;

        let mut ripped_tracks = Vec::new();

        for (i, wav_path) in wav_files.iter().enumerate() {
            let track_num = (i + 1) as u32;

            // Metadata from MusicBrainz if available
            let mb_track = disc.tracks.iter().find(|t| t.number == track_num);
            let title = mb_track.map(|t| t.title.clone())
                .unwrap_or_else(|| format!("Track {:02}", track_num));
            let artist = mb_track.map(|t| t.artist.clone())
                .or_else(|| disc.artist.clone())
                .unwrap_or_else(|| "Unknown Artist".to_owned());

            let file_stem = self.format_filename(track_num, &title);

            // Step 3: Encode WAV → MP3 320kbps
            let mp3_path = output_dir.join(format!("{file_stem}.mp3"));
            encode_wav_to_mp3(wav_path, &mp3_path, self.cfg.ripper.bitrate_kbps)?;

            // Step 4: Tag MP3 with ID3 metadata
            let tag = TagInfo {
                title: title.clone(),
                artist: artist.clone(),
                album: disc.title.clone().unwrap_or_else(|| "Unknown Album".to_owned()),
                year: None, // Could be fetched from MB release
                track_number: track_num,
                total_tracks: disc.track_count,
                musicbrainz_track_id: mb_track.and_then(|t| t.mbid).map(|u| u.to_string()),
                musicbrainz_release_id: disc.musicbrainz_release_id.map(|u| u.to_string()),
            };
            tag_mp3(&mp3_path, &tag)?;

            // Step 5: Optionally keep FLAC master
            let flac_path = if self.cfg.ripper.keep_flac {
                let p = output_dir.join(format!("{file_stem}.flac"));
                mm_encoder::encode_wav_to_flac(wav_path, &p)?;
                Some(p)
            } else {
                None
            };

            // Step 6: Get file metadata
            let file_size = tokio::fs::metadata(&mp3_path).await?.len();
            let duration_ms = mb_track.and_then(|t| t.length_ms);

            // Step 7: Insert track record into DB
            let track = Track {
                id: Uuid::new_v4(),
                rip_job_id: self.job_id,
                release_id: None,
                track_number: track_num as i32,
                title: Some(title.clone()),
                artist: Some(artist.clone()),
                album: disc.title.clone(),
                year: None,
                file_path: mp3_path.to_string_lossy().to_string(),
                file_format: "mp3".to_owned(),
                bitrate_kbps: Some(self.cfg.ripper.bitrate_kbps as i32),
                sample_rate: Some(44100),
                channels: Some(2),
                duration_ms: duration_ms.map(|d| d as i32),
                file_size_bytes: Some(file_size as i64),
                accuraterip_v1: None,
                accuraterip_v2: None,
                accuraterip_ok: None,
                musicbrainz_id: mb_track.and_then(|t| t.mbid),
                created_at: chrono::Utc::now(),
            };
            queries::insert_track(&self.pool, &track).await?;

            info!("Track {track_num:02}: {artist} - {title} → {}", mp3_path.display());

            ripped_tracks.push(RippedTrack {
                number: track_num,
                title,
                mp3_path,
                flac_path,
                duration_ms,
            });
        }

        // Cleanup temp WAV files
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        Ok(RipResult {
            output_dir: output_dir.to_string_lossy().to_string(),
            tracks: ripped_tracks,
        })
    }

    /// Rip all tracks from drive to WAV files in temp_dir.
    async fn rip_to_wav(&self, drive_path: &str, temp_dir: &Path) -> Result<Vec<PathBuf>> {
        let backend = if cfg!(target_os = "linux") {
            &self.cfg.ripper.backend_linux
        } else {
            &self.cfg.ripper.backend_windows
        };

        match backend.as_str() {
            "cdparanoia" => self.rip_cdparanoia(drive_path, temp_dir).await,
            "ffmpeg" => self.rip_ffmpeg(drive_path, temp_dir).await,
            other => bail!("Unknown rip backend: {other}"),
        }
    }

    /// Linux: use cdparanoia in batch mode - rips all tracks to track01.wav, track02.wav, …
    async fn rip_cdparanoia(&self, drive_path: &str, temp_dir: &Path) -> Result<Vec<PathBuf>> {
        info!("Ripping with cdparanoia from {drive_path}");

        let status = Command::new("cdparanoia")
            .args(["-B", "-d", drive_path, "--"])
            .current_dir(temp_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await
            .context("cdparanoia not found - install: apt install cdparanoia")?;

        if !status.success() {
            bail!("cdparanoia exited with status: {status}");
        }

        self.collect_wav_files(temp_dir).await
    }

    /// Windows (and fallback): use ffmpeg to rip each track from CDDA.
    async fn rip_ffmpeg(&self, drive_path: &str, temp_dir: &Path) -> Result<Vec<PathBuf>> {
        info!("Ripping with ffmpeg from {drive_path}");

        // First, probe how many tracks exist
        let probe = Command::new("ffprobe")
            .args([
                "-i", &format!("cdda:{drive_path}"),
                "-show_chapters",
                "-print_format", "json",
                "-v", "quiet",
            ])
            .output()
            .await
            .context("ffprobe not found - install ffmpeg")?;

        let json: serde_json::Value = serde_json::from_slice(&probe.stdout)
            .unwrap_or(serde_json::json!({"chapters": []}));
        let track_count = json["chapters"].as_array().map(|a| a.len()).unwrap_or(0);

        if track_count == 0 {
            // Fall back: try ripping the whole disc as one file
            warn!("ffprobe found 0 chapters - attempting single-file rip");
            let out_path = temp_dir.join("track01.wav");
            Command::new("ffmpeg")
                .args([
                    "-i", &format!("cdda:{drive_path}"),
                    "-ar", "44100",
                    "-ac", "2",
                    "-acodec", "pcm_s16le",
                    out_path.to_str().unwrap(),
                ])
                .status()
                .await?;
            return Ok(vec![out_path]);
        }

        let mut files = Vec::new();
        for i in 1..=track_count {
            let out_path = temp_dir.join(format!("track{i:02}.wav"));
            let status = Command::new("ffmpeg")
                .args([
                    "-i", &format!("cdda:{drive_path}"),
                    "-map_chapters", &i.to_string(),
                    "-ar", "44100",
                    "-ac", "2",
                    "-acodec", "pcm_s16le",
                    out_path.to_str().unwrap(),
                ])
                .status()
                .await?;

            if status.success() {
                files.push(out_path);
            } else {
                warn!("ffmpeg failed for track {i}");
            }
        }

        Ok(files)
    }

    async fn collect_wav_files(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let mut entries = tokio::fs::read_dir(dir).await?;
        let mut files = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let p = entry.path();
            if p.extension().map_or(false, |e| e == "wav") {
                files.push(p);
            }
        }

        files.sort();
        Ok(files)
    }

    fn build_output_dir(&self, disc: &DiscInfo) -> PathBuf {
        let artist = disc.artist.as_deref().unwrap_or("Unknown Artist");
        let album = disc.title.as_deref().unwrap_or("Unknown Album");
        // Sanitize path components
        let artist = sanitize_path(artist);
        let album = sanitize_path(album);

        PathBuf::from(&self.cfg.storage.local_path)
            .join(&artist)
            .join(&album)
    }

    fn format_filename(&self, track_num: u32, title: &str) -> String {
        let title = sanitize_path(title);
        format!("{:02} - {}", track_num, title)
    }
}

fn sanitize_path(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_owned()
}
