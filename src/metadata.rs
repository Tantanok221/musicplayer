//! Read-only metadata inspection: the Properties view of a track and library
//! health checks for gaps in the tags Navidrome reports.
use crate::model::Track;
use serde::{Deserialize, Serialize};

const MISSING: &str = "—";

/// A tag gap the library can be narrowed to, in the spirit of MusicBee's
/// "missing" auto-playlists.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum HealthIssue {
    MissingYear,
    MissingGenre,
    MissingTrackNumber,
    MissingMusicBrainzId,
    MixedAlbumArtist,
}

impl HealthIssue {
    pub const ALL: [Self; 5] = [
        Self::MissingYear,
        Self::MissingGenre,
        Self::MissingTrackNumber,
        Self::MissingMusicBrainzId,
        Self::MixedAlbumArtist,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::MissingYear => "Missing year",
            Self::MissingGenre => "Missing genre",
            Self::MissingTrackNumber => "Missing track number",
            Self::MissingMusicBrainzId => "Missing MusicBrainz ID",
            Self::MixedAlbumArtist => "Mixed album artist",
        }
    }

    /// A condition on the `tracks` table that holds for affected tracks.
    pub fn sql(self) -> &'static str {
        match self {
            Self::MissingYear => "coalesce(tracks.year,0)<=0",
            Self::MissingGenre => "trim(tracks.genre)=''",
            Self::MissingTrackNumber => "coalesce(tracks.track,0)<=0",
            Self::MissingMusicBrainzId => {
                "trim(coalesce(json_extract(tracks.data,'$.musicBrainzId'),''))=''"
            }
            Self::MixedAlbumArtist => {
                "tracks.album_id IN (SELECT album_id FROM tracks WHERE album_id!='' GROUP BY album_id HAVING count(DISTINCT album_artist)>1)"
            }
        }
    }
}

/// One row of the Properties view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub key: &'static str,
    pub label: &'static str,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertySection {
    pub title: &'static str,
    pub rows: Vec<Property>,
}

/// Everything Navidrome reports about a track's file and tags, grouped like
/// MusicBee's Properties tab. Missing values show as a dash so gaps stand out.
pub fn properties(track: &Track) -> Vec<PropertySection> {
    let text = |value: &str| {
        if value.trim().is_empty() {
            MISSING.to_string()
        } else {
            value.to_string()
        }
    };
    let number = |value: Option<u32>| {
        value
            .filter(|v| *v > 0)
            .map_or(MISSING.into(), |v| v.to_string())
    };
    let row = |key, label, value| Property { key, label, value };
    let gain = |value: Option<f32>| value.map_or(MISSING.into(), |v| format!("{v:+.2} dB"));
    let peak = |value: Option<f32>| value.map_or(MISSING.into(), |v| format!("{v:.6}"));
    let replay = track.replay_gain.clone().unwrap_or_default();
    let composer = if track.display_composer.is_empty() {
        track
            .contributors
            .iter()
            .filter(|c| c.role.eq_ignore_ascii_case("composer"))
            .map(|c| c.artist.name.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    } else {
        track.display_composer.clone()
    };
    vec![
        PropertySection {
            title: "Tags",
            rows: vec![
                row("title", "Title", text(&track.title)),
                row("sort-title", "Sort title", text(&track.sort_name)),
                row("artist", "Artist", text(track.artist_label())),
                row(
                    "album-artist",
                    "Album artist",
                    text(track.album_artist_label()),
                ),
                row("album", "Album", text(&track.album)),
                row("composer", "Composer", text(&composer)),
                row("genre", "Genre", text(&track.genres_label())),
                row(
                    "year",
                    "Year",
                    track
                        .year
                        .filter(|y| *y > 0)
                        .map_or(MISSING.into(), |y| y.to_string()),
                ),
                row(
                    "release-date",
                    "Release date",
                    date_label(track.release_date.as_ref()),
                ),
                row(
                    "original-date",
                    "Original date",
                    date_label(track.original_release_date.as_ref()),
                ),
                row("track-number", "Track", number(track.track)),
                row("disc-number", "Disc", number(track.disc_number)),
                row("comment", "Comment", text(&track.comment)),
            ],
        },
        PropertySection {
            title: "File",
            rows: vec![
                row("path", "Path", text(&track.path)),
                row("format", "Format", text(&track.suffix.to_uppercase())),
                row("content-type", "Content type", text(&track.content_type)),
                row("duration", "Duration", duration(track.duration)),
                row(
                    "bit-rate",
                    "Bit rate",
                    track
                        .bit_rate
                        .map_or(MISSING.into(), |v| format!("{v} kbps")),
                ),
                row(
                    "sample-rate",
                    "Sample rate",
                    track.sampling_rate.map_or(MISSING.into(), |v| {
                        format!("{:.1} kHz", f64::from(v) / 1000.)
                    }),
                ),
                row(
                    "bit-depth",
                    "Bit depth",
                    track
                        .bit_depth
                        .map_or(MISSING.into(), |v| format!("{v}-bit")),
                ),
                row(
                    "channels",
                    "Channels",
                    track.channel_count.map_or(MISSING.into(), channels),
                ),
                row("size", "Size", track.size.map_or(MISSING.into(), size)),
                row(
                    "added",
                    "Added",
                    text(track.created.as_deref().unwrap_or_default()),
                ),
            ],
        },
        PropertySection {
            title: "ReplayGain",
            rows: vec![
                row("track-gain", "Track gain", gain(replay.track_gain)),
                row("track-peak", "Track peak", peak(replay.track_peak)),
                row("album-gain", "Album gain", gain(replay.album_gain)),
                row("album-peak", "Album peak", peak(replay.album_peak)),
            ],
        },
        PropertySection {
            title: "Identifiers",
            rows: vec![
                row(
                    "musicbrainz-id",
                    "MusicBrainz",
                    text(&track.music_brainz_id),
                ),
                row("track-id", "Track ID", text(&track.id)),
                row("album-id", "Album ID", text(&track.album_id)),
                row("cover-art-id", "Cover art ID", text(&track.cover_art)),
            ],
        },
    ]
}

/// Navidrome sends dates as `YYYY-MM-DD` text or as `{year, month, day}`.
fn date_label(value: Option<&serde_json::Value>) -> String {
    let Some(value) = value else {
        return MISSING.into();
    };
    if let Some(text) = value.as_str() {
        return if text.trim().is_empty() {
            MISSING.into()
        } else {
            text.trim().into()
        };
    }
    let part = |name| value[name].as_i64().filter(|v| *v > 0);
    match (part("year"), part("month"), part("day")) {
        (Some(y), Some(m), Some(d)) => format!("{y:04}-{m:02}-{d:02}"),
        (Some(y), Some(m), None) => format!("{y:04}-{m:02}"),
        (Some(y), ..) => format!("{y:04}"),
        _ => MISSING.into(),
    }
}

fn duration(seconds: f64) -> String {
    if seconds <= 0. {
        return MISSING.into();
    }
    let total = seconds.round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

fn channels(count: u32) -> String {
    match count {
        1 => "Mono".into(),
        2 => "Stereo".into(),
        n => format!("{n} channels"),
    }
}

fn size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ReplayGain;

    fn value(sections: &[PropertySection], key: &str) -> String {
        sections
            .iter()
            .flat_map(|s| &s.rows)
            .find(|row| row.key == key)
            .unwrap_or_else(|| panic!("no {key} property"))
            .value
            .clone()
    }

    #[test]
    fn properties_format_file_details_and_mark_gaps() {
        let track = Track {
            id: "t1".into(),
            title: "Song".into(),
            path: "Artist/Album/01 Song.flac".into(),
            suffix: "flac".into(),
            bit_rate: Some(1411),
            sampling_rate: Some(44_100),
            bit_depth: Some(16),
            channel_count: Some(2),
            size: Some(3 * 1024 * 1024 + 512 * 1024),
            duration: 185.4,
            track: Some(0),
            original_release_date: Some(serde_json::json!({"year": 1999, "month": 3})),
            release_date: Some(serde_json::json!("2004-05-06")),
            replay_gain: Some(ReplayGain {
                track_gain: Some(-7.5),
                ..Default::default()
            }),
            ..Default::default()
        };
        let sections = properties(&track);
        assert_eq!(
            sections.iter().map(|s| s.title).collect::<Vec<_>>(),
            ["Tags", "File", "ReplayGain", "Identifiers"]
        );
        assert_eq!(value(&sections, "path"), "Artist/Album/01 Song.flac");
        assert_eq!(value(&sections, "format"), "FLAC");
        assert_eq!(value(&sections, "bit-rate"), "1411 kbps");
        assert_eq!(value(&sections, "sample-rate"), "44.1 kHz");
        assert_eq!(value(&sections, "bit-depth"), "16-bit");
        assert_eq!(value(&sections, "channels"), "Stereo");
        assert_eq!(value(&sections, "size"), "3.5 MB");
        assert_eq!(value(&sections, "duration"), "3:05");
        assert_eq!(value(&sections, "original-date"), "1999-03");
        assert_eq!(value(&sections, "release-date"), "2004-05-06");
        assert_eq!(value(&sections, "track-gain"), "-7.50 dB");
        assert_eq!(value(&sections, "track-peak"), MISSING);
        assert_eq!(value(&sections, "track-number"), MISSING);
        assert_eq!(value(&sections, "year"), MISSING);
        assert_eq!(value(&sections, "genre"), MISSING);
        assert_eq!(value(&sections, "musicbrainz-id"), MISSING);
    }
}
