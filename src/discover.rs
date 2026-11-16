//! Shelves of owned music that has fallen out of rotation.
use crate::model::Album;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

pub const WINDOW_DAYS: i64 = 182;
pub const SHELF_LIMIT: usize = 48;
pub const MIN_ROTATION_PLAYS: u64 = 5;
pub const NOT_NOW_DAYS: i64 = 30;

/// An album with how many of its songs are favorited or rated four stars or more.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub album: Arc<Album>,
    pub loved_songs: usize,
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

#[derive(Clone, Debug, Default)]
pub struct Rediscover {
    pub old_rotation: Shelf,
    pub forgotten_favorites: Shelf,
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
            (favorite(&candidate.album) || candidate.loved_songs >= 2)
                && resting(&candidate.album)
                && !old_rotation
                    .iter()
                    .any(|pick| pick.album.id == candidate.album.id)
        })
        .collect::<Vec<_>>();
    forgotten.sort_by(|a, b| {
        favorite(&b.album)
            .cmp(&favorite(&a.album))
            .then_with(|| b.loved_songs.cmp(&a.loved_songs))
            .then_with(|| played(&a.album).cmp(&played(&b.album)))
            .then_with(|| a.album.id.cmp(&b.album.id))
    });
    let forgotten = forgotten
        .into_iter()
        .take(SHELF_LIMIT)
        .map(|candidate| Pick {
            album: candidate.album.clone(),
            reason: Reason::Favorite {
                loved_songs: candidate.loved_songs,
                played: played(&candidate.album),
            },
        })
        .collect();
    Rediscover {
        old_rotation: shelf(old_rotation, now, "old-rotation"),
        forgotten_favorites: shelf(forgotten, now, "forgotten-favorites"),
    }
}
