//! Rip pipeline: rip WAV tracks → split if needed → encode to MP3 → tag → organize.

use anyhow::{bail, Context, Result};
use mm_config::AppConfig;
use mm_db::{models::Track, queries, Db};
use mm_discogs::RipReleaseInfo;
use mm_encoder::encode_wav_to_mp3;
use mm_tagger::{TagInfo, tag_mp3};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, info, warn};
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

    /// Run the rip pipeline using MusicBrainz metadata (original flow).
    pub async fn run(&self, drive_path: &str, disc: &DiscInfo) -> Result<RipResult> {
        let temp_dir = PathBuf::from(&self.cfg.ripper.temp_dir).join(self.job_id.to_string());
        tokio::fs::create_dir_all(&temp_dir).await?;

        queries::update_rip_job_status(&self.pool, self.job_id, "ripping", None).await?;

        // Step 1: Rip all tracks to WAV
        let wav_files = self.rip_to_wav(drive_path, &temp_dir).await?;
        info!("Ripped {} WAV tracks", wav_files.len());

        queries::update_rip_job_status(&self.pool, self.job_id, "encoding", None).await?;

        // Step 2: Build output directory
        let artist = disc.artist.as_deref().unwrap_or("Unknown Artist");
        let album = disc.title.as_deref().unwrap_or("Unknown Album");
        let output_dir = self.build_output_dir(artist, album, None);
        tokio::fs::create_dir_all(&output_dir).await?;

        let total_tracks = disc.track_count.or(Some(wav_files.len() as u32));
        let mut ripped_tracks = Vec::new();

        for (i, wav_path) in wav_files.iter().enumerate() {
            let track_num = (i + 1) as u32;

            let mb_track = disc.tracks.iter().find(|t| t.number == track_num);
            let title = mb_track.map(|t| t.title.clone())
                .unwrap_or_else(|| format!("Track {:02}", track_num));
            let track_artist = mb_track.map(|t| t.artist.clone())
                .or_else(|| disc.artist.clone())
                .unwrap_or_else(|| "Unknown Artist".to_owned());

            let ripped = self.encode_and_tag_track(
                wav_path,
                &output_dir,
                track_num,
                &title,
                &track_artist,
                album,
                None, // year
                total_tracks,
                mb_track.and_then(|t| t.mbid).map(|u| u.to_string()),
                disc.musicbrainz_release_id.map(|u| u.to_string()),
                mb_track.and_then(|t| t.length_ms),
                None, // cover art
            ).await?;

            ripped_tracks.push(ripped);
        }

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        Ok(RipResult {
            output_dir: output_dir.to_string_lossy().to_string(),
            tracks: ripped_tracks,
        })
    }

    /// Run the rip pipeline using Discogs metadata.
    /// Handles single-track CDs by splitting with silence detection + Discogs durations.
    pub async fn run_with_discogs(
        &self,
        drive_path: &str,
        release: &RipReleaseInfo,
        discogs: &mm_discogs::DiscogsClient,
    ) -> Result<RipResult> {
        let temp_dir = PathBuf::from(&self.cfg.ripper.temp_dir).join(self.job_id.to_string());
        tokio::fs::create_dir_all(&temp_dir).await?;

        queries::update_rip_job_status(&self.pool, self.job_id, "ripping", None).await?;

        // Step 1: Rip to WAV
        let wav_files = self.rip_to_wav(drive_path, &temp_dir).await?;
        info!("Ripped {} WAV file(s)", wav_files.len());

        // Step 2: Determine if we need to split
        let expected_tracks = release.tracklist.len();
        let wav_files = if wav_files.len() == 1 && expected_tracks > 1 {
            info!(
                "Single WAV file but {} tracks expected — splitting",
                expected_tracks
            );
            queries::update_rip_job_status(&self.pool, self.job_id, "splitting", None).await?;
            self.split_single_file(&wav_files[0], &release.tracklist, &temp_dir)
                .await?
        } else {
            wav_files
        };

        queries::update_rip_job_status(&self.pool, self.job_id, "encoding", None).await?;

        // Step 3: Build output directory: Artist/Year - Album/
        let output_dir = self.build_output_dir(
            &release.artist,
            &release.title,
            release.year,
        );
        tokio::fs::create_dir_all(&output_dir).await?;

        // Step 4: Download cover art
        let cover_art = if let Some(ref url) = release.cover_art_url {
            match discogs.download_cover_art(url).await {
                Ok(bytes) => {
                    // Save cover.jpg alongside MP3s
                    let cover_path = output_dir.join("cover.jpg");
                    if let Err(e) = tokio::fs::write(&cover_path, &bytes).await {
                        warn!("Failed to save cover.jpg: {e}");
                    } else {
                        info!("Saved cover art to {}", cover_path.display());
                    }
                    Some(bytes)
                }
                Err(e) => {
                    warn!("Failed to download cover art: {e}");
                    None
                }
            }
        } else {
            None
        };

        // Step 5: Encode, tag, and organize each track
        let total_tracks = Some(expected_tracks as u32);
        let mut ripped_tracks = Vec::new();

        for (i, wav_path) in wav_files.iter().enumerate() {
            let track_num = (i + 1) as u32;

            let (title, duration_ms) = if i < release.tracklist.len() {
                let (ref t, d) = release.tracklist[i];
                (t.clone(), d)
            } else {
                (format!("Track {:02}", track_num), None)
            };

            let ripped = self.encode_and_tag_track(
                wav_path,
                &output_dir,
                track_num,
                &title,
                &release.artist,
                &release.title,
                release.year,
                total_tracks,
                None, // no MusicBrainz track ID
                None, // no MusicBrainz release ID
                duration_ms,
                cover_art.as_deref(),
            ).await?;

            ripped_tracks.push(ripped);
        }

        // Cleanup temp
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        Ok(RipResult {
            output_dir: output_dir.to_string_lossy().to_string(),
            tracks: ripped_tracks,
        })
    }

    /// Encode a single WAV to MP3, tag it, insert DB record, return result.
    #[allow(clippy::too_many_arguments)]
    async fn encode_and_tag_track(
        &self,
        wav_path: &Path,
        output_dir: &Path,
        track_num: u32,
        title: &str,
        artist: &str,
        album: &str,
        year: Option<i32>,
        total_tracks: Option<u32>,
        musicbrainz_track_id: Option<String>,
        musicbrainz_release_id: Option<String>,
        duration_ms: Option<u32>,
        cover_art: Option<&[u8]>,
    ) -> Result<RippedTrack> {
        let file_stem = self.format_filename(track_num, title);

        // Encode WAV → MP3
        let mp3_path = output_dir.join(format!("{file_stem}.mp3"));
        encode_wav_to_mp3(wav_path, &mp3_path, self.cfg.ripper.bitrate_kbps)?;

        // Tag MP3
        let tag = TagInfo {
            title: title.to_owned(),
            artist: artist.to_owned(),
            album: album.to_owned(),
            year,
            track_number: track_num,
            total_tracks,
            musicbrainz_track_id,
            musicbrainz_release_id,
            cover_art: cover_art.map(|b| b.to_vec()),
        };
        tag_mp3(&mp3_path, &tag)?;

        // Optionally keep FLAC master
        let flac_path = if self.cfg.ripper.keep_flac {
            let p = output_dir.join(format!("{file_stem}.flac"));
            mm_encoder::encode_wav_to_flac(wav_path, &p)?;
            Some(p)
        } else {
            None
        };

        let file_size = tokio::fs::metadata(&mp3_path).await?.len();

        // Insert track record
        let track = Track {
            id: Uuid::new_v4(),
            rip_job_id: self.job_id,
            release_id: None,
            track_number: track_num as i32,
            title: Some(title.to_owned()),
            artist: Some(artist.to_owned()),
            album: Some(album.to_owned()),
            year,
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
            musicbrainz_id: None,
            created_at: chrono::Utc::now(),
        };
        queries::insert_track(&self.pool, &track).await?;

        info!("Track {track_num:02}: {artist} - {title} → {}", mp3_path.display());

        Ok(RippedTrack {
            number: track_num,
            title: title.to_owned(),
            mp3_path,
            flac_path,
            duration_ms,
        })
    }

    // ─── Single-file splitting ──────────────────────────────────────────────────

    /// Split a single WAV file into individual tracks.
    /// Priority: CUE sheet → cdrdao TOC → silence detection + Discogs durations → durations only.
    async fn split_single_file(
        &self,
        wav_path: &Path,
        tracklist: &[(String, Option<u32>)],
        temp_dir: &Path,
    ) -> Result<Vec<PathBuf>> {
        let expected_count = tracklist.len();

        // 1. Check for a CUE sheet next to the WAV file
        let cue_path = wav_path.with_extension("cue");
        if cue_path.exists() {
            info!("Found CUE sheet: {}", cue_path.display());
            if let Ok(splits) = self.parse_cue_split_points(&cue_path).await {
                if splits.len() == expected_count - 1 || !splits.is_empty() {
                    info!("Using CUE sheet split points ({} boundaries)", splits.len());
                    return self.split_wav_at_points(wav_path, &splits, expected_count, temp_dir).await;
                }
            }
            warn!("CUE sheet parse failed or wrong track count — trying other methods");
        }

        // Also check for CUE in temp dir (some tools put it there)
        let cue_in_temp = temp_dir.join("disc.cue");
        if cue_in_temp.exists() {
            info!("Found CUE sheet in temp dir: {}", cue_in_temp.display());
            if let Ok(splits) = self.parse_cue_split_points(&cue_in_temp).await {
                if !splits.is_empty() {
                    info!("Using CUE sheet split points ({} boundaries)", splits.len());
                    return self.split_wav_at_points(wav_path, &splits, expected_count, temp_dir).await;
                }
            }
        }

        // 2. Try cdrdao to read TOC from disc (if drive is still available)
        if let Ok(splits) = self.try_cdrdao_toc(temp_dir).await {
            if splits.len() == expected_count - 1 {
                info!("Using cdrdao TOC split points ({} boundaries)", splits.len());
                return self.split_wav_at_points(wav_path, &splits, expected_count, temp_dir).await;
            }
            if !splits.is_empty() {
                debug!("cdrdao found {} splits but expected {} — falling through", splits.len(), expected_count - 1);
            }
        }

        // 3. Silence detection cross-referenced with Discogs durations
        let total_duration_ms = self.get_wav_duration_ms(wav_path).await?;
        info!("WAV total duration: {:.1}s", total_duration_ms as f64 / 1000.0);

        let silence_points = self.detect_silence(wav_path).await?;
        info!("Found {} silence gaps in audio", silence_points.len());

        let expected_splits = self.compute_expected_splits(tracklist, total_duration_ms);

        let split_points = if !silence_points.is_empty() && !expected_splits.is_empty() {
            // Best case: match silence positions to expected Discogs boundaries
            info!("Matching {} silence gaps to {} expected boundaries", silence_points.len(), expected_splits.len());
            self.match_splits_to_silence(&expected_splits, &silence_points)
        } else if silence_points.len() == expected_count - 1 {
            // Exact number of silence gaps matches expected tracks
            info!("Silence gaps match expected track count exactly");
            silence_points
        } else if !expected_splits.is_empty() {
            // 4. Last resort: Discogs durations only
            warn!("No usable silence gaps — using Discogs durations directly");
            expected_splits
        } else {
            bail!(
                "Cannot split: no CUE sheet, no silence detected, and no track durations from Discogs"
            );
        };

        info!("Splitting into {} tracks at: {:?}",
            split_points.len() + 1,
            split_points.iter().map(|ms| format!("{:.1}s", *ms as f64 / 1000.0)).collect::<Vec<_>>()
        );

        self.split_wav_at_points(wav_path, &split_points, expected_count, temp_dir).await
    }

    /// Parse a CUE sheet and extract split points in milliseconds.
    /// CUE timestamps are MM:SS:FF where FF = frames at 75fps (Red Book standard).
    async fn parse_cue_split_points(&self, cue_path: &Path) -> Result<Vec<u32>> {
        let content = tokio::fs::read_to_string(cue_path).await?;
        let mut splits = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();
            // Look for INDEX 01 lines (start of actual track audio)
            if trimmed.starts_with("INDEX 01") || trimmed.starts_with("INDEX  01") {
                if let Some(timestamp) = trimmed.split_whitespace().nth(2) {
                    if let Some(ms) = parse_cue_timestamp(timestamp) {
                        // Skip the first track's INDEX 01 (it's at 00:00:00)
                        if ms > 0 {
                            splits.push(ms);
                        }
                    }
                }
            }
        }

        debug!("Parsed {} CUE split points", splits.len());
        Ok(splits)
    }

    /// Try to read TOC using cdrdao and extract split points.
    async fn try_cdrdao_toc(&self, temp_dir: &Path) -> Result<Vec<u32>> {
        let toc_path = temp_dir.join("disc.toc");

        // Check if we already have a .toc file
        if !toc_path.exists() {
            // cdrdao needs the drive — bail if this is a manual rip without drive access
            return Ok(Vec::new());
        }

        // Convert .toc to .cue using toc2cue
        let cue_path = temp_dir.join("disc.cue");
        let status = Command::new("toc2cue")
            .args([
                toc_path.to_str().unwrap(),
                cue_path.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        match status {
            Ok(s) if s.success() && cue_path.exists() => {
                self.parse_cue_split_points(&cue_path).await
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Get WAV file duration in milliseconds using ffprobe.
    async fn get_wav_duration_ms(&self, wav_path: &Path) -> Result<u32> {
        let output = Command::new("ffprobe")
            .args([
                "-v", "quiet",
                "-show_entries", "format=duration",
                "-of", "default=noprint_wrappers=1:nokey=1",
                wav_path.to_str().unwrap(),
            ])
            .output()
            .await
            .context("ffprobe not found")?;

        let duration_str = String::from_utf8_lossy(&output.stdout);
        let secs: f64 = duration_str.trim().parse()
            .context("Failed to parse duration from ffprobe")?;
        Ok((secs * 1000.0) as u32)
    }

    /// Detect silence gaps in a WAV file using ffmpeg's silencedetect filter.
    /// Returns midpoints of each silence gap in milliseconds, sorted by position.
    /// Uses -45dB threshold (typical CD track gaps) and minimum 0.8s silence duration.
    async fn detect_silence(&self, wav_path: &Path) -> Result<Vec<u32>> {
        let output = Command::new("ffmpeg")
            .args([
                "-i", wav_path.to_str().unwrap(),
                "-af", "silencedetect=noise=-45dB:d=0.8",
                "-f", "null",
                "/dev/null",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("ffmpeg silencedetect failed")?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut points = Vec::new();
        let mut silence_start: Option<f64> = None;

        for line in stderr.lines() {
            if let Some(pos) = line.find("silence_start: ") {
                let val = &line[pos + 16..];
                if let Some(end) = val.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-') {
                    silence_start = val[..end].parse().ok();
                } else {
                    silence_start = val.trim().parse().ok();
                }
            } else if let Some(pos) = line.find("silence_end: ") {
                let val = &line[pos + 13..];
                let end_time: Option<f64> = if let Some(end) = val.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-') {
                    val[..end].parse().ok()
                } else {
                    val.trim().parse().ok()
                };
                if let (Some(start), Some(end)) = (silence_start, end_time) {
                    // Use midpoint of silence gap as split point
                    let midpoint_ms = ((start + end) / 2.0 * 1000.0) as u32;
                    points.push(midpoint_ms);
                    silence_start = None;
                }
            }
        }

        Ok(points)
    }

    /// Compute expected split points from cumulative Discogs track durations.
    /// Returns cumulative boundary positions in milliseconds (excluding the last track end).
    fn compute_expected_splits(
        &self,
        tracklist: &[(String, Option<u32>)],
        _total_duration_ms: u32,
    ) -> Vec<u32> {
        let mut splits = Vec::new();
        let mut cumulative = 0u32;

        // We need N-1 split points for N tracks
        for (i, (_title, duration_ms)) in tracklist.iter().enumerate() {
            if i == tracklist.len() - 1 {
                break; // Don't add split after last track
            }
            if let Some(dur) = duration_ms {
                cumulative += dur;
                splits.push(cumulative);
            }
        }

        splits
    }

    /// Match expected split points to nearest silence positions.
    /// For each expected boundary, find the closest silence point within a tolerance window.
    fn match_splits_to_silence(
        &self,
        expected: &[u32],
        silence_points: &[u32],
    ) -> Vec<u32> {
        const TOLERANCE_MS: u32 = 15_000; // 15 second tolerance window

        let mut result = Vec::new();
        let mut used_silence = std::collections::HashSet::new();

        for &expected_ms in expected {
            // Find closest unused silence point within tolerance
            let best = silence_points
                .iter()
                .enumerate()
                .filter(|(idx, _)| !used_silence.contains(idx))
                .filter(|(_, &sp)| {
                    let diff = if sp > expected_ms { sp - expected_ms } else { expected_ms - sp };
                    diff <= TOLERANCE_MS
                })
                .min_by_key(|(_, &sp)| {
                    if sp > expected_ms { sp - expected_ms } else { expected_ms - sp }
                });

            if let Some((idx, &silence_ms)) = best {
                debug!(
                    "Expected split at {:.1}s → matched silence at {:.1}s",
                    expected_ms as f64 / 1000.0,
                    silence_ms as f64 / 1000.0
                );
                result.push(silence_ms);
                used_silence.insert(idx);
            } else {
                debug!(
                    "Expected split at {:.1}s → no silence nearby, using duration",
                    expected_ms as f64 / 1000.0
                );
                result.push(expected_ms);
            }
        }

        result
    }

    /// Split a WAV file at the given millisecond positions.
    async fn split_wav_at_points(
        &self,
        wav_path: &Path,
        split_points: &[u32],
        expected_count: usize,
        temp_dir: &Path,
    ) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let mut starts = vec![0u32];
        starts.extend_from_slice(split_points);

        let mut ends: Vec<Option<u32>> = split_points.iter().map(|&p| Some(p)).collect();
        ends.push(None); // Last track runs to end

        let track_count = starts.len().min(expected_count);

        for i in 0..track_count {
            let out_path = temp_dir.join(format!("split_{:02}.wav", i + 1));
            let start_secs = starts[i] as f64 / 1000.0;

            let mut args = vec![
                "-i".to_string(),
                wav_path.to_str().unwrap().to_string(),
                "-ss".to_string(),
                format!("{start_secs:.3}"),
            ];

            if let Some(end_ms) = ends[i] {
                let end_secs = end_ms as f64 / 1000.0;
                args.extend(["-to".to_string(), format!("{end_secs:.3}")]);
            }

            args.extend([
                "-acodec".to_string(),
                "pcm_s16le".to_string(),
                "-ar".to_string(),
                "44100".to_string(),
                "-ac".to_string(),
                "2".to_string(),
                "-y".to_string(),
                out_path.to_str().unwrap().to_string(),
            ]);

            let status = Command::new("ffmpeg")
                .args(&args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .context("ffmpeg split failed")?;

            if !status.success() {
                warn!("ffmpeg failed to split track {}", i + 1);
                continue;
            }

            files.push(out_path);
        }

        if files.is_empty() {
            bail!("No tracks produced after splitting");
        }

        info!("Split into {} WAV files", files.len());
        Ok(files)
    }

    // ─── Ripping backends ───────────────────────────────────────────────────────

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

    // ─── Path helpers ───────────────────────────────────────────────────────────

    /// Build output directory: local_path/Artist/Year - Album/
    fn build_output_dir(&self, artist: &str, album: &str, year: Option<i32>) -> PathBuf {
        let artist = sanitize_path(artist);
        let album_dir = match year {
            Some(y) => format!("{y} - {}", sanitize_path(album)),
            None => sanitize_path(album),
        };

        PathBuf::from(&self.cfg.storage.local_path)
            .join(&artist)
            .join(&album_dir)
    }

    fn format_filename(&self, track_num: u32, title: &str) -> String {
        let title = sanitize_path(title);
        format!("{:02} - {}", track_num, title)
    }
}

/// Parse a CUE sheet timestamp "MM:SS:FF" to milliseconds.
/// FF = frames at 75fps (Red Book CD standard).
fn parse_cue_timestamp(ts: &str) -> Option<u32> {
    let parts: Vec<&str> = ts.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let mins: u32 = parts[0].parse().ok()?;
    let secs: u32 = parts[1].parse().ok()?;
    let frames: u32 = parts[2].parse().ok()?;
    // 75 frames per second (Red Book standard)
    Some(mins * 60_000 + secs * 1000 + (frames * 1000) / 75)
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
