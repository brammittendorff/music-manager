//! ID3 tagging for MP3 files using the `id3` crate.

use anyhow::{Context, Result};
use id3::{Tag, TagLike, Version};
use std::path::Path;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct TagInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<i32>,
    pub track_number: u32,
    pub total_tracks: Option<u32>,
    pub musicbrainz_track_id: Option<String>,
    pub musicbrainz_release_id: Option<String>,
}

/// Write ID3v2.4 tags to an MP3 file.
pub fn tag_mp3(mp3_path: &Path, info: &TagInfo) -> Result<()> {
    debug!("Tagging {}: {} – {}", mp3_path.display(), info.artist, info.title);

    let mut tag = Tag::new();

    tag.set_title(&info.title);
    tag.set_artist(&info.artist);
    tag.set_album(&info.album);

    if let Some(year) = info.year {
        tag.set_year(year);
    }

    tag.set_track(info.track_number);

    if let Some(total) = info.total_tracks {
        // ID3 stores track as "n/total"
        tag.set_total_tracks(total);
    }

    // MusicBrainz IDs stored as TXXX frames - standard convention
    if let Some(ref mb_id) = info.musicbrainz_track_id {
        tag.add_frame(id3::frame::ExtendedText {
            description: "MusicBrainz Track Id".to_owned(),
            value: mb_id.clone(),
        });
    }

    if let Some(ref mb_id) = info.musicbrainz_release_id {
        tag.add_frame(id3::frame::ExtendedText {
            description: "MusicBrainz Album Id".to_owned(),
            value: mb_id.clone(),
        });
    }

    // Tag written with: music-manager
    tag.add_frame(id3::frame::ExtendedText {
        description: "Encoded by".to_owned(),
        value: "music-manager / LAME 320kbps CBR".to_owned(),
    });

    tag.write_to_path(mp3_path, Version::Id3v24)
        .with_context(|| format!("Write ID3 tags to {}", mp3_path.display()))?;

    Ok(())
}

/// Read existing tags from an MP3 file.
pub fn read_tags(mp3_path: &Path) -> Result<Option<TagInfo>> {
    let tag = match Tag::read_from_path(mp3_path) {
        Ok(t) => t,
        Err(id3::Error { kind: id3::ErrorKind::NoTag, .. }) => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let title = tag.title().unwrap_or("").to_owned();
    let artist = tag.artist().unwrap_or("").to_owned();
    let album = tag.album().unwrap_or("").to_owned();
    let year = tag.year();
    let track_number = tag.track().unwrap_or(0);
    let total_tracks = tag.total_tracks();
    let musicbrainz_track_id = tag
        .extended_texts()
        .find(|t| t.description == "MusicBrainz Track Id")
        .map(|t| t.value.clone());
    let musicbrainz_release_id = tag
        .extended_texts()
        .find(|t| t.description == "MusicBrainz Album Id")
        .map(|t| t.value.clone());

    Ok(Some(TagInfo {
        title,
        artist,
        album,
        year,
        track_number,
        total_tracks,
        musicbrainz_track_id,
        musicbrainz_release_id,
    }))
}
