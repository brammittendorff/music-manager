//! Audio encoding: WAV → MP3 320kbps (via mp3lame-encoder) and WAV → FLAC (via flac CLI).

use anyhow::{bail, anyhow, Context, Result};
use std::path::Path;
use std::process::Command;
use tracing::info;

// ─── WAV header parsing ───────────────────────────────────────────────────────

#[derive(Debug)]
struct WavInfo {
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    data_offset: usize,
    data_len: usize,
}

fn parse_wav_header(data: &[u8]) -> Result<WavInfo> {
    if data.len() < 44 {
        bail!("WAV file too small");
    }
    if &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        bail!("Not a valid WAV file");
    }

    // Parse fmt chunk
    let channels = u16::from_le_bytes([data[22], data[23]]);
    let sample_rate = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
    let bits_per_sample = u16::from_le_bytes([data[34], data[35]]);

    // Find data chunk
    let mut offset = 12usize;
    loop {
        if offset + 8 > data.len() {
            bail!("WAV data chunk not found");
        }
        let chunk_id = &data[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            data[offset + 4],
            data[offset + 5],
            data[offset + 6],
            data[offset + 7],
        ]) as usize;

        if chunk_id == b"data" {
            return Ok(WavInfo {
                sample_rate,
                channels,
                bits_per_sample,
                data_offset: offset + 8,
                data_len: chunk_size,
            });
        }
        offset += 8 + chunk_size;
    }
}

// ─── MP3 encoding ─────────────────────────────────────────────────────────────

/// Encode a WAV file to MP3 at the specified bitrate (kbps) using libmp3lame.
pub fn encode_wav_to_mp3(wav_path: &Path, mp3_path: &Path, bitrate_kbps: u32) -> Result<()> {
    info!("Encoding {} → {}", wav_path.display(), mp3_path.display());

    let wav_data = std::fs::read(wav_path)
        .with_context(|| format!("Read WAV: {}", wav_path.display()))?;
    let wav = parse_wav_header(&wav_data)?;

    if wav.bits_per_sample != 16 {
        bail!("Only 16-bit WAV is supported, got {} bits", wav.bits_per_sample);
    }

    // Build LAME encoder
    use mp3lame_encoder::{Builder, FlushNoGap, MonoPcm, Quality, DualPcm, Bitrate};

    let mut builder = Builder::new()
        .ok_or_else(|| anyhow!("LAME: failed to create builder"))?;

    builder.set_num_channels(wav.channels as u8)
        .map_err(|e| anyhow!("LAME: set channels: {e:?}"))?;
    builder.set_sample_rate(wav.sample_rate)
        .map_err(|e| anyhow!("LAME: set sample rate: {e:?}"))?;
    builder.set_brate(match bitrate_kbps {
        320 => Bitrate::Kbps320,
        256 => Bitrate::Kbps256,
        192 => Bitrate::Kbps192,
        128 => Bitrate::Kbps128,
        _ => Bitrate::Kbps320,
    })
    .map_err(|e| anyhow!("LAME: set bitrate: {e:?}"))?;
    builder.set_quality(Quality::Best)
        .map_err(|e| anyhow!("LAME: set quality: {e:?}"))?;

    let mut encoder = builder.build()
        .map_err(|e| anyhow!("LAME: build encoder: {e:?}"))?;

    // Convert raw PCM bytes to i16 samples
    let audio = &wav_data[wav.data_offset..wav.data_offset + wav.data_len];
    let samples: Vec<i16> = audio
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();

    let mut mp3_out = Vec::with_capacity(samples.len() / 4);

    // LAME output buffer: guideline is 1.25 * num_samples + 7200 bytes per chunk
    const CHUNK: usize = 8192;
    let out_buf_size = (CHUNK as f64 * 1.25) as usize + 7200;

    let mut out_buf: Vec<std::mem::MaybeUninit<u8>> =
        vec![std::mem::MaybeUninit::uninit(); out_buf_size];

    // Helper to extract written bytes from the MaybeUninit output buffer
    macro_rules! take_written {
        ($n:expr) => {{
            // Safety: LAME wrote exactly $n bytes into out_buf[..$n]
            unsafe {
                std::slice::from_raw_parts(out_buf.as_ptr() as *const u8, $n)
            }
        }};
    }

    if wav.channels == 1 {
        for chunk in samples.chunks(CHUNK) {
            let written = encoder
                .encode(MonoPcm(chunk), &mut out_buf)
                .map_err(|e| anyhow!("LAME: encode chunk: {e:?}"))?;
            mp3_out.extend_from_slice(take_written!(written));
        }
    } else {
        // Interleaved stereo → split into L/R
        for chunk in samples.chunks(CHUNK * 2) {
            let left: Vec<i16> = chunk.iter().step_by(2).copied().collect();
            let right: Vec<i16> = chunk.iter().skip(1).step_by(2).copied().collect();
            let written = encoder
                .encode(DualPcm { left: &left, right: &right }, &mut out_buf)
                .map_err(|e| anyhow!("LAME: encode chunk: {e:?}"))?;
            mp3_out.extend_from_slice(take_written!(written));
        }
    }

    // Flush remaining buffered data
    let flush_buf_size = 7200usize;
    let mut flush_buf: Vec<std::mem::MaybeUninit<u8>> =
        vec![std::mem::MaybeUninit::uninit(); flush_buf_size];
    let written = encoder
        .flush::<FlushNoGap>(&mut flush_buf)
        .map_err(|e| anyhow!("LAME: flush: {e:?}"))?;
    mp3_out.extend_from_slice(unsafe {
        std::slice::from_raw_parts(flush_buf.as_ptr() as *const u8, written)
    });

    std::fs::write(mp3_path, &mp3_out)
        .with_context(|| format!("Write MP3: {}", mp3_path.display()))?;

    info!(
        "MP3 written: {} ({} KB)",
        mp3_path.display(),
        mp3_out.len() / 1024
    );
    Ok(())
}

// ─── FLAC encoding (via flac CLI) ────────────────────────────────────────────

/// Encode a WAV file to FLAC using the `flac` command-line tool.
/// This gives lossless compression without adding a Rust C binding.
pub fn encode_wav_to_flac(wav_path: &Path, flac_path: &Path) -> Result<()> {
    info!("Encoding {} → {}", wav_path.display(), flac_path.display());

    let status = Command::new("flac")
        .args([
            "--silent",
            "--best",
            "-o",
            flac_path.to_str().unwrap(),
            wav_path.to_str().unwrap(),
        ])
        .status()
        .context("flac CLI not found - install: apt install flac / choco install flac")?;

    if !status.success() {
        bail!("flac encoder exited with status: {status}");
    }

    Ok(())
}
