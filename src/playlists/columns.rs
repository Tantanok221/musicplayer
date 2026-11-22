//! Saved playlist metadata choices, independent of the UI and playlist ownership.
use crate::model::{Track, duration_label};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistColumn {
    Artist,
    Album,
    AlbumArtist,
    Genre,
    Year,
    Track,
    Disc,
    Duration,
    Format,
    Bitrate,
    SampleRate,
    BitDepth,
    PlayCount,
}

impl PlaylistColumn {
    pub const ALL: [Self; 13] = [
        Self::Artist,
        Self::Album,
        Self::AlbumArtist,
        Self::Genre,
        Self::Year,
        Self::Track,
        Self::Disc,
        Self::Duration,
        Self::Format,
        Self::Bitrate,
        Self::SampleRate,
        Self::BitDepth,
        Self::PlayCount,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Album => "album",
            Self::AlbumArtist => "album-artist",
            Self::Genre => "genre",
            Self::Year => "year",
            Self::Track => "track",
            Self::Disc => "disc",
            Self::Duration => "duration",
            Self::Format => "format",
            Self::Bitrate => "bitrate",
            Self::SampleRate => "sample-rate",
            Self::BitDepth => "bit-depth",
            Self::PlayCount => "play-count",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::AlbumArtist => "Album artist",
            Self::Genre => "Genre",
            Self::Year => "Year",
            Self::Track => "Track number",
            Self::Disc => "Disc number",
            Self::Duration => "Duration",
            Self::Format => "Format",
            Self::Bitrate => "Bitrate",
            Self::SampleRate => "Sample rate",
            Self::BitDepth => "Bit depth",
            Self::PlayCount => "Play count",
        }
    }

    pub fn width(self) -> f32 {
        match self {
            Self::Artist | Self::AlbumArtist => 130.,
            Self::Album => 160.,
            Self::Genre => 110.,
            Self::Year | Self::Format => 55.,
            _ => 80.,
        }
    }

    pub fn value(self, track: &Track) -> String {
        let value = match self {
            Self::Artist => track.artist_label().to_owned(),
            Self::Album => track.album.clone(),
            Self::AlbumArtist => track.album_artist_label().to_owned(),
            Self::Genre => track.genres_label(),
            Self::Year => track
                .year
                .filter(|n| *n > 0)
                .map(|n| n.to_string())
                .unwrap_or_default(),
            Self::Track => track
                .track
                .filter(|n| *n > 0)
                .map(|n| n.to_string())
                .unwrap_or_default(),
            Self::Disc => track
                .disc_number
                .filter(|n| *n > 0)
                .map(|n| n.to_string())
                .unwrap_or_default(),
            Self::Duration => duration_label(track.duration),
            Self::Format => track.suffix.to_uppercase(),
            Self::Bitrate => track
                .bit_rate
                .filter(|n| *n > 0)
                .map(|n| format!("{n} kbps"))
                .unwrap_or_default(),
            Self::SampleRate => track
                .sampling_rate
                .filter(|n| *n > 0)
                .map(|n| format!("{} kHz", n as f64 / 1000.))
                .unwrap_or_default(),
            Self::BitDepth => track
                .bit_depth
                .filter(|n| *n > 0)
                .map(|n| format!("{n}-bit"))
                .unwrap_or_default(),
            Self::PlayCount => track.play_count.map(|n| n.to_string()).unwrap_or_default(),
        };
        if value.trim().is_empty() {
            "—".into()
        } else {
            value
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct PlaylistColumns {
    visible: Vec<PlaylistColumn>,
}

impl Default for PlaylistColumns {
    fn default() -> Self {
        Self {
            visible: vec![
                PlaylistColumn::Artist,
                PlaylistColumn::Album,
                PlaylistColumn::Duration,
            ],
        }
    }
}

impl PlaylistColumns {
    pub fn contains(&self, column: PlaylistColumn) -> bool {
        self.visible.contains(&column)
    }

    pub fn set(&mut self, column: PlaylistColumn, visible: bool) {
        self.visible.retain(|item| *item != column);
        if visible {
            self.visible.push(column);
        }
    }

    /// A fixed display order prevents selection order or saved duplicates moving columns.
    pub fn iter(&self) -> impl Iterator<Item = PlaylistColumn> + '_ {
        PlaylistColumn::ALL
            .into_iter()
            .filter(|column| self.contains(*column))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_round_trip_and_old_preferences_get_defaults() {
        let old: crate::model::Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(old.playlist_columns, PlaylistColumns::default());
        let mut columns = PlaylistColumns::default();
        for column in PlaylistColumn::ALL {
            columns.set(column, false);
        }
        assert_eq!(
            columns.iter().count(),
            0,
            "title is always rendered separately"
        );
        columns.set(PlaylistColumn::Genre, true);
        columns.set(PlaylistColumn::Artist, true);
        columns.set(PlaylistColumn::Genre, true);
        assert_eq!(
            columns.iter().collect::<Vec<_>>(),
            [PlaylistColumn::Artist, PlaylistColumn::Genre]
        );
        let restored: PlaylistColumns =
            serde_json::from_str(&serde_json::to_string(&columns).unwrap()).unwrap();
        assert_eq!(restored, columns);
    }

    #[test]
    fn values_use_display_metadata_and_distinguish_missing_from_zero() {
        let track = Track {
            artist: "Artist".into(),
            display_artist: "Display Artist".into(),
            genre: "Jazz".into(),
            bit_rate: Some(320),
            sampling_rate: Some(44100),
            bit_depth: Some(24),
            suffix: "flac".into(),
            play_count: Some(0),
            ..Default::default()
        };
        assert_eq!(PlaylistColumn::Artist.value(&track), "Display Artist");
        assert_eq!(PlaylistColumn::AlbumArtist.value(&track), "Artist");
        assert_eq!(PlaylistColumn::Genre.value(&track), "Jazz");
        assert_eq!(PlaylistColumn::Album.value(&track), "—");
        assert_eq!(PlaylistColumn::Year.value(&track), "—");
        assert_eq!(PlaylistColumn::Bitrate.value(&track), "320 kbps");
        assert_eq!(PlaylistColumn::SampleRate.value(&track), "44.1 kHz");
        assert_eq!(PlaylistColumn::BitDepth.value(&track), "24-bit");
        assert_eq!(PlaylistColumn::Format.value(&track), "FLAC");
        assert_eq!(PlaylistColumn::PlayCount.value(&track), "0");
    }
}
