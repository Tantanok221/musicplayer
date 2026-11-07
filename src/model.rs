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

pub fn split_artist_names(value: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    value
        .split(['&', ',', ';', '＆', '，', '；'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter(|name| seen.insert(normalize(name)))
        .map(str::to_string)
        .collect()
}

pub fn artist_credits(label: &str, structured: &[NamedId], fallback_id: &str) -> Vec<NamedId> {
    let mut credits = Vec::<NamedId>::new();
    for artist in structured {
        let names = split_artist_names(&artist.name);
        let single = names.len() == 1;
        for name in names {
            if !credits
                .iter()
                .any(|credit| normalize(&credit.name) == normalize(&name))
            {
                credits.push(NamedId {
                    name,
                    id: if single {
                        artist.id.clone()
                    } else {
                        String::new()
                    },
                });
            }
        }
    }
    let names = split_artist_names(label);
    let single = names.len() == 1;
    for name in names {
        if !credits
            .iter()
            .any(|credit| normalize(&credit.name) == normalize(&name))
        {
            credits.push(NamedId {
                name,
                id: if single && structured.is_empty() {
                    fallback_id.into()
                } else {
                    String::new()
                },
            });
        }
    }
    credits
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
    pub fn artist_credits(&self) -> Vec<NamedId> {
        artist_credits(self.artist_label(), &self.artists, &self.artist_id)
    }
    pub fn album_artist_credits(&self) -> Vec<NamedId> {
        let fallback = if normalize(self.album_artist_label()) == normalize(self.artist_label()) {
            self.artist_id.as_str()
        } else {
            ""
        };
        artist_credits(self.album_artist_label(), &self.album_artists, fallback)
    }
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
    pub artists: Vec<NamedId>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseCategory {
    Album,
    Ep,
    Single,
    Other,
}
impl ReleaseCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Album => "Albums",
            Self::Ep => "EPs",
            Self::Single => "Singles",
            Self::Other => "Other releases",
        }
    }
}
impl Album {
    pub fn artist_credits(&self) -> Vec<NamedId> {
        artist_credits(
            if self.display_artist.is_empty() {
                &self.artist
            } else {
                &self.display_artist
            },
            &self.artists,
            &self.artist_id,
        )
    }
    /// Untagged releases count as albums; only an explicit other type (such as
    /// a compilation) lands in Other releases.
    pub fn category(&self) -> ReleaseCategory {
        if self.release_types.iter().all(|kind| kind.trim().is_empty()) {
            return ReleaseCategory::Album;
        }
        for (value, category) in [
            ("ep", ReleaseCategory::Ep),
            ("single", ReleaseCategory::Single),
            ("album", ReleaseCategory::Album),
        ] {
            if self
                .release_types
                .iter()
                .any(|kind| kind.eq_ignore_ascii_case(value))
            {
                return category;
            }
        }
        ReleaseCategory::Other
    }
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
    pub entry: Arc<Vec<Arc<Track>>>,
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct LibraryQuery {
    pub text: String,
    pub selection: crate::browser::BrowserSelection,
    pub artist: String,
    pub performer: String,
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
    Album,
    /// Album gain while the whole queue is one album, otherwise track gain.
    #[default]
    Smart,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
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
            gain: GainMode::Smart,
            shuffle: false,
            repeat: RepeatMode::Off,
            dark: None,
            layout: crate::browser::LayoutPreferences::default(),
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SavedQueue {
    #[serde(default)]
    pub listen: Option<crate::listening::ListenSession>,
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
    pub queue: Arc<Vec<Arc<Track>>>,
    pub error: Option<String>,
}

pub fn normalize(value: &str) -> String {
    value.nfkc().flat_map(char::to_lowercase).collect()
}

pub fn duration_label(seconds: f64) -> String {
    let seconds = seconds.max(0.) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
