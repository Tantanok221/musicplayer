//! Shelves of owned music that has fallen out of rotation.
use crate::model::Album;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

pub const WINDOW_DAYS: i64 = 182;
pub const SHELF_LIMIT: usize = 48;
/// Albums a season shows before See all, however wide the window is.
pub const SEASON_ROW: usize = 8;
pub const MIN_ROTATION_PLAYS: u64 = 5;
pub const NOT_NOW_DAYS: i64 = 30;

/// How often the album was played and when Navidrome last saw it played.
/// `in_season` counts plays within one season of Last.fm history rather than
/// all time.
#[derive(Clone, Debug, PartialEq)]
pub struct Reason {
    pub plays: u64,
    pub played: Option<DateTime<Utc>>,
    pub in_season: bool,
}

/// One album on a Last.fm chart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChartAlbum {
    pub artist: String,
    pub name: String,
    pub plays: u64,
}

/// A season's album chart, with when it was fetched.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeasonChart {
    pub from: i64,
    pub to: i64,
    pub fetched_ms: i64,
    pub albums: Vec<ChartAlbum>,
}

/// A listener's Last.fm album charts by season, kept between visits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub user: String,
    /// When the profile was created, in Unix seconds.
    pub registered: Option<i64>,
    pub seasons: BTreeMap<String, SeasonChart>,
}

/// A season's key, label and bounds in Unix seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct SeasonSpan {
    pub key: String,
    pub label: String,
    pub from: i64,
    pub to: i64,
}

/// Where Memory lane's Last.fm history stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum HistoryStatus {
    /// No Last.fm username is set.
    #[default]
    Off,
    /// This build has no Last.fm API key.
    Unavailable,
    Loading {
        done: usize,
        total: usize,
    },
    Ready,
    Failed(String),
}

pub const MIN_SEASON_PLAYS: u64 = 5;
/// How long an unfinished season's chart is trusted before it is fetched again.
pub const SEASON_REFRESH_MS: i64 = 12 * 3_600_000;

fn first_of(year: i32, month: u32) -> i64 {
    NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map_or(0, |at| at.and_utc().timestamp())
}

/// The meteorological season holding `at` (winter is December to February),
/// with a number that sorts seasons in time.
fn season_span(at: DateTime<Utc>) -> (i64, SeasonSpan) {
    let (month, year) = (at.month(), at.year());
    let year = if month <= 2 { year - 1 } else { year };
    let (index, name, from, to) = match month {
        3..=5 => (1, "spring", first_of(year, 3), first_of(year, 6)),
        6..=8 => (2, "summer", first_of(year, 6), first_of(year, 9)),
        9..=11 => (3, "autumn", first_of(year, 9), first_of(year, 12)),
        _ => (4, "winter", first_of(year, 12), first_of(year + 1, 3)),
    };
    let label = if name == "winter" {
        format!("Winter {year}\u{2013}{:02}", (year + 1) % 100)
    } else {
        let mut title = name.to_string();
        title[..1].make_ascii_uppercase();
        format!("{title} {year}")
    };
    (
        i64::from(year) * 4 + index,
        SeasonSpan {
            key: format!("{year}-{name}"),
            label,
            from,
            to,
        },
    )
}

/// Every season from the one holding `first` to the one holding `now`, oldest first.
pub fn seasons_between(first: DateTime<Utc>, now: DateTime<Utc>) -> Vec<SeasonSpan> {
    let mut spans = Vec::new();
    let mut at = first;
    while at <= now {
        let (_, span) = season_span(at);
        let Some(next) = DateTime::from_timestamp(span.to, 0) else {
            break;
        };
        spans.push(span);
        at = next;
    }
    spans
}

/// Seasons whose chart is missing, or was fetched before the season ended
/// and long enough ago to be worth asking for again.
pub fn stale_seasons(history: &History, now: DateTime<Utc>) -> Vec<SeasonSpan> {
    let Some(first) = history
        .registered
        .and_then(|registered| DateTime::from_timestamp(registered, 0))
    else {
        return Vec::new();
    };
    let now_ms = now.timestamp_millis();
    seasons_between(first, now)
        .into_iter()
        .filter(|span| {
            history.seasons.get(&span.key).is_none_or(|chart| {
                chart.fetched_ms < chart.to * 1000 && now_ms - chart.fetched_ms >= SEASON_REFRESH_MS
            })
        })
        .collect()
}

fn same_artist(chart: &str, album: &str) -> bool {
    let (chart, album) = (
        crate::discography::fold(chart),
        crate::discography::fold(album),
    );
    !chart.is_empty()
        && !album.is_empty()
        && (chart == album || chart.contains(&album) || album.contains(&chart))
}

/// Memory lane from Last.fm seasons: each owned album sits in the season it
/// was played most, unless it was played in the last six months on either side.
fn history_lane(albums: &[&Arc<Album>], history: &History, now: DateTime<Utc>) -> Vec<Season> {
    let cutoff = now - chrono::Duration::days(WINDOW_DAYS);
    let mut by_name: HashMap<String, Vec<&Arc<Album>>> = HashMap::new();
    for album in albums {
        by_name
            .entry(crate::discography::fold(&album.name))
            .or_default()
            .push(album);
    }
    let owned = |chart: &ChartAlbum| {
        by_name
            .get(&crate::discography::fold(&chart.name))?
            .iter()
            .find(|album| same_artist(&chart.artist, &album.artist))
            .copied()
    };
    let mut recent = HashSet::new();
    let mut best: HashMap<&str, (u64, i64, SeasonSpan, &Arc<Album>)> = HashMap::new();
    for chart in history.seasons.values() {
        let Some(start) = DateTime::from_timestamp(chart.from, 0) else {
            continue;
        };
        let (order, span) = season_span(start);
        for entry in &chart.albums {
            let Some(album) = owned(entry) else {
                continue;
            };
            if chart.to > cutoff.timestamp() && entry.plays > 0 {
                recent.insert(album.id.as_str());
            }
            if entry.plays < MIN_SEASON_PLAYS {
                continue;
            }
            let better = best
                .get(album.id.as_str())
                .is_none_or(|(plays, seen, ..)| (entry.plays, order) > (*plays, *seen));
            if better {
                best.insert(&album.id, (entry.plays, order, span.clone(), album));
            }
        }
    }
    let mut seasons: Vec<(i64, Season)> = Vec::new();
    for (id, (plays, order, span, album)) in best {
        if recent.contains(id) || played(album).is_some_and(|at| at >= cutoff) {
            continue;
        }
        let pick = Pick {
            album: album.clone(),
            reason: Reason {
                plays,
                played: played(album),
                in_season: true,
            },
        };
        match seasons
            .iter_mut()
            .find(|(_, season)| season.key == span.key)
        {
            Some((_, season)) => season.shelf.ranked.push(pick),
            None => seasons.push((
                order,
                Season {
                    key: span.key,
                    label: span.label,
                    shelf: Shelf { ranked: vec![pick] },
                },
            )),
        }
    }
    seasons.sort_by_key(|(order, _)| std::cmp::Reverse(*order));
    seasons
        .into_iter()
        .map(|(_, mut season)| {
            season.shelf.ranked.sort_by(by_plays);
            season.shelf.ranked.truncate(SHELF_LIMIT);
            season
        })
        .collect()
}

fn by_plays(a: &Pick, b: &Pick) -> std::cmp::Ordering {
    b.reason
        .plays
        .cmp(&a.reason.plays)
        .then_with(|| a.album.id.cmp(&b.album.id))
}

#[derive(Clone, Debug)]
pub struct Pick {
    pub album: Arc<Album>,
    pub reason: Reason,
}

/// One shelf: every pick, most played first.
#[derive(Clone, Debug, Default)]
pub struct Shelf {
    pub ranked: Vec<Pick>,
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
    pub history: HistoryStatus,
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
        let plays = self.reason.plays;
        match (self.reason.in_season, self.reason.played) {
            (true, None) => format!("{plays} plays that season"),
            (true, Some(at)) => {
                format!("{plays} plays that season \u{b7} {}", last_played(at, now))
            }
            (false, at) => format!(
                "{plays} plays \u{b7} {}",
                last_played(at.unwrap_or(now), now)
            ),
        }
    }
}

/// Groups old rotation by the season each album was last played, newest first;
/// anything from a year ago or more shares one season.
fn lane(ranked: &[Pick], now: DateTime<Utc>) -> Vec<Season> {
    let mut seasons: Vec<(i64, Season)> = Vec::new();
    for pick in ranked {
        let Some(played) = pick.reason.played else {
            continue;
        };
        let (order, key, label) = if (now - played).num_days() >= 365 {
            (
                i64::MIN,
                "older".to_string(),
                "A year ago and longer".to_string(),
            )
        } else {
            let (order, span) = season_span(played);
            (order, span.key, span.label)
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
                    },
                },
            )),
        }
    }
    seasons.sort_by_key(|(order, _)| std::cmp::Reverse(*order));
    seasons
        .into_iter()
        .map(|(_, mut season)| {
            season.shelf.ranked.sort_by(by_plays);
            season
        })
        .collect()
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

/// Old rotation: the most played tenth of the library, not played in the
/// window, grouped into Memory lane by the season each was last played.
pub fn rediscover(
    albums: &[Arc<Album>],
    history: Option<&History>,
    hidden: &Hidden,
    now: DateTime<Utc>,
) -> Rediscover {
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
                    played: Some(at),
                    in_season: false,
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
    if let Some(history) = history.filter(|history| !history.seasons.is_empty()) {
        let lane = history_lane(&albums, history, now);
        let mut all = lane
            .iter()
            .flat_map(|season| season.shelf.ranked.iter().cloned())
            .collect::<Vec<_>>();
        all.sort_by(by_plays);
        return Rediscover {
            lane,
            old_rotation: Shelf { ranked: all },
            history: HistoryStatus::Ready,
        };
    }
    Rediscover {
        lane: lane(&old_rotation, now),
        old_rotation: Shelf {
            ranked: old_rotation,
        },
        history: HistoryStatus::Off,
    }
}
