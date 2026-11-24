//! A listener's ListenBrainz history, read with their user token.
//!
//! ListenBrainz asks clients not to page through whole histories, so the
//! first import downloads an export of every listen, and later visits only
//! fetch the listens that arrived since.
use crate::history::{
    BoxFuture, ChartAlbum, History, HistoryProvider, HistoryUpdate, ListeningHistory, Report,
    SEASON_REFRESH_MS, SeasonChart,
};
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Seek, Write},
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::{sync::Mutex, time::Instant};
use url::Url;

/// The most listens one request may return.
const PAGE: usize = 1000;
/// Pages a top-up may read before a fresh export is cheaper and kinder.
const TOP_UP_PAGES: usize = 10;
/// The longest a rate-limit wait is honored before giving up.
const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// One listen: when, by whom, and from which release.
#[derive(Clone, Debug, PartialEq)]
pub struct Listen {
    pub at: i64,
    pub artist: String,
    pub release: String,
}

#[derive(Clone)]
pub struct ListenBrainz {
    http: reqwest::Client,
    base: Url,
    /// Shared by clones, so a token entered in Settings reaches every holder.
    token: Arc<RwLock<Option<String>>>,
    /// ListenBrainz asks for no more than one call a second.
    interval: Duration,
    /// How long to wait between checks on an export being prepared.
    poll: Duration,
    last: Arc<Mutex<Option<Instant>>>,
}

impl ListenBrainz {
    /// ListenBrainz without a token until one is entered in Settings.
    pub fn new() -> Result<Self> {
        Self::with_endpoints(
            "https://api.listenbrainz.org/1/",
            None,
            Duration::from_secs(1),
            Duration::from_secs(10),
        )
    }

    pub fn with_endpoints(
        base: &str,
        token: Option<String>,
        interval: Duration,
        poll: Duration,
    ) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("Sonora/", env!("CARGO_PKG_VERSION")))
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(300))
                .build()?,
            base: Url::parse(base)?,
            token: Arc::new(RwLock::new(token)),
            interval,
            poll,
            last: Arc::default(),
        })
    }

    pub fn set_token(&self, token: String) {
        *self
            .token
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(token);
    }

    fn token(&self) -> Option<String> {
        self.token
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Sends one throttled request, waiting out a rate limit once.
    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        user: Option<&str>,
    ) -> Result<reqwest::Response> {
        let token = self
            .token()
            .ok_or_else(|| anyhow!("Add a ListenBrainz user token in Settings"))?;
        let mut url = self.base.join(path)?;
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
        }
        for attempt in 0..2 {
            {
                let mut last = self.last.lock().await;
                if let Some(at) = *last {
                    tokio::time::sleep_until(at + self.interval).await;
                }
                *last = Some(Instant::now());
            }
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .header("Authorization", format!("Token {token}"));
            if method == Method::POST {
                request = request.json(&serde_json::json!({}));
            }
            let response = request.send().await.map_err(|error| {
                anyhow::Error::new(error.without_url()).context("Cannot reach ListenBrainz")
            })?;
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS && attempt == 0 {
                let wait = response
                    .headers()
                    .get("X-RateLimit-Reset-In")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .unwrap_or(self.interval);
                if wait <= LONGEST_WAIT {
                    tokio::time::sleep(wait).await;
                    continue;
                }
            }
            return match status {
                status if status.is_success() => Ok(response),
                StatusCode::UNAUTHORIZED => {
                    bail!("ListenBrainz did not accept your user token")
                }
                StatusCode::NOT_FOUND if user.is_some() => {
                    bail!(
                        "ListenBrainz has no user named {}",
                        user.unwrap_or_default()
                    )
                }
                StatusCode::TOO_MANY_REQUESTS => bail!("ListenBrainz is busy; try again later"),
                StatusCode::SERVICE_UNAVAILABLE => {
                    bail!("ListenBrainz is unavailable; try again later")
                }
                status => bail!("ListenBrainz returned HTTP {}", status.as_u16()),
            };
        }
        bail!("ListenBrainz is busy; try again later")
    }

    async fn json(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        user: Option<&str>,
    ) -> Result<Value> {
        let response = self.send(method, path, query, user).await?;
        response
            .json()
            .await
            .map_err(|_| anyhow!("ListenBrainz returned an invalid response"))
    }

    /// The user the saved token belongs to.
    pub async fn token_owner(&self) -> Result<String> {
        let body = self.json(Method::GET, "validate-token", &[], None).await?;
        if body["valid"].as_bool() != Some(true) {
            bail!("ListenBrainz did not accept your user token");
        }
        body["user_name"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("ListenBrainz returned an invalid response"))
    }

    /// Every listen of the token's owner, from a full export.
    pub async fn export(&self) -> Result<Vec<Listen>> {
        let exports = self.json(Method::GET, "export/list", &[], None).await?;
        let pending =
            exports.as_array().into_iter().flatten().find(|export| {
                matches!(export["status"].as_str(), Some("waiting" | "in_progress"))
            });
        let mut export = match pending {
            Some(export) => export.clone(),
            None => self.json(Method::POST, "export/", &[], None).await?,
        };
        let id = export["export_id"]
            .as_i64()
            .ok_or_else(|| anyhow!("ListenBrainz returned an invalid response"))?;
        loop {
            match export["status"].as_str() {
                Some("completed") => break,
                Some("failed") => bail!("ListenBrainz could not prepare your export"),
                Some("waiting" | "in_progress") => {
                    tokio::time::sleep(self.poll).await;
                    export = self
                        .json(Method::GET, &format!("export/{id}"), &[], None)
                        .await?;
                }
                _ => bail!("ListenBrainz returned an invalid response"),
            }
        }
        let mut response = self
            .send(Method::GET, &format!("export/{id}/download"), &[], None)
            .await?;
        let mut file = tempfile::tempfile().context("Could not store the ListenBrainz export")?;
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            anyhow::Error::new(error.without_url()).context("Cannot reach ListenBrainz")
        })? {
            file.write_all(&chunk)
                .context("Could not store the ListenBrainz export")?;
        }
        tokio::task::spawn_blocking(move || read_export(file)).await?
    }

    /// Listens newer than `after`, newest first; `None` when there are too
    /// many to fetch politely and a fresh export should be made instead.
    pub async fn listens_since(&self, user: &str, after: i64) -> Result<Option<Vec<Listen>>> {
        let path = format!("user/{user}/listens");
        let mut found = Vec::new();
        let mut before: Option<i64> = None;
        for _ in 0..TOP_UP_PAGES {
            let mut query = vec![("count", PAGE.to_string())];
            if let Some(before) = before {
                query.push(("max_ts", before.to_string()));
            }
            let body = self.json(Method::GET, &path, &query, Some(user)).await?;
            let page = body["payload"]["listens"]
                .as_array()
                .ok_or_else(|| anyhow!("ListenBrainz returned an invalid response"))?;
            let mut reached = page.len() < PAGE;
            for listen in page {
                let Some(listen) = parse_listen(listen) else {
                    continue;
                };
                if listen.at <= after {
                    reached = true;
                    break;
                }
                before = Some(before.map_or(listen.at, |before| before.min(listen.at)));
                found.push(listen);
            }
            if reached {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }
}

/// One listen from the API or an export line; listens with no release are
/// skipped because Memory lane ranks albums.
fn parse_listen(value: &Value) -> Option<Listen> {
    let metadata = &value["track_metadata"];
    let release = metadata["release_name"].as_str()?.trim();
    let artist = metadata["artist_name"].as_str()?.trim();
    let at = value["listened_at"].as_i64()?;
    (!release.is_empty() && !artist.is_empty()).then(|| Listen {
        at,
        artist: artist.into(),
        release: release.into(),
    })
}

/// Reads `listens/<year>/<month>.jsonl` out of an export archive.
fn read_export(file: impl std::io::Read + Seek) -> Result<Vec<Listen>> {
    let mut archive =
        zip::ZipArchive::new(file).context("ListenBrainz sent an unreadable export")?;
    let mut listens = Vec::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .context("ListenBrainz sent an unreadable export")?;
        let name = entry
            .name()
            .context("ListenBrainz sent an unreadable export")?
            .to_string();
        if !(name.starts_with("listens/") && name.ends_with(".jsonl")) {
            continue;
        }
        for line in BufReader::new(entry).lines() {
            let line = line.context("ListenBrainz sent an unreadable export")?;
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && let Some(listen) = parse_listen(&value)
            {
                listens.push(listen);
            }
        }
    }
    Ok(listens)
}

/// Adds listens to the season charts they fall in, returning only the
/// seasons that changed.
pub fn add_listens(
    seasons: &BTreeMap<String, SeasonChart>,
    listens: &[Listen],
    fetched_ms: i64,
) -> Vec<(String, SeasonChart)> {
    let mut changed: BTreeMap<String, SeasonChart> = BTreeMap::new();
    for listen in listens {
        let span = crate::history::season_of(listen.at);
        let chart = changed.entry(span.key.clone()).or_insert_with(|| {
            seasons.get(&span.key).cloned().unwrap_or(SeasonChart {
                from: span.from,
                to: span.to,
                ..Default::default()
            })
        });
        match chart
            .albums
            .iter_mut()
            .find(|album| album.artist == listen.artist && album.name == listen.release)
        {
            Some(album) => album.plays += 1,
            None => chart.albums.push(ChartAlbum {
                artist: listen.artist.clone(),
                name: listen.release.clone(),
                plays: 1,
            }),
        }
    }
    changed
        .into_iter()
        .map(|(key, mut chart)| {
            chart.fetched_ms = fetched_ms;
            chart.albums.sort_by(|a, b| {
                b.plays
                    .cmp(&a.plays)
                    .then_with(|| a.artist.cmp(&b.artist))
                    .then_with(|| a.name.cmp(&b.name))
            });
            (key, chart)
        })
        .collect()
}

impl ListeningHistory for ListenBrainz {
    fn provider(&self) -> HistoryProvider {
        HistoryProvider::ListenBrainz
    }

    fn ready(&self) -> bool {
        self.token().is_some()
    }

    fn set_secret(&self, secret: String) {
        self.set_token(secret);
    }

    fn needs_sync(&self, history: &History, now: DateTime<Utc>) -> bool {
        history.newest.is_none() || now.timestamp_millis() - history.synced_ms >= SEASON_REFRESH_MS
    }

    fn sync<'a>(
        &'a self,
        user: &'a str,
        history: History,
        report: Report<'a>,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let owner = self.token_owner().await?;
            if !owner.eq_ignore_ascii_case(user) {
                bail!("This ListenBrainz token belongs to {owner}, not {user}");
            }
            let now = Utc::now();
            let recent = match history.newest {
                Some(newest) => self.listens_since(user, newest).await?,
                None => None,
            };
            let update = match recent {
                Some(listens) => HistoryUpdate::Merge {
                    registered: None,
                    seasons: add_listens(&history.seasons, &listens, now.timestamp_millis()),
                    newest: listens.iter().map(|listen| listen.at).max(),
                    synced_ms: now.timestamp_millis(),
                },
                None => {
                    let listens = self.export().await?;
                    let mut seasons =
                        add_listens(&BTreeMap::new(), &listens, now.timestamp_millis());
                    // A fresh export replaces every season it does not cover.
                    for key in history.seasons.keys() {
                        if !seasons.iter().any(|(found, _)| found == key) {
                            seasons.push((key.clone(), SeasonChart::default()));
                        }
                    }
                    let oldest = listens.iter().map(|listen| listen.at).min();
                    HistoryUpdate::Merge {
                        registered: Some(oldest.unwrap_or(now.timestamp())),
                        seasons,
                        newest: Some(
                            listens
                                .iter()
                                .map(|listen| listen.at)
                                .max()
                                .unwrap_or(now.timestamp()),
                        ),
                        synced_ms: now.timestamp_millis(),
                    }
                }
            };
            report(update);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::season_of;

    fn listen(at: i64, artist: &str, release: &str) -> Listen {
        Listen {
            at,
            artist: artist.into(),
            release: release.into(),
        }
    }

    #[test]
    fn listens_are_counted_into_the_seasons_they_fall_in() {
        let spring = season_of(1_620_000_000);
        let summer = season_of(1_626_000_000);
        let listens = [
            listen(spring.from + 10, "A", "One"),
            listen(spring.from + 20, "A", "One"),
            listen(spring.from + 30, "B", "Two"),
            listen(summer.from + 5, "A", "One"),
        ];
        let changed = add_listens(&BTreeMap::new(), &listens, 7);
        assert_eq!(changed.len(), 2);
        let (key, chart) = &changed[0];
        assert_eq!(key, &spring.key);
        assert_eq!(
            (chart.from, chart.to, chart.fetched_ms),
            (spring.from, spring.to, 7)
        );
        assert_eq!(
            chart
                .albums
                .iter()
                .map(|album| (album.name.as_str(), album.plays))
                .collect::<Vec<_>>(),
            [("One", 2), ("Two", 1)]
        );
    }

    #[test]
    fn new_listens_add_to_a_kept_season_without_touching_others() {
        let spring = season_of(1_620_000_000);
        let mut kept = BTreeMap::new();
        kept.insert(
            spring.key.clone(),
            SeasonChart {
                from: spring.from,
                to: spring.to,
                fetched_ms: 1,
                albums: vec![ChartAlbum {
                    artist: "A".into(),
                    name: "One".into(),
                    plays: 4,
                }],
            },
        );
        kept.insert("2001-spring".into(), SeasonChart::default());
        let changed = add_listens(&kept, &[listen(spring.from + 1, "A", "One")], 2);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].1.albums[0].plays, 5);
    }

    #[test]
    fn listens_without_a_release_are_skipped() {
        let value = serde_json::json!({
            "listened_at": 5,
            "track_metadata": {"artist_name": "A", "track_name": "T"}
        });
        assert_eq!(parse_listen(&value), None);
        let value = serde_json::json!({
            "listened_at": 5,
            "track_metadata": {"artist_name": " A ", "release_name": " R ", "track_name": "T"}
        });
        assert_eq!(parse_listen(&value), Some(listen(5, "A", "R")));
    }

    #[test]
    fn an_export_archive_yields_its_listens_only() {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("user.json", options).unwrap();
            zip.write_all(br#"{"user_name":"listener"}"#).unwrap();
            zip.start_file("feedback.jsonl", options).unwrap();
            zip.write_all(b"{\"listened_at\":1,\"track_metadata\":{\"artist_name\":\"X\",\"release_name\":\"Y\"}}\n").unwrap();
            zip.start_file("listens/2021/5.jsonl", options).unwrap();
            zip.write_all(b"{\"listened_at\":3,\"track_metadata\":{\"artist_name\":\"A\",\"release_name\":\"R\"}}\nnot json\n{\"listened_at\":4,\"track_metadata\":{\"artist_name\":\"B\",\"release_name\":\"S\"}}\n").unwrap();
            zip.finish().unwrap();
        }
        buffer.set_position(0);
        assert_eq!(
            read_export(buffer).unwrap(),
            [listen(3, "A", "R"), listen(4, "B", "S")]
        );
    }
}
