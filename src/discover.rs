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

#[derive(Clone, Debug, PartialEq)]
pub struct Reason {
    pub plays: u64,
    pub played: DateTime<Utc>,
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
    pub lane: Vec<Season>,
}

/// The shelves the listener has expanded on the page.
#[derive(Clone, Debug, Default)]
pub struct Visit {
    expanded: HashSet<String>,
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

fn last_played(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
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
        format!(
            "{} plays \u{b7} {}",
            self.reason.plays,
            last_played(self.reason.played, now)
        )
    }
}

/// Groups old rotation by the season each album was last played, newest first;
/// anything from a year ago or more shares one season.
fn lane(ranked: &[Pick], now: DateTime<Utc>) -> Vec<Season> {
    let mut seasons: Vec<(i64, Season)> = Vec::new();
    for pick in ranked {
        let played = pick.reason.played;
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

fn shelf(ranked: Vec<Pick>, now: DateTime<Utc>, key: &str) -> Shelf {
    let today = todays_order(&ranked, now.date_naive(), key);
    Shelf { ranked, today }
}

/// Old rotation: the most played tenth of the library, not played in the
/// window, grouped into Memory lane by the season each was last played.
pub fn rediscover(albums: &[Arc<Album>], hidden: &Hidden, now: DateTime<Utc>) -> Rediscover {
    let cutoff = now - chrono::Duration::days(WINDOW_DAYS);
    let albums = albums
        .iter()
        .filter(|album| !hidden.hides(&album.id, now))
        .collect::<Vec<_>>();
    let mut counts = albums
        .iter()
        .map(|album| plays(album))
        .filter(|plays| *plays > 0)
        .collect::<Vec<_>>();
    counts.sort_unstable_by(|a, b| b.cmp(a));
    let threshold = counts
        .get(counts.len().div_ceil(10).saturating_sub(1))
        .copied()
        .unwrap_or(u64::MAX)
        .max(MIN_ROTATION_PLAYS);
    let mut old_rotation = albums
        .iter()
        .filter_map(|album| {
            let at = played(album)?;
            (plays(album) >= threshold && at < cutoff).then(|| Pick {
                album: (*album).clone(),
                reason: Reason {
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
    Rediscover {
        lane: lane(&old_rotation, now),
        old_rotation: shelf(old_rotation, now, "old-rotation"),
    }
}
