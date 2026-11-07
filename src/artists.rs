//! Cached artist enrichment supplied by the connected Navidrome server.
use crate::model::{Album, Track};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
#[derive(Clone, Debug, Default)]
pub struct ArtistPageData {
    pub albums: Vec<Arc<Album>>,
    pub most_played: Vec<Arc<Track>>,
    pub genres: Vec<String>,
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
