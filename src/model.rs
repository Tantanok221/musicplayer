use serde::{Deserialize, Serialize};
use std::sync::Arc;
use unicode_normalization::UnicodeNormalization;

pub type TrackId = String;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct NamedId {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Contributor {
    pub role: String,
    pub artist: NamedId,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct ReplayGain {
    pub track_gain: Option<f32>,
    pub album_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_peak: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Track {
    pub id: TrackId,
    pub title: String,
    pub album: String,
    pub album_id: String,
    pub artist: String,
    pub artist_id: String,
    pub album_artist: String,
    pub display_album_artist: String,
    pub display_artist: String,
    pub artists: Vec<NamedId>,
    pub album_artists: Vec<NamedId>,
    pub contributors: Vec<Contributor>,
    pub display_composer: String,
    pub genre: String,
    pub genres: Vec<Genre>,
    pub year: Option<i32>,
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub duration: f64,
    pub suffix: String,
    pub content_type: String,
    pub bit_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub sampling_rate: Option<u32>,
    pub channel_count: Option<u32>,
    pub size: Option<u64>,
    pub path: String,
    pub cover_art: String,
    pub starred: Option<String>,
    pub user_rating: Option<u8>,
    pub play_count: Option<u64>,
    pub music_brainz_id: String,
    pub comment: String,
    pub replay_gain: Option<ReplayGain>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Genre {
    pub name: String,
}

impl Track {
    pub fn artist_label(&self) -> &str {
        if self.display_artist.is_empty() {
            &self.artist
        } else {
            &self.display_artist
        }
    }
    pub fn album_artist_label(&self) -> &str {
        if !self.display_album_artist.is_empty() {
            &self.display_album_artist
        } else if !self.album_artist.is_empty() {
            &self.album_artist
        } else if let Some(artist) = self.album_artists.first() {
            &artist.name
        } else {
            &self.artist
        }
    }
    pub fn genres_label(&self) -> String {
        if self.genres.is_empty() {
            self.genre.clone()
        } else {
            self.genres
                .iter()
                .map(|g| g.name.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub artist_id: String,
    pub display_artist: String,
    pub cover_art: String,
    pub year: Option<i32>,
    pub genre: String,
    pub song_count: u32,
    pub duration: f64,
    pub starred: Option<String>,
    pub play_count: Option<u64>,
    pub user_rating: Option<u8>,
    pub record_labels: Vec<NamedId>,
    pub release_types: Vec<String>,
    pub original_release_date: Option<serde_json::Value>,
    pub release_date: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub owner: String,
    pub song_count: u32,
    pub duration: f64,
    pub cover_art: String,
    pub entry: Vec<Track>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum Section {
    #[default]
    Albums,
    Artists,
    Tracks,
    Favorites,
    Playlists,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Album,
    Artist,
    Title,
    Year,
    Added,
    Duration,
    Rating,
    Plays,
    Format,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LibraryQuery {
    pub text: String,
    pub selection: crate::browser::BrowserSelection,
    pub artist: String,
    pub genre: String,
    pub year: Option<i32>,
    pub favorites: bool,
    pub sort: Sort,
    pub descending: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum GainMode {
    Off,
    Track,
    #[default]
    Album,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Preferences {
    pub volume: f32,
    pub gain: GainMode,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub dark: Option<bool>,
    pub layout: crate::browser::LayoutPreferences,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            volume: 0.7,
            gain: GainMode::Album,
            shuffle: false,
            repeat: RepeatMode::Off,
            dark: None,
            layout: crate::browser::LayoutPreferences::default(),
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SavedQueue {
    pub ids: Vec<TrackId>,
    pub index: Option<usize>,
    pub position: f64,
}

#[derive(Clone, Debug, Default)]
pub struct PlaybackSnapshot {
    pub track: Option<Arc<Track>>,
    pub playing: bool,
    pub buffering: bool,
    pub position: f64,
    pub duration: f64,
    pub index: Option<usize>,
    pub queue: Vec<Arc<Track>>,
    pub error: Option<String>,
}

pub fn normalize(value: &str) -> String {
    value.nfkc().flat_map(char::to_lowercase).collect()
}

pub fn duration_label(seconds: f64) -> String {
    let seconds = seconds.max(0.) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
