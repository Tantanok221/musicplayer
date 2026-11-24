//! A listener's album charts by season, read from ListenBrainz or Last.fm
//! behind one interface. Nothing in the app reads them since Discover was
//! removed; the clients are kept for a future feature.
use crate::{lastfm::LastFm, listenbrainz::ListenBrainz};
use anyhow::Result;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

/// One album on a listening-history chart.
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

/// A listener's album charts by season, kept between visits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub user: String,
    /// When the profile was created, in Unix seconds.
    pub registered: Option<i64>,
    pub seasons: BTreeMap<String, SeasonChart>,
    /// The newest listen counted, in Unix seconds, for services that are
    /// topped up with new listens rather than asked for whole charts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest: Option<i64>,
    /// When the history was last brought up to date, in Unix milliseconds.
    #[serde(skip_serializing_if = "is_zero")]
    pub synced_ms: i64,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// A season's key, label and bounds in Unix seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct SeasonSpan {
    pub key: String,
    pub label: String,
    pub from: i64,
    pub to: i64,
}

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

/// The season holding a Unix time.
pub fn season_of(at: i64) -> SeasonSpan {
    season_span(DateTime::from_timestamp(at, 0).unwrap_or_default()).1
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

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The service a listener's history is read from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HistoryProvider {
    #[default]
    ListenBrainz,
    LastFm,
}

impl HistoryProvider {
    pub const ALL: [Self; 2] = [Self::ListenBrainz, Self::LastFm];

    pub fn name(self) -> &'static str {
        match self {
            Self::ListenBrainz => "ListenBrainz",
            Self::LastFm => "Last.fm",
        }
    }

    /// What the provider calls the secret Settings asks for.
    pub fn secret_name(self) -> &'static str {
        match self {
            Self::ListenBrainz => "user token",
            Self::LastFm => "API key",
        }
    }

    /// The catalog state key a listener's history is kept under. Last.fm keeps
    /// its original key so histories saved before ListenBrainz still load.
    pub fn state_key(self, user: &str) -> String {
        let user = user.to_lowercase();
        match self {
            Self::ListenBrainz => format!("listenbrainz-history:v1:{user}"),
            Self::LastFm => format!("lastfm-history:v1:{user}"),
        }
    }

    /// The private file under `credentials/` holding the secret.
    pub fn secret_file(self) -> &'static str {
        match self {
            Self::ListenBrainz => "listenbrainz.token",
            Self::LastFm => "lastfm.key",
        }
    }
}

/// What a sync hands back as it goes; the worker stores each one.
#[derive(Clone, Debug, PartialEq)]
pub enum HistoryUpdate {
    /// When the history starts, in Unix seconds.
    Registered(i64),
    /// One season's chart, replacing what was kept, with progress.
    Season {
        key: String,
        chart: SeasonChart,
        done: usize,
        total: usize,
    },
    /// Several seasons and the newest listen they cover, stored together so a
    /// top-up is never counted twice.
    Merge {
        registered: Option<i64>,
        seasons: Vec<(String, SeasonChart)>,
        newest: Option<i64>,
        synced_ms: i64,
    },
}

impl HistoryUpdate {
    /// Applies the update to a kept history.
    pub fn apply(self, history: &mut History) {
        match self {
            Self::Registered(at) => history.registered = Some(at),
            Self::Season { key, chart, .. } => {
                history.seasons.insert(key, chart);
            }
            Self::Merge {
                registered,
                seasons,
                newest,
                synced_ms,
            } => {
                if registered.is_some() {
                    history.registered = registered;
                }
                history.seasons.extend(seasons);
                if newest.is_some() {
                    history.newest = newest;
                }
                history.synced_ms = synced_ms;
            }
        }
    }
}

pub type Report<'a> = &'a (dyn Fn(HistoryUpdate) + Send + Sync);

/// A service Memory lane can read a listener's history from.
pub trait ListeningHistory: Send + Sync {
    fn provider(&self) -> HistoryProvider;
    /// False until the secret the service needs has been entered.
    fn ready(&self) -> bool;
    /// Stores a secret entered in Settings for later requests.
    fn set_secret(&self, secret: String);
    /// Whether the kept history is missing or old enough to ask again.
    fn needs_sync(&self, history: &History, now: DateTime<Utc>) -> bool;
    /// Brings `history` up to date, reporting each change as it arrives.
    fn sync<'a>(
        &'a self,
        user: &'a str,
        history: History,
        report: Report<'a>,
    ) -> BoxFuture<'a, Result<()>>;
}

/// The history services handed to the worker; it picks one per request.
#[derive(Clone)]
pub struct HistorySources {
    pub lastfm: LastFm,
    pub listenbrainz: ListenBrainz,
}

impl HistorySources {
    pub fn new() -> Result<Self> {
        Ok(Self {
            lastfm: LastFm::new()?,
            listenbrainz: ListenBrainz::new()?,
        })
    }

    pub fn get(&self, provider: HistoryProvider) -> Arc<dyn ListeningHistory> {
        match provider {
            HistoryProvider::ListenBrainz => Arc::new(self.listenbrainz.clone()),
            HistoryProvider::LastFm => Arc::new(self.lastfm.clone()),
        }
    }
}

/// The provider saved preferences without a choice get: Last.fm for anyone
/// who already set it up, ListenBrainz otherwise.
pub fn default_provider(lastfm_user: &str, lastfm_secret: bool) -> HistoryProvider {
    if !lastfm_user.trim().is_empty() || lastfm_secret {
        HistoryProvider::LastFm
    } else {
        HistoryProvider::ListenBrainz
    }
}

/// Makes `provider` the service Memory lane reads.
pub fn choose(preferences: &mut crate::model::Preferences, provider: HistoryProvider) {
    preferences.history_provider = Some(provider);
}

/// Saves the Settings form for `provider`: the trimmed username goes into
/// preferences, and a non-empty secret is handed back to send to the worker
/// so it never lands in preferences.
pub fn save_form(
    preferences: &mut crate::model::Preferences,
    provider: HistoryProvider,
    user: &str,
    secret: &str,
) -> Option<String> {
    let user = user.trim().to_string();
    match provider {
        HistoryProvider::ListenBrainz => preferences.listenbrainz_user = user,
        HistoryProvider::LastFm => preferences.lastfm_user = user,
    }
    preferences.history_provider = Some(provider);
    Some(secret.trim().to_string()).filter(|secret| !secret.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Preferences;

    #[test]
    fn saving_a_form_keeps_the_other_service_and_the_secret_out_of_preferences() {
        let mut preferences = Preferences {
            lastfm_user: "old".into(),
            ..Default::default()
        };
        let secret = save_form(
            &mut preferences,
            HistoryProvider::ListenBrainz,
            "  listener ",
            " token ",
        );
        assert_eq!(secret.as_deref(), Some("token"));
        assert_eq!(preferences.listenbrainz_user, "listener");
        assert_eq!(preferences.lastfm_user, "old");
        assert_eq!(
            preferences.history(),
            (HistoryProvider::ListenBrainz, "listener")
        );
        assert_eq!(
            save_form(
                &mut preferences,
                HistoryProvider::ListenBrainz,
                "listener",
                "  "
            ),
            None
        );
        choose(&mut preferences, HistoryProvider::LastFm);
        assert_eq!(preferences.history(), (HistoryProvider::LastFm, "old"));
    }

    #[test]
    fn people_who_set_up_lastfm_keep_it_and_everyone_else_gets_listenbrainz() {
        assert_eq!(default_provider("", false), HistoryProvider::ListenBrainz);
        assert_eq!(default_provider("  ", false), HistoryProvider::ListenBrainz);
        assert_eq!(default_provider("listener", false), HistoryProvider::LastFm);
        assert_eq!(default_provider("", true), HistoryProvider::LastFm);
    }

    #[test]
    fn last_fm_keeps_its_original_state_key() {
        assert_eq!(
            HistoryProvider::LastFm.state_key("Listener"),
            "lastfm-history:v1:listener"
        );
        assert_eq!(
            HistoryProvider::ListenBrainz.state_key("Listener"),
            "listenbrainz-history:v1:listener"
        );
    }

    #[test]
    fn a_merge_keeps_what_it_does_not_replace() {
        let mut history = History {
            registered: Some(10),
            newest: Some(20),
            ..Default::default()
        };
        history
            .seasons
            .insert("2020-spring".into(), SeasonChart::default());
        HistoryUpdate::Merge {
            registered: None,
            seasons: vec![("2020-summer".into(), SeasonChart::default())],
            newest: Some(30),
            synced_ms: 5,
        }
        .apply(&mut history);
        assert_eq!(history.registered, Some(10));
        assert_eq!(history.newest, Some(30));
        assert_eq!(history.synced_ms, 5);
        assert_eq!(history.seasons.len(), 2);
    }
}
