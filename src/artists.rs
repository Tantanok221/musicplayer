//! Cached artist enrichment supplied by the connected Navidrome server.
use crate::model::{Album, ReleaseCategory, Track, normalize};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistProfile {
    pub name: String,
    pub id: String,
    pub portrait: String,
    pub biography: String,
    pub url: String,
    pub similar: Vec<SimilarArtist>,
    pub popular: Vec<Arc<Track>>,
    pub fetched_ms: i64,
    pub complete: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SimilarArtist {
    pub name: String,
    pub id: String,
    pub portrait: String,
    pub in_library: bool,
}
/// Navidrome's ranked top songs for one artist, kept apart from profiles so album
/// selection never pays for profile hydration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TopSongs {
    pub songs: Vec<TopSong>,
    pub fetched_ms: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TopSong {
    pub id: String,
    pub album_id: String,
}
impl TopSongs {
    pub fn from_tracks(tracks: &[Arc<Track>], fetched_ms: i64) -> Self {
        Self {
            songs: tracks
                .iter()
                .map(|track| TopSong {
                    id: track.id.clone(),
                    album_id: track.album_id.clone(),
                })
                .collect(),
            fetched_ms,
        }
    }
    pub fn fresh(&self, now_ms: i64) -> bool {
        self.fetched_ms > 0 && now_ms.saturating_sub(self.fetched_ms) < 86_400_000
    }
    /// Track IDs from `album_id`, best-ranked first, at most `limit`.
    pub fn for_album(&self, album_id: &str, limit: usize) -> Vec<String> {
        self.songs
            .iter()
            .filter(|song| song.album_id == album_id)
            .map(|song| song.id.clone())
            .take(limit)
            .collect()
    }
}
#[derive(Clone, Debug, Default)]
pub struct ArtistPageData {
    pub albums: Vec<Arc<Album>>,
    pub tracks: Vec<Arc<Track>>,
    pub most_played: Vec<Arc<Track>>,
    pub genres: Vec<String>,
}
/// One release kept by an artist-page search, with the songs whose titles matched.
pub struct ReleaseHit {
    pub album: Arc<Album>,
    pub songs: Vec<Arc<Track>>,
}
/// Albums and songs on this artist's releases, grouped into the page's sections.
/// An album is included when its name matches or it contains a matching song.
pub fn search_releases(
    albums: &[Arc<Album>],
    tracks: &[Arc<Track>],
    query: &str,
) -> Vec<(ReleaseCategory, Vec<ReleaseHit>)> {
    let query = normalize(query.trim());
    if query.is_empty() {
        return Vec::new();
    }
    let mut songs_by_album: HashMap<&str, Vec<Arc<Track>>> = HashMap::new();
    for track in tracks {
        if normalize(&track.title).contains(&query) {
            songs_by_album
                .entry(track.album_id.as_str())
                .or_default()
                .push(track.clone());
        }
    }
    let mut sections = Vec::new();
    for category in [
        ReleaseCategory::Album,
        ReleaseCategory::Ep,
        ReleaseCategory::Single,
        ReleaseCategory::Other,
    ] {
        let mut hits = Vec::new();
        for album in albums.iter().filter(|album| album.category() == category) {
            let songs = songs_by_album.remove(album.id.as_str()).unwrap_or_default();
            if normalize(&album.name).contains(&query) || !songs.is_empty() {
                hits.push(ReleaseHit {
                    album: album.clone(),
                    songs,
                });
            }
        }
        if !hits.is_empty() {
            sections.push((category, hits));
        }
    }
    sections
}
pub fn lastfm_url(name: &str) -> String {
    let mut url = url::Url::parse("https://www.last.fm/music/").unwrap();
    url.path_segments_mut().unwrap().push(name);
    url.to_string().replace('+', "%2B")
}
pub fn profile_url(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    (matches!(url.scheme(), "https" | "http")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none())
    .then(|| url.to_string())
}
pub fn plain_biography(html: &str) -> String {
    let mut text = String::new();
    let mut remaining = html;
    while let Some(start) = remaining.find('<') {
        text.push_str(&remaining[..start]);
        let Some(end) = remaining[start..].find('>') else {
            remaining = "";
            break;
        };
        let tag = &remaining[start + 1..start + end];
        let lower = tag.to_ascii_lowercase();
        remaining = &remaining[start + end + 1..];
        let name = lower.split_whitespace().next().unwrap_or("");
        if matches!(name, "script" | "style") {
            let closing = format!("</{name}>");
            let tail = remaining.to_ascii_lowercase();
            remaining = tail
                .find(&closing)
                .map_or("", |at| &remaining[at + closing.len()..]);
        } else if matches!(name.trim_matches('/'), "p" | "br" | "div" | "li") {
            text.push(' ');
        }
    }
    text.push_str(remaining);
    html_escape::decode_html_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn cache_key(name: &str, id: &str) -> String {
    format!("{}\n{id}", crate::model::normalize(name))
}

impl ArtistProfile {
    pub fn fresh(&self, full: bool, now_ms: i64) -> bool {
        self.fetched_ms > 0
            && now_ms.saturating_sub(self.fetched_ms)
                < if self.complete { 86_400_000 } else { 3_600_000 }
            && (!full || self.complete || self.id.is_empty())
    }
}
