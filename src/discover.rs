//! Shelves of owned music that has fallen out of rotation.
use crate::model::Album;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub const WINDOW_DAYS: i64 = 182;
pub const SHELF_LIMIT: usize = 48;
pub const MIN_ROTATION_PLAYS: u64 = 5;
pub const NOT_NOW_DAYS: i64 = 30;
pub const LOVED_SHOWN: usize = 5;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub plays: u64,
}

/// An album with its loved songs (favorited or rated four stars or more) and
/// its most played song.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub album: Arc<Album>,
    pub loved: Vec<Song>,
    pub top_song: Option<Song>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    Rotation {
        plays: u64,
        played: DateTime<Utc>,
    },
    Favorite {
        loved_songs: usize,
        played: Option<DateTime<Utc>>,
    },
}

#[derive(Clone, Debug)]
pub struct Pick {
    pub album: Arc<Album>,
    pub reason: Reason,
    pub loved: Vec<Song>,
    pub top_song: Option<Song>,
}

/// One shelf: every pick by rank, and the same picks in today's rotation.
#[derive(Clone, Debug, Default)]
pub struct Shelf {
    pub ranked: Vec<Pick>,
    pub today: Vec<Pick>,
}

impl Shelf {
    pub fn is_empty(&self) -> bool {
        self.ranked.is_empty()
    }
}

/// Old rotation albums last played in the same season.
#[derive(Clone, Debug, Default)]
pub struct Season {
    pub key: String,
    pub label: String,
    pub shelf: Shelf,
}

#[derive(Clone, Debug, Default)]
pub struct Rediscover {
    pub old_rotation: Shelf,
    pub forgotten_favorites: Shelf,
    pub lane: Vec<Season>,
    pub spotlight: Vec<Pick>,
}

/// What the listener has opened on the page: expanded shelves and which of
/// today's spotlight picks is showing.
#[derive(Clone, Debug, Default)]
pub struct Visit {
    expanded: HashSet<String>,
    spotlight: usize,
}

impl Visit {
    pub fn toggle(&mut self, key: &str) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.into());
        }
    }

    pub fn expanded(&self, key: &str) -> bool {
        self.expanded.contains(key)
    }

    pub fn another(&mut self) {
        self.spotlight = self.spotlight.wrapping_add(1);
    }

    pub fn spotlight<'a>(&self, picks: &'a [Pick]) -> Option<&'a Pick> {
        (!picks.is_empty()).then(|| &picks[self.spotlight % picks.len()])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dismissal {
    NotNow,
    Never,
}

/// Albums the listener dismissed, with when each may return; `None` is never.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Hidden {
    until: HashMap<String, Option<i64>>,
}

impl Hidden {
    pub fn hide(&mut self, id: &str, dismissal: Dismissal, now: DateTime<Utc>) {
        let now_ms = now.timestamp_millis();
        self.until
            .retain(|_, until| until.is_none_or(|at| at > now_ms));
        let until = match dismissal {
            Dismissal::NotNow => Some(now_ms + NOT_NOW_DAYS * 86_400_000),
            Dismissal::Never => None,
        };
        self.until.insert(id.into(), until);
    }

    pub fn hides(&self, id: &str, now: DateTime<Utc>) -> bool {
        self.until
            .get(id)
            .is_some_and(|until| until.is_none_or(|at| at > now.timestamp_millis()))
    }
}

fn last_played(played: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(at) = played else {
        return "never played".into();
    };
    let days = (now - at).num_days();
    if days >= 365 {
        return "last played over a year ago".into();
    }
    let months = (days / 30).clamp(1, 11);
    format!(
        "last played {months} month{} ago",
        if months == 1 { "" } else { "s" }
    )
}

impl Pick {
    /// The album's story in one sentence.
    pub fn memory(&self) -> String {
        match &self.reason {
            Reason::Rotation { plays, played } => format!(
                "You played this {plays} times. The last time was in {}.",
                month(*played)
            ),
            Reason::Favorite { played, .. } => {
                let rating = self.album.user_rating.filter(|rating| *rating >= 4);
                match (self.album.starred.is_some(), rating, played) {
                    (true, _, None) => {
                        "You saved this as a favorite but never pressed play.".into()
                    }
                    (false, Some(rating), None) => {
                        format!("You rated this {rating} stars but never pressed play.")
                    }
                    (true, _, Some(at)) => format!(
                        "One of your favorite albums. You last played it in {}.",
                        month(*at)
                    ),
                    (false, Some(rating), Some(at)) => format!(
                        "You rated this {rating} stars. You last played it in {}.",
                        month(*at)
                    ),
                    (false, None, _) if self.loved.len() == 1 => {
                        "You loved a song on this album, then let it rest.".into()
                    }
                    (false, None, _) => format!(
                        "You loved {} songs on this album, then let it rest.",
                        self.loved.len()
                    ),
                }
            }
        }
    }

    /// A second line: the song that kept you coming back, or the ones you loved.
    pub fn detail(&self) -> String {
        if let (Reason::Rotation { .. }, Some(song)) = (&self.reason, &self.top_song)
            && song.plays >= 2
        {
            return format!(
                "The song you came back to most was \u{201c}{}\u{201d}, {} times.",
                song.title, song.plays
            );
        }
        if !self.loved.is_empty() {
            let mut titles = self
                .loved
                .iter()
                .take(LOVED_SHOWN)
                .map(|song| song.title.as_str())
                .collect::<Vec<_>>()
                .join(" \u{b7} ");
            if self.loved.len() > LOVED_SHOWN {
                titles.push_str(&format!(" and {} more", self.loved.len() - LOVED_SHOWN));
            }
            return format!("Songs you loved: {titles}");
        }
        let added = self
            .album
            .created
            .as_deref()
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok());
        match added {
            Some(at) => format!("In your library since {}.", month(at.with_timezone(&Utc))),
            None => "Still in your library, right where you left it.".into(),
        }
    }

    /// Why the album is on its shelf, in a few words.
    pub fn reason_text(&self, now: DateTime<Utc>) -> String {
        match &self.reason {
            Reason::Rotation { plays, played } => {
                format!("{plays} plays · {}", last_played(Some(*played), now))
            }
            Reason::Favorite {
                loved_songs,
                played,
            } => {
                let mut parts = Vec::new();
                if self.album.starred.is_some() {
                    parts.push("Favorite album".to_string());
                } else if let Some(rating) = self.album.user_rating.filter(|rating| *rating >= 4) {
                    parts.push(format!("Rated {rating} stars"));
                }
                if *loved_songs > 0 {
                    parts.push(format!(
                        "{loved_songs} loved song{}",
                        if *loved_songs == 1 { "" } else { "s" }
                    ));
                }
                parts.push(last_played(*played, now));
                parts.join(" · ")
            }
        }
    }
}

fn month(at: DateTime<Utc>) -> String {
    at.format("%B %Y").to_string()
}

/// Groups old rotation by the season each album was last played, newest first;
/// anything from a year ago or more shares one season.
fn lane(ranked: &[Pick], now: DateTime<Utc>) -> Vec<Season> {
    let mut seasons: Vec<(i64, Season)> = Vec::new();
    for pick in ranked {
        let Reason::Rotation { played, .. } = pick.reason else {
            continue;
        };
        let (order, key, label) = if (now - played).num_days() >= 365 {
            (
                i64::MIN,
                "older".to_string(),
                "A year ago and longer".to_string(),
            )
        } else {
            let (month, year) = (played.month() as i64, i64::from(played.year()));
            let year = if month <= 2 { year - 1 } else { year };
            let (index, name) = match month {
                3..=5 => (1, "spring"),
                6..=8 => (2, "summer"),
                9..=11 => (3, "autumn"),
                _ => (0, "winter"),
            };
            let label = if index == 0 {
                format!("Winter {year}\u{2013}{:02}", (year + 1) % 100)
            } else {
                let mut title = name.to_string();
                title[..1].make_ascii_uppercase();
                format!("{title} {year}")
            };
            (year * 4 + index, format!("{year}-{name}"), label)
        };
        match seasons.iter_mut().find(|(_, season)| season.key == key) {
            Some((_, season)) => season.shelf.ranked.push(pick.clone()),
            None => seasons.push((
                order,
                Season {
                    key,
                    label,
                    shelf: Shelf {
                        ranked: vec![pick.clone()],
                        today: Vec::new(),
                    },
                },
            )),
        }
    }
    seasons.sort_by_key(|(order, _)| std::cmp::Reverse(*order));
    seasons
        .into_iter()
        .map(|(_, mut season)| {
            season.shelf.today = todays_order(&season.shelf.ranked, now.date_naive(), &season.key);
            season
        })
        .collect()
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The shelf in the day's order: the same all day, different the next.
pub fn todays_order(picks: &[Pick], day: NaiveDate, shelf: &str) -> Vec<Pick> {
    let mut order = picks.to_vec();
    order.sort_by_cached_key(|pick| fnv(&format!("{day}/{shelf}/{}", pick.album.id)));
    order
}

fn played(album: &Album) -> Option<DateTime<Utc>> {
    let played = album.played.as_deref()?;
    DateTime::parse_from_rfc3339(played)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn plays(album: &Album) -> u64 {
    album.play_count.unwrap_or(0)
}

fn favorite(album: &Album) -> bool {
    album.starred.is_some() || album.user_rating.unwrap_or(0) >= 4
}

fn shelf(ranked: Vec<Pick>, now: DateTime<Utc>, key: &str) -> Shelf {
    let today = todays_order(&ranked, now.date_naive(), key);
    Shelf { ranked, today }
}

/// Old rotation: the most played tenth of the library, not played in the window.
/// Forgotten favorites: favorite albums or albums with two loved songs, not
/// played in the window and not already in old rotation.
pub fn rediscover(candidates: &[Candidate], hidden: &Hidden, now: DateTime<Utc>) -> Rediscover {
    let cutoff = now - chrono::Duration::days(WINDOW_DAYS);
    let resting = |album: &Album| played(album).is_none_or(|at| at < cutoff);
    let candidates = candidates
        .iter()
        .filter(|candidate| !hidden.hides(&candidate.album.id, now))
        .collect::<Vec<_>>();
    let mut counts = candidates
        .iter()
        .map(|candidate| plays(&candidate.album))
        .filter(|plays| *plays > 0)
        .collect::<Vec<_>>();
    counts.sort_unstable_by(|a, b| b.cmp(a));
    let threshold = counts
        .get(counts.len().div_ceil(10).saturating_sub(1))
        .copied()
        .unwrap_or(u64::MAX)
        .max(MIN_ROTATION_PLAYS);
    let mut old_rotation = candidates
        .iter()
        .filter_map(|candidate| {
            let album = &candidate.album;
            let at = played(album)?;
            (plays(album) >= threshold && at < cutoff).then(|| Pick {
                album: album.clone(),
                reason: Reason::Rotation {
                    plays: plays(album),
                    played: at,
                },
                loved: candidate.loved.clone(),
                top_song: candidate.top_song.clone(),
            })
        })
        .collect::<Vec<_>>();
    old_rotation.sort_by(|a, b| {
        plays(&b.album)
            .cmp(&plays(&a.album))
            .then_with(|| a.album.id.cmp(&b.album.id))
    });
    old_rotation.truncate(SHELF_LIMIT);
    let mut forgotten = candidates
        .iter()
        .filter(|candidate| {
            (favorite(&candidate.album) || candidate.loved.len() >= 2)
                && resting(&candidate.album)
                && !old_rotation
                    .iter()
                    .any(|pick| pick.album.id == candidate.album.id)
        })
        .collect::<Vec<_>>();
    forgotten.sort_by(|a, b| {
        favorite(&b.album)
            .cmp(&favorite(&a.album))
            .then_with(|| b.loved.len().cmp(&a.loved.len()))
            .then_with(|| played(&a.album).cmp(&played(&b.album)))
            .then_with(|| a.album.id.cmp(&b.album.id))
    });
    let forgotten: Vec<Pick> = forgotten
        .into_iter()
        .take(SHELF_LIMIT)
        .map(|candidate| Pick {
            album: candidate.album.clone(),
            reason: Reason::Favorite {
                loved_songs: candidate.loved.len(),
                played: played(&candidate.album),
            },
            loved: candidate.loved.clone(),
            top_song: candidate.top_song.clone(),
        })
        .collect();
    let lane = lane(&old_rotation, now);
    let everything = old_rotation
        .iter()
        .chain(&forgotten)
        .cloned()
        .collect::<Vec<_>>();
    Rediscover {
        spotlight: todays_order(&everything, now.date_naive(), "spotlight"),
        old_rotation: shelf(old_rotation, now, "old-rotation"),
        forgotten_favorites: shelf(forgotten, now, "forgotten-favorites"),
        lane,
    }
}
