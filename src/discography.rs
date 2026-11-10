//! An artist's published releases and which of them the library lacks.
//!
//! The release list comes from MusicBrainz and is cached; ownership is decided
//! locally against the catalog so a library refresh never needs the network.
use crate::model::{Album, ReleaseCategory};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};
use unicode_normalization::UnicodeNormalization;

pub const VARIOUS_ARTISTS_MBID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";
const FRESH_MS: i64 = 7 * 86_400_000;

/// One MusicBrainz release group: every edition of one album, EP or single.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReleaseGroup {
    pub id: String,
    pub title: String,
    pub primary: String,
    pub secondary: Vec<String>,
    /// First release date as MusicBrainz gives it: `YYYY`, `YYYY-MM`, `YYYY-MM-DD` or empty.
    pub date: String,
    pub official: bool,
    /// Titles of the individual releases, which include transliterations.
    pub titles: Vec<String>,
    /// Release IDs, the kind Navidrome reports as an album's MusicBrainz ID.
    pub releases: Vec<String>,
    pub listens: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Discography {
    /// Empty when the artist could not be identified on MusicBrainz.
    pub artist: String,
    pub groups: Vec<ReleaseGroup>,
    pub fetched_ms: i64,
}
impl Discography {
    pub fn fresh(&self, now_ms: i64) -> bool {
        self.fetched_ms > 0 && now_ms.saturating_sub(self.fetched_ms) < FRESH_MS
    }
}

/// A release the library lacks, ready for the artist page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MissingRelease {
    pub id: String,
    pub title: String,
    pub year: Option<i32>,
    pub kind: String,
    /// An official album or EP with no secondary type; shown by default.
    pub core: bool,
    pub upcoming: bool,
    pub cover: String,
}

#[derive(Clone, Debug, Default)]
pub struct DiscographyView {
    pub loading: bool,
    pub missing: Vec<MissingRelease>,
}

pub fn is_various_artists(name: &str, mbid: &str) -> bool {
    mbid == VARIOUS_ARTISTS_MBID
        || matches!(
            crate::model::normalize(name.trim()).as_str(),
            "various artists" | "various artist" | "various" | "v.a." | "va" | "v.a"
        )
}

const EDITION_WORDS: [&str; 18] = [
    "deluxe",
    "remaster",
    "remastered",
    "expanded",
    "edition",
    "version",
    "anniversary",
    "bonus",
    "special",
    "limited",
    "complete",
    "通常盤",
    "初回限定盤",
    "初回生産限定盤",
    "期間生産限定盤",
    "完全生産限定盤",
    "初回盤",
    "限定盤",
];

fn letters(value: &str) -> String {
    value.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// A title reduced to what identifies the release: width, case, Latin accents,
/// punctuation, bracketed notes and edition wording are all ignored.
pub fn fold(title: &str) -> String {
    // Only Latin accents are dropped; kana voicing marks distinguish titles.
    let plain: String = title
        .nfkd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .nfkc()
        .flat_map(char::to_lowercase)
        .collect();
    let mut kept = String::new();
    let mut depth = 0usize;
    for c in plain.chars() {
        match c {
            '(' | '[' | '【' | '〈' | '《' => depth += 1,
            ')' | ']' | '】' | '〉' | '》' => depth = depth.saturating_sub(1),
            _ if depth == 0 => kept.push(c),
            _ => {}
        }
    }
    // "Title - Deluxe Edition" style suffixes, and a bare trailing "EP".
    if let Some((head, tail)) = kept.rsplit_once(" - ")
        && !letters(head).is_empty()
        && EDITION_WORDS.iter().any(|word| tail.contains(word))
    {
        kept = head.to_string();
    }
    let mut words = kept.split_whitespace().collect::<Vec<_>>();
    while words.len() > 1
        && matches!(words.last(), Some(word) if matches!(letters(word).as_str(), "ep" | "single") || EDITION_WORDS.contains(&letters(word).as_str()))
    {
        words.pop();
    }
    let folded = letters(&words.join(" "));
    if !folded.is_empty() {
        return folded;
    }
    let whole = letters(&plain);
    if whole.is_empty() {
        plain.trim().to_string()
    } else {
        whole
    }
}

impl ReleaseGroup {
    fn folded_titles(&self) -> HashSet<String> {
        std::iter::once(&self.title)
            .chain(&self.titles)
            .map(|title| fold(title))
            .collect()
    }
    pub fn year(&self) -> Option<i32> {
        self.date.get(..4)?.parse().ok()
    }
    pub fn core(&self) -> bool {
        self.official
            && self.secondary.is_empty()
            && (self.primary.eq_ignore_ascii_case("album")
                || self.primary.eq_ignore_ascii_case("ep"))
    }
    pub fn category(&self) -> ReleaseCategory {
        if !self.secondary.is_empty() {
            return ReleaseCategory::Other;
        }
        match self.primary.to_ascii_lowercase().as_str() {
            "album" => ReleaseCategory::Album,
            "ep" => ReleaseCategory::Ep,
            "single" => ReleaseCategory::Single,
            _ => ReleaseCategory::Other,
        }
    }
    /// "Album", "EP", "Live album", "Remix single", "Broadcast"…
    pub fn kind(&self) -> String {
        let primary = if self.primary.is_empty() {
            "Release"
        } else {
            &self.primary
        };
        if self.secondary.is_empty() {
            return primary.to_string();
        }
        let secondary = self.secondary.join(" ");
        if primary.eq_ignore_ascii_case("ep") {
            format!("{secondary} EP")
        } else {
            format!("{secondary} {}", primary.to_lowercase())
        }
    }
    /// Dates without a day count as released once their month or year has begun.
    pub fn upcoming(&self, today: &str) -> bool {
        !self.date.is_empty() && self.date.as_str() > today
    }
}

/// What the catalog knows that decides ownership.
#[derive(Default)]
pub struct Library<'a> {
    /// Releases credited to the artist whose page is open.
    pub albums: &'a [Arc<Album>],
    /// MusicBrainz release IDs of every album in the library, whoever it is filed under.
    pub release_ids: HashSet<String>,
    /// Folded titles of the artist's tracks, to recognise singles owned on an album.
    pub track_titles: HashSet<String>,
    pub hidden: HashSet<String>,
    /// Release groups with an unfinished download. They stay listed even once
    /// a server scan has added the tracks that have arrived so far.
    pub downloading: HashSet<String>,
}

/// Official releases the library lacks, most listened first, then newest.
/// `today` is `YYYY-MM-DD`.
pub fn missing(discography: &Discography, library: &Library, today: &str) -> Vec<MissingRelease> {
    let album_titles: HashSet<String> = library
        .albums
        .iter()
        .map(|album| fold(&album.name))
        .collect();
    let titles: Vec<HashSet<String>> = discography
        .groups
        .iter()
        .map(ReleaseGroup::folded_titles)
        .collect();
    let owned: Vec<bool> = discography
        .groups
        .iter()
        .zip(&titles)
        .map(|(group, titles)| {
            library.release_ids.contains(&group.id)
                || group
                    .releases
                    .iter()
                    .any(|id| library.release_ids.contains(id))
                || titles.iter().any(|title| album_titles.contains(title))
        })
        .collect();
    // Regional editions are separate groups with one title; owning one covers the rest.
    let owned_titles: HashSet<&String> = titles
        .iter()
        .zip(&owned)
        .filter(|(_, owned)| **owned)
        .flat_map(|(titles, _)| titles)
        .collect();
    let mut seen = HashSet::new();
    let mut result: Vec<(&ReleaseGroup, MissingRelease)> = Vec::new();
    for ((group, titles), owned) in discography.groups.iter().zip(&titles).zip(&owned) {
        let settled = !library.downloading.contains(&group.id);
        if (settled
            && (*owned
                || !group.official
                || library.hidden.contains(&group.id)
                || titles.iter().any(|title| owned_titles.contains(title))
                || (group.category() == ReleaseCategory::Single
                    && titles
                        .iter()
                        .any(|title| library.track_titles.contains(title)))))
            // The same title twice among the missing is one tile, not two.
            || !seen.insert((fold(&group.title), group.category() as u8))
        {
            continue;
        }
        result.push((
            group,
            MissingRelease {
                id: group.id.clone(),
                title: group.title.clone(),
                year: group.year(),
                kind: group.kind(),
                core: group.core(),
                upcoming: group.upcoming(today),
                cover: format!(
                    "remote:https://coverartarchive.org/release-group/{}/front-250",
                    group.id
                ),
            },
        ));
    }
    result.sort_by(|(a, _), (b, _)| {
        b.listens
            .cmp(&a.listens)
            .then_with(|| b.date.cmp(&a.date))
            .then_with(|| a.title.cmp(&b.title))
    });
    result.into_iter().map(|(_, release)| release).collect()
}
