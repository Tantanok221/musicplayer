use crate::{
    lyrics::Lyrics,
    model::*,
    platform::{Account, ServerCredentials},
};
use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

#[derive(Clone)]
pub struct Server {
    pub account: Account,
    // Never derive Debug: credentials and authenticated URLs must stay out of logs.
    credentials: ServerCredentials,
    http: reqwest::Client,
    pub extensions: HashSet<String>,
}

impl Server {
    pub fn new(account: Account, password: String) -> Result<Self> {
        Self::with_credentials(account, ServerCredentials::from_password(&password)?)
    }

    pub fn credentials(&self) -> &ServerCredentials {
        &self.credentials
    }

    pub fn with_credentials(mut account: Account, credentials: ServerCredentials) -> Result<Self> {
        credentials.validate()?;
        let mut url = Url::parse(account.url.trim())
            .context("Enter a complete server URL, including https://")?;
        if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
            bail!("Use an http:// or https:// server URL");
        }
        if !url.username().is_empty() || url.password().is_some() {
            bail!("Enter credentials in the username and password fields");
        }
        url.set_query(None);
        url.set_fragment(None);
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        account.url = url.to_string();
        account.username = account.username.trim().to_string();
        if account.username.is_empty() {
            bail!("Enter your Navidrome username and password");
        }
        Ok(Self {
            account,
            credentials,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build()?,
            extensions: HashSet::new(),
        })
    }
    pub fn url(&self, endpoint: &str, params: &[(&str, String)]) -> Result<Url> {
        let mut url = Url::parse(&self.account.url)?.join(&format!("rest/{endpoint}.view"))?;
        url.query_pairs_mut()
            .extend_pairs([
                ("u", self.account.username.as_str()),
                ("t", self.credentials.token.as_str()),
                ("s", self.credentials.salt.as_str()),
                ("v", "1.16.1"),
                ("c", "Sonora"),
                ("f", "json"),
            ])
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        Ok(url)
    }
    async fn request(&self, endpoint: &str, params: &[(&str, String)]) -> Result<Value> {
        // Strip reqwest's URL-bearing errors before returning anything user-visible.
        let response = self
            .http
            .get(self.url(endpoint, params)?)
            .send()
            .await
            .map_err(|e| anyhow::Error::new(e.without_url()).context("Cannot reach server. For a local server, allow Sonora in System Settings → Privacy & Security → Local Network"))?;
        let status = response.status();
        if !status.is_success() {
            bail!("Server returned HTTP {}", status.as_u16());
        }
        let envelope: Value = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("Server returned an invalid JSON response"))?;
        let body = envelope
            .get("subsonic-response")
            .context("This address did not return a Subsonic response")?;
        if body["status"] != "ok" {
            let code = body["error"]["code"].as_i64().unwrap_or(0);
            if code == 40 || code == 41 || code == 44 {
                bail!("Authentication failed. Check your username and password.");
            }
            bail!(
                "Server rejected {endpoint}: {}",
                body["error"]["message"].as_str().unwrap_or("Unknown error")
            );
        }
        Ok(body.clone())
    }
    async fn value<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        params: &[(&str, String)],
        key: &str,
    ) -> Result<T> {
        let response = self.request(endpoint, params).await?;
        Ok(serde_json::from_value(
            response.get(key).cloned().unwrap_or_else(|| json!({})),
        )?)
    }
    pub async fn connect(&mut self) -> Result<String> {
        let info = self.request("ping", &[]).await?;
        if let Ok(response) = self.request("getOpenSubsonicExtensions", &[]).await
            && let Some(items) = response["openSubsonicExtensions"].as_array()
        {
            for item in items {
                if let Some(name) = item["name"].as_str() {
                    self.extensions.insert(name.into());
                }
            }
        }
        Ok(format!(
            "{} {}",
            info["type"].as_str().unwrap_or("Navidrome"),
            info["serverVersion"].as_str().unwrap_or_default()
        ))
    }
    pub async fn page(&self, kind: &str, offset: usize) -> Result<Value> {
        let params = [
            ("query", "".into()),
            (
                "artistCount",
                if kind == "artist" { "500" } else { "0" }.into(),
            ),
            (
                "albumCount",
                if kind == "album" { "500" } else { "0" }.into(),
            ),
            ("songCount", if kind == "song" { "500" } else { "0" }.into()),
        ];
        let mut params = params.to_vec();
        let key = match kind {
            "artist" => "artistOffset",
            "album" => "albumOffset",
            _ => "songOffset",
        };
        params.push((key, offset.to_string()));
        let response = self.request("search3", &params).await?;
        Ok(response["searchResult3"][kind]
            .as_array()
            .cloned()
            .map(Value::Array)
            .unwrap_or_else(|| json!([])))
    }
    pub async fn scan_status(&self) -> Result<bool> {
        let response = self.request("getScanStatus", &[]).await?;
        Ok(response["scanStatus"]["scanning"]
            .as_bool()
            .unwrap_or(false))
    }
    pub async fn album(&self, id: &str) -> Result<(Album, Vec<Track>)> {
        let data = self.request("getAlbum", &[("id", id.into())]).await?;
        let album = serde_json::from_value(data["album"].clone())?;
        let tracks = serde_json::from_value(
            data["album"]["song"]
                .as_array()
                .cloned()
                .map(Value::Array)
                .unwrap_or_else(|| json!([])),
        )?;
        Ok((album, tracks))
    }
    pub async fn playlists(&self) -> Result<Vec<Playlist>> {
        let response = self.request("getPlaylists", &[]).await?;
        Ok(serde_json::from_value(
            response["playlists"]["playlist"]
                .as_array()
                .cloned()
                .map(Value::Array)
                .unwrap_or_else(|| json!([])),
        )?)
    }
    pub async fn playlist(&self, id: &str) -> Result<Playlist> {
        self.value("getPlaylist", &[("id", id.into())], "playlist")
            .await
    }
    pub async fn create_playlist(&self, name: &str, ids: &[String]) -> Result<()> {
        let mut params = vec![("name", name.into())];
        params.extend(ids.iter().cloned().map(|id| ("songId", id)));
        self.request("createPlaylist", &params).await?;
        Ok(())
    }
    pub async fn replace_playlist(&self, id: &str, ids: &[String]) -> Result<()> {
        let mut params = vec![("playlistId", id.into())];
        params.extend(ids.iter().cloned().map(|id| ("songId", id)));
        self.request("createPlaylist", &params).await?;
        Ok(())
    }
    pub async fn rename_playlist(&self, id: &str, name: &str) -> Result<()> {
        self.request(
            "updatePlaylist",
            &[("playlistId", id.into()), ("name", name.into())],
        )
        .await?;
        Ok(())
    }
    pub async fn delete_playlist(&self, id: &str) -> Result<()> {
        self.request("deletePlaylist", &[("id", id.into())]).await?;
        Ok(())
    }
    pub async fn star(&self, id: &str, starred: bool) -> Result<()> {
        self.request(
            if starred { "star" } else { "unstar" },
            &[("id", id.into())],
        )
        .await?;
        Ok(())
    }
    pub async fn rating(&self, id: &str, rating: u8) -> Result<()> {
        self.request(
            "setRating",
            &[("id", id.into()), ("rating", rating.min(5).to_string())],
        )
        .await?;
        Ok(())
    }
    pub async fn scrobble(&self, id: &str, submission: bool) -> Result<()> {
        self.request(
            "scrobble",
            &[("id", id.into()), ("submission", submission.to_string())],
        )
        .await?;
        Ok(())
    }
    pub async fn lyrics(&self, track: &Track) -> Result<Lyrics> {
        if let Ok(response) = self
            .request("getLyricsBySongId", &[("id", track.id.clone())])
            .await
            && let Some(versions) = response["lyricsList"]["structuredLyrics"].as_array()
        {
            let mut lyrics = versions
                .iter()
                .filter_map(|version| serde_json::from_value::<Lyrics>(version.clone()).ok())
                .filter(|lyrics| !lyrics.is_empty())
                .collect::<Vec<_>>();
            // Prefer one synchronized version rather than concatenating languages/timelines.
            lyrics.sort_by_key(|lyrics| !lyrics.is_timed());
            if let Some(lyrics) = lyrics.into_iter().next() {
                return Ok(lyrics);
            }
        }
        let response = self
            .request(
                "getLyrics",
                &[
                    ("artist", track.artist.clone()),
                    ("title", track.title.clone()),
                ],
            )
            .await?;
        Ok(Lyrics::plain(
            response["lyrics"]["value"].as_str().unwrap_or_default(),
        ))
    }
    pub async fn artwork(&self, id: &str, cache: &Path) -> Result<PathBuf> {
        let dir = cache.join(self.account.namespace());
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(format!("{}.art", blake3::hash(id.as_bytes()).to_hex()));
        if path.exists() {
            return Ok(path);
        }
        let response = self
            .http
            .get(self.url("getCoverArt", &[("id", id.into()), ("size", "512".into())])?)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Artwork request failed: {}", e.without_url()))?;
        if !response.status().is_success() {
            bail!("Artwork unavailable");
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| anyhow::anyhow!("Artwork download failed"))?;
        if bytes.len() > 10 * 1024 * 1024 {
            bail!("Artwork too large");
        }
        crate::platform::atomic_write(&path, &bytes)?;
        // A bounded FIFO cache; reads don't touch mtime or block the renderer.
        let mut entries = std::fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .filter_map(|e| {
                e.metadata()
                    .ok()
                    .map(|m| (e.path(), m.len(), m.modified().ok()))
            })
            .collect::<Vec<_>>();
        let mut total: u64 = entries.iter().map(|e| e.1).sum();
        if total > 512 * 1024 * 1024 {
            entries.sort_by_key(|e| e.2);
            for (candidate, len, _) in entries {
                if total <= 480 * 1024 * 1024 {
                    break;
                }
                if candidate != path && std::fs::remove_file(candidate).is_ok() {
                    total = total.saturating_sub(len);
                }
            }
        }
        Ok(path)
    }
}
