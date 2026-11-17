//! Read-only Last.fm lookups of a listener's public album charts.
use crate::discover::ChartAlbum;
use anyhow::{Result, anyhow, bail};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};
use url::Url;

#[derive(Clone)]
pub struct LastFm {
    http: reqwest::Client,
    base: Url,
    key: Option<String>,
    /// Last.fm asks clients to stay well under five requests a second.
    interval: Duration,
    last: Arc<Mutex<Option<Instant>>>,
}

impl LastFm {
    /// Last.fm without a key until one is entered in Settings.
    pub fn new() -> Result<Self> {
        Self::with_endpoints(
            "https://ws.audioscrobbler.com/2.0/",
            None,
            Duration::from_millis(250),
        )
    }

    pub fn with_endpoints(base: &str, key: Option<String>, interval: Duration) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("Sonora/", env!("CARGO_PKG_VERSION")))
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()?,
            base: Url::parse(base)?,
            key,
            interval,
            last: Arc::default(),
        })
    }

    /// Uses an API key entered in Settings.
    pub fn set_key(&mut self, key: String) {
        self.key = Some(key);
    }

    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    async fn get(&self, user: &str, method: &str, params: &[(&str, &str)]) -> Result<Value> {
        let Some(key) = &self.key else {
            bail!("Add a Last.fm API key in Settings");
        };
        let mut url = self.base.clone();
        url.query_pairs_mut()
            .append_pair("method", method)
            .append_pair("user", user)
            .extend_pairs(params)
            .append_pair("api_key", key)
            .append_pair("format", "json");
        {
            let mut last = self.last.lock().await;
            if let Some(at) = *last {
                tokio::time::sleep_until(at + self.interval).await;
            }
            *last = Some(Instant::now());
        }
        let response = self.http.get(url).send().await.map_err(|error| {
            anyhow::Error::new(error.without_url()).context("Cannot reach Last.fm")
        })?;
        let status = response.status();
        let body: Value = response.json().await.map_err(|_| {
            anyhow!(
                "Last.fm returned an invalid response (HTTP {})",
                status.as_u16()
            )
        })?;
        if let Some(code) = body["error"].as_u64() {
            bail!(match code {
                6 => format!("Last.fm has no user named {user}"),
                17 => format!("{user}'s Last.fm listening history is private"),
                10 | 26 => "Last.fm did not accept Sonora's API key".into(),
                29 => "Last.fm is busy; try again later".into(),
                _ => format!("Last.fm returned error {code}"),
            });
        }
        if !status.is_success() {
            bail!("Last.fm returned HTTP {}", status.as_u16());
        }
        Ok(body)
    }

    /// When the listener's profile was created, in Unix seconds.
    pub async fn registered(&self, user: &str) -> Result<i64> {
        let body = self.get(user, "user.getinfo", &[]).await?;
        let registered = &body["user"]["registered"];
        registered["unixtime"]
            .as_str()
            .and_then(|text| text.parse().ok())
            .or_else(|| registered["#text"].as_i64())
            .ok_or_else(|| anyhow!("Last.fm did not say when {user} joined"))
    }

    /// The listener's most played albums between two Unix times.
    pub async fn album_chart(&self, user: &str, from: i64, to: i64) -> Result<Vec<ChartAlbum>> {
        let (from, to) = (from.to_string(), to.to_string());
        let body = self
            .get(
                user,
                "user.getweeklyalbumchart",
                &[("from", &from), ("to", &to)],
            )
            .await?;
        let albums = &body["weeklyalbumchart"]["album"];
        let entries = match albums {
            Value::Array(entries) => entries.iter().collect::<Vec<_>>(),
            Value::Object(_) => vec![albums],
            _ => Vec::new(),
        };
        Ok(entries
            .into_iter()
            .filter_map(|entry| {
                let plays = match &entry["playcount"] {
                    Value::String(text) => text.parse().ok()?,
                    value => value.as_u64()?,
                };
                Some(ChartAlbum {
                    artist: entry["artist"]["#text"].as_str()?.to_string(),
                    name: entry["name"].as_str()?.to_string(),
                    plays,
                })
            })
            .collect())
    }
}
