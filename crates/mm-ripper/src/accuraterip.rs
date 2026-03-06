//! AccurateRip checksum verification.
//!
//! AccurateRip verifies that your rip matches known-good rips by other users.
//! The database URL is: http://www.accuraterip.com/accuraterip/{a}/{b}/{c}/{discid}.bin
//! where a,b,c are the 1st, 2nd, 3rd hex digits of the first track offset CRC.

use anyhow::Result;
use reqwest::Client;
use std::path::Path;
use tracing::{debug, warn};

/// Result of verifying a track against AccurateRip.
#[derive(Debug, Clone)]
pub enum AccurateRipStatus {
    Verified { confidence: u32 },
    NotInDatabase,
    Mismatch,
}

/// Compute the AccurateRip CRC v1 for a WAV file.
/// The CRC covers all samples except the first and last 5 sectors.
pub fn compute_crc_v1(wav_path: &Path) -> Result<u32> {
    let data = std::fs::read(wav_path)?;

    // WAV PCM data starts at byte 44 (standard header)
    // Each sector = 2352 bytes = 588 samples × 4 bytes (16-bit stereo)
    const SECTOR_SIZE: usize = 2352;
    const SKIP_SECTORS: usize = 5;
    const HEADER: usize = 44;

    if data.len() <= HEADER + SKIP_SECTORS * SECTOR_SIZE * 2 {
        return Ok(0);
    }

    let audio = &data[HEADER..];
    let samples_to_skip = SKIP_SECTORS * 588; // 588 samples per sector
    let total_samples = audio.len() / 4;

    let mut crc: u32 = 0;
    let mut mult: u32 = 1;

    for i in samples_to_skip..(total_samples.saturating_sub(samples_to_skip)) {
        let sample = i32::from_le_bytes([
            audio[i * 4],
            audio[i * 4 + 1],
            audio[i * 4 + 2],
            audio[i * 4 + 3],
        ]) as u32;
        crc = crc.wrapping_add(sample.wrapping_mul(mult));
        mult = mult.wrapping_add(1);
    }

    Ok(crc)
}

/// Verify a track CRC against the AccurateRip database.
pub async fn verify(
    http: &Client,
    disc_id_1: u32,
    disc_id_2: u32,
    cddb_disc_id: u32,
    track_crc: u32,
    track_number: u32,
) -> Result<AccurateRipStatus> {
    let a = disc_id_1 & 0xF;
    let b = (disc_id_1 >> 4) & 0xF;
    let c = (disc_id_1 >> 8) & 0xF;

    let url = format!(
        "http://www.accuraterip.com/accuraterip/{a:x}/{b:x}/{c:x}/dBAR-{disc_id_1:08x}-{disc_id_2:08x}-{cddb_disc_id:08x}.bin"
    );

    debug!("AccurateRip lookup: {url}");

    let resp = match http.get(&url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(_) => return Ok(AccurateRipStatus::NotInDatabase),
        Err(e) => {
            warn!("AccurateRip request failed: {e}");
            return Ok(AccurateRipStatus::NotInDatabase);
        }
    };

    let data = resp.bytes().await?;

    // Parse binary AccurateRip response
    // Format: repeated chunks of: [track_count(1)] [disc_id_1(4)] [disc_id_2(4)] [cddb_id(4)]
    //         followed by per-track: [confidence(1)] [crc(4)] [crc450(4)]
    let mut offset = 0;
    while offset + 13 <= data.len() {
        let chunk_track_count = data[offset] as usize;
        offset += 13; // skip header

        for t in 0..chunk_track_count {
            if offset + 9 > data.len() {
                break;
            }
            let confidence = data[offset];
            let stored_crc = u32::from_le_bytes([
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
            ]);
            offset += 9;

            if t + 1 == track_number as usize && stored_crc == track_crc {
                return Ok(AccurateRipStatus::Verified {
                    confidence: confidence as u32,
                });
            }
        }
    }

    Ok(AccurateRipStatus::Mismatch)
}
