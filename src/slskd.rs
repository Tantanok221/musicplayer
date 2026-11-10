//! A client for a user's own slskd instance: search Soulseek for one release,
//! describe the folders peers offer, and queue the folder the user picks.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};
use url::Url;

/// The Soulseek server bans clients that search too often.
const SEARCH_GAP: Duration = Duration::from_secs(5);
const AUDIO: [&str; 14] = [
    "flac", "mp3", "m4a", "ogg", "opus", "wav", "aiff", "aif", "ape", "wv", "aac", "wma", "dsf",
    "alac",
];
const LOSSLESS: [&str; 8] = ["flac", "wav", "aiff", "aif", "ape", "wv", "dsf", "alac"];

// Grants control of the user's slskd. Never derive Debug or log its contents.
#[derive(Clone)]
pub struct SlskdKey(String);
impl SlskdKey {
    pub fn new(key: &str) -> Result<Self> {
        let key = key.trim();
        anyhow::ensure!(
            key.len() >= 16 && key.len() <= 255 && !key.chars().any(char::is_control),
            "Enter the slskd API key (16 to 255 characters)"
        );
        Ok(Self(key.into()))
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

/// How Sonora proves itself to slskd. Never derive Debug or log its contents.
#[derive(Clone)]
pub enum SlskdAuth {
    /// For an slskd that has authentication turned off.
    None,
    Key(SlskdKey),
    /// The web interface's username and password, exchanged for a session token.
    Login {
        username: String,
        password: String,
    },
}
impl From<SlskdKey> for SlskdAuth {
    fn from(key: SlskdKey) -> Self {
        Self::Key(key)
    }
}
impl SlskdAuth {
    /// What the settings fields describe. An API key wins over a login; all
    /// three empty means nothing was entered.
    pub fn from_fields(key: &str, username: &str, password: &str) -> Result<Option<Self>> {
        if !key.trim().is_empty() {
            return Ok(Some(Self::Key(SlskdKey::new(key)?)));
        }
        let username = username.trim();
        if username.is_empty() && password.is_empty() {
            return Ok(None);
        }
        anyhow::ensure!(
            !username.is_empty() && !password.is_empty(),
            "Enter both the slskd username and password"
        );
        Ok(Some(Self::Login {
            username: username.into(),
            password: password.into(),
        }))
    }
    pub(crate) fn to_file(&self) -> String {
        match self {
            Self::None => json!({}),
            Self::Key(key) => json!({"key": key.expose()}),
            Self::Login { username, password } => {
                json!({"username": username, "password": password})
            }
        }
        .to_string()
    }
    pub(crate) fn from_file(text: &str) -> Result<Self> {
        let text = text.trim();
        if !text.starts_with('{') {
            return Ok(Self::Key(SlskdKey::new(text)?));
        }
        let value: Value = serde_json::from_str(text)?;
        let field = |name: &str| value[name].as_str().unwrap_or_default().to_string();
        Ok(
            Self::from_fields(&field("key"), &field("username"), &field("password"))?
                .unwrap_or(Self::None),
        )
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteFile {
    /// Full path on the peer, backslash separated. Display and request text only.
    pub filename: String,
    pub size: u64,
    pub length: Option<u32>,
    pub bit_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<u32>,
}
impl RemoteFile {
    pub fn name(&self) -> &str {
        self.filename.rsplit('\\').next().unwrap_or_default()
    }
    fn directory(&self) -> &str {
        self.filename.rsplit_once('\\').map_or("", |(dir, _)| dir)
    }
    fn extension(&self) -> String {
        self.name()
            .rsplit_once('.')
            .map_or(String::new(), |(_, ext)| ext.to_ascii_lowercase())
    }
    pub fn is_audio(&self) -> bool {
        AUDIO.contains(&self.extension().as_str())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchResponse {
    pub username: String,
    pub has_free_upload_slot: bool,
    pub queue_length: u64,
    pub upload_speed: u64,
    pub files: Vec<RemoteFile>,
    pub locked_file_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Completeness {
    /// As many audio files as the release has tracks.
    Exact,
    /// More files than tracks: a deluxe edition, or extras.
    Extra,
    Unknown,
    Partial,
}

/// One folder offered by one peer.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub username: String,
    /// Remote folder that holds the release, including any disc subfolders.
    pub directory: String,
    pub files: Vec<RemoteFile>,
    pub tracks: u32,
    pub locked: u32,
    pub size: u64,
    pub seconds: u32,
    pub format: String,
    pub lossless: bool,
    pub completeness: Completeness,
    pub free_slot: bool,
    pub queue: u64,
    pub speed: u64,
}
impl Candidate {
    pub fn folder(&self) -> &str {
        self.directory.rsplit('\\').next().unwrap_or_default()
    }
}

fn is_disc_folder(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    ["cd", "disc", "disk"].iter().any(|prefix| {
        name.strip_prefix(prefix).is_some_and(|rest| {
            let rest = rest.trim_start_matches([' ', '-', '_', '.']);
            !rest.is_empty() && rest.len() <= 2 && rest.chars().all(|c| c.is_ascii_digit())
        })
    })
}

fn rate(hz: u32) -> String {
    let khz = hz as f64 / 1000.;
    if khz.fract() == 0. {
        format!("{khz:.0}")
    } else {
        format!("{khz:.1}")
    }
}

fn uniform<T: PartialEq + Copy>(mut values: impl Iterator<Item = Option<T>>) -> Option<T> {
    let first = values.next()??;
    values.all(|value| value == Some(first)).then_some(first)
}

/// "FLAC 24/96", "MP3 320", "MP3", "Mixed (FLAC, MP3)" or "Unknown".
fn format_summary(audio: &[&RemoteFile]) -> (String, bool) {
    let mut kinds: Vec<String> = Vec::new();
    for file in audio {
        let kind = file.extension();
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    let [kind] = kinds.as_slice() else {
        if kinds.is_empty() {
            return ("Unknown".into(), false);
        }
        let names = kinds
            .iter()
            .map(|kind| kind.to_uppercase())
            .collect::<Vec<_>>();
        return (format!("Mixed ({})", names.join(", ")), false);
    };
    let lossless = LOSSLESS.contains(&kind.as_str());
    let label = kind.to_uppercase();
    let detail = if lossless {
        uniform(audio.iter().map(|file| file.bit_depth))
            .zip(uniform(audio.iter().map(|file| file.sample_rate)))
            .map(|(depth, hz)| format!("{depth}/{}", rate(hz)))
    } else {
        uniform(audio.iter().map(|file| file.bit_rate)).map(|rate| rate.to_string())
    };
    (
        detail.map_or(label.clone(), |detail| format!("{label} {detail}")),
        lossless,
    )
}

/// Groups search hits into folders, best first: complete releases, then
/// lossless, then peers that can send now, then faster peers.
pub fn candidates(responses: &[SearchResponse], expected_tracks: Option<u32>) -> Vec<Candidate> {
    let mut result = Vec::new();
    for response in responses {
        let mut folders: Vec<(String, Vec<RemoteFile>)> = Vec::new();
        for file in &response.files {
            let directory = file.directory();
            let root = match directory.rsplit_once('\\') {
                Some((parent, last)) if is_disc_folder(last) => parent,
                _ => directory,
            };
            match folders.iter_mut().find(|(known, _)| known == root) {
                Some((_, files)) => files.push(file.clone()),
                None => folders.push((root.to_string(), vec![file.clone()])),
            }
        }
        for (directory, mut files) in folders {
            files.sort_by(|a, b| a.filename.cmp(&b.filename));
            let audio = files
                .iter()
                .filter(|file| file.is_audio())
                .collect::<Vec<_>>();
            if audio.is_empty() {
                continue;
            }
            let tracks = audio.len() as u32;
            let (format, lossless) = format_summary(&audio);
            result.push(Candidate {
                username: response.username.clone(),
                tracks,
                locked: response.locked_file_count,
                size: files.iter().map(|file| file.size).sum(),
                seconds: audio.iter().filter_map(|file| file.length).sum(),
                format,
                lossless,
                completeness: match expected_tracks {
                    None => Completeness::Unknown,
                    Some(expected) if tracks == expected => Completeness::Exact,
                    Some(expected) if tracks > expected => Completeness::Extra,
                    Some(_) => Completeness::Partial,
                },
                free_slot: response.has_free_upload_slot,
                queue: response.queue_length,
                speed: response.upload_speed,
                directory,
                files,
            });
        }
    }
    result.sort_by(|a, b| {
        a.completeness
            .cmp(&b.completeness)
            .then(b.lossless.cmp(&a.lossless))
            .then(b.free_slot.cmp(&a.free_slot))
            .then(a.queue.cmp(&b.queue))
            .then(b.speed.cmp(&a.speed))
            .then_with(|| a.username.cmp(&b.username))
    });
    result
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchStatus {
    pub complete: bool,
    pub responses: u64,
}

/// Progress of one queued folder, summed over its files.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub total: u32,
    pub done: u32,
    pub failed: u32,
    /// Why the first failed file failed, in slskd's words.
    pub failure: String,
    pub queued: u32,
    pub place: Option<u32>,
}
impl Progress {
    pub fn finished(&self) -> bool {
        self.total > 0 && self.done + self.failed >= self.total
    }
    pub fn succeeded(&self) -> bool {
        self.total > 0 && self.done == self.total
    }
}

/// Reads one peer's downloads for the files of one job. Files slskd no longer
/// lists are not counted either way.
pub fn progress(downloads: &Value, files: &[String]) -> Progress {
    let mut states: HashMap<&str, (&str, Option<u64>)> = HashMap::new();
    for directory in downloads["directories"].as_array().into_iter().flatten() {
        for transfer in directory["files"].as_array().into_iter().flatten() {
            if let Some(name) = transfer["filename"].as_str() {
                // The same file can be listed once per attempt; the last entry is current.
                states.insert(
                    name,
                    (
                        transfer["state"].as_str().unwrap_or_default(),
                        transfer["placeInQueue"].as_u64(),
                    ),
                );
            }
        }
    }
    let mut progress = Progress {
        total: files.len() as u32,
        ..Default::default()
    };
    for file in files {
        let Some((state, place)) = states.get(file.as_str()) else {
            continue;
        };
        if state.contains("Succeeded") {
            progress.done += 1;
        } else if state.contains("Completed") {
            progress.failed += 1;
            if progress.failure.is_empty() {
                progress.failure = state
                    .split(',')
                    .map(str::trim)
                    .find(|part| *part != "Completed")
                    .unwrap_or("Errored")
                    .to_lowercase();
            }
        } else if state.contains("Queued") || *state == "Requested" {
            progress.queued += 1;
            if let Some(place) = place {
                let place = *place as u32;
                progress.place = Some(progress.place.map_or(place, |known| known.min(place)));
            }
        }
    }
    progress
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    #[default]
    Active,
    Failed,
    /// Every file arrived; the library has not picked the release up yet.
    Downloaded,
}

/// One folder handed to slskd for one missing release.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadJob {
    /// Cache key of the artist page the release was picked from.
    pub artist: String,
    /// MusicBrainz release group.
    pub release: String,
    pub title: String,
    pub username: String,
    pub files: Vec<String>,
    /// How many tracks the release usually has, when MusicBrainz knows.
    pub expected: Option<u32>,
    pub progress: Progress,
    pub state: JobState,
}

#[derive(Clone)]
pub struct Slskd {
    base: Url,
    auth: SlskdAuth,
    /// The session token a username and password were last exchanged for.
    token: Arc<Mutex<Option<String>>>,
    http: reqwest::Client,
    gap: Duration,
    /// Held for a whole search so only one runs, and searches stay spaced apart.
    searching: Arc<Mutex<Option<Instant>>>,
}

impl Slskd {
    pub fn new(url: &str, auth: impl Into<SlskdAuth>) -> Result<Self> {
        Self::with_gap(url, auth, SEARCH_GAP)
    }
    pub fn with_gap(url: &str, auth: impl Into<SlskdAuth>, gap: Duration) -> Result<Self> {
        let mut base =
            Url::parse(url.trim()).context("Enter the complete slskd URL, including http://")?;
        if !matches!(base.scheme(), "https" | "http") || base.host_str().is_none() {
            bail!("Use an http:// or https:// slskd URL");
        }
        if !base.username().is_empty() || base.password().is_some() {
            bail!("Enter the slskd API key in its own field");
        }
        base.set_query(None);
        base.set_fragment(None);
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            base,
            auth: auth.into(),
            token: Arc::default(),
            http: reqwest::Client::builder()
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .build()?,
            gap,
            searching: Arc::default(),
        })
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &[&str],
        body: Option<&Value>,
    ) -> Result<(u16, Value)> {
        let url = self.endpoint(path)?;
        // A session token can expire or be revoked by a restart; sign in again once.
        for renew in [false, true] {
            let mut request = self.http.request(method.clone(), url.clone());
            match &self.auth {
                SlskdAuth::None => {}
                SlskdAuth::Key(key) => request = request.header("X-API-Key", key.expose()),
                SlskdAuth::Login { username, password } => {
                    request = request.bearer_auth(self.session(username, password, renew).await?);
                }
            }
            if let Some(body) = body {
                request = request.json(body);
            }
            // Strip reqwest's URL-bearing errors before returning anything user-visible.
            let response = request.send().await.map_err(|error| {
                anyhow::Error::new(error.without_url()).context("Cannot reach slskd")
            })?;
            let status = response.status().as_u16();
            if status == 401 || status == 403 {
                match &self.auth {
                    SlskdAuth::Login { .. } if !renew => continue,
                    SlskdAuth::Login { .. } => bail!("slskd rejected the username or password"),
                    SlskdAuth::Key(_) => bail!("slskd rejected the API key"),
                    SlskdAuth::None => bail!(
                        "slskd asks for a login. Enter its API key, or its web username and password."
                    ),
                }
            }
            let text = response.text().await.unwrap_or_default();
            return Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)));
        }
        unreachable!("the second attempt always returns")
    }
    fn endpoint(&self, path: &[&str]) -> Result<Url> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Invalid slskd URL"))?
            .pop_if_empty()
            .extend(["api", "v0"])
            // One segment each, so a peer name containing a slash stays one name.
            .extend(path);
        Ok(url)
    }
    async fn session(&self, username: &str, password: &str, renew: bool) -> Result<String> {
        let mut token = self.token.lock().await;
        if let Some(known) = token.as_ref().filter(|_| !renew) {
            return Ok(known.clone());
        }
        let response = self
            .http
            .post(self.endpoint(&["session"])?)
            .json(&json!({"username": username, "password": password}))
            .send()
            .await
            .map_err(|error| {
                anyhow::Error::new(error.without_url()).context("Cannot reach slskd")
            })?;
        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            bail!("slskd rejected the username or password");
        }
        if !(200..300).contains(&status) {
            bail!("slskd returned HTTP {status}");
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("slskd returned an invalid sign-in response"))?;
        let fresh = value["token"]
            .as_str()
            .filter(|token| !token.is_empty())
            .context("slskd returned an invalid sign-in response")?
            .to_string();
        *token = Some(fresh.clone());
        Ok(fresh)
    }
    async fn ok(
        &self,
        method: reqwest::Method,
        path: &[&str],
        body: Option<&Value>,
    ) -> Result<Value> {
        let (status, value) = self.send(method, path, body).await?;
        if !(200..300).contains(&status) {
            bail!("slskd returned HTTP {status}");
        }
        Ok(value)
    }

    /// Confirms the address and key, and reports whether slskd is logged in to Soulseek.
    pub async fn test(&self) -> Result<String> {
        let value = self
            .ok(reqwest::Method::GET, &["application"], None)
            .await?;
        let version = value["version"]["current"]
            .as_str()
            .or_else(|| value["version"]["full"].as_str())
            .unwrap_or_default();
        let state = value["server"]["state"].as_str().unwrap_or_default();
        Ok(if state.is_empty() || state.contains("LoggedIn") {
            format!("Connected to slskd {version}")
        } else {
            format!("Connected to slskd {version}, but it is not logged in to Soulseek")
        }
        .replace("  ", " "))
    }

    /// Runs one search to completion. `update` receives the response count as it grows.
    pub async fn search(
        &self,
        text: &str,
        mut update: impl FnMut(u64),
    ) -> Result<Vec<SearchResponse>> {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        anyhow::ensure!(!text.is_empty(), "Enter something to search for");
        let mut last = self.searching.lock().await;
        if let Some(at) = *last {
            tokio::time::sleep_until(at + self.gap).await;
        }
        let id = format!(
            "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
            rand::random::<u32>(),
            rand::random::<u16>(),
            rand::random::<u16>() & 0xfff,
            rand::random::<u16>() & 0xfff,
            rand::random::<u64>() & 0xffff_ffff_ffff
        );
        let started = self
            .send(
                reqwest::Method::POST,
                &["searches"],
                Some(&json!({"id": id, "searchText": text})),
            )
            .await;
        *last = Some(Instant::now());
        match started? {
            (200..300, _) => {}
            (429, _) => bail!("slskd is busy with another search. Try again in a moment."),
            (status, _) => bail!("slskd returned HTTP {status}"),
        }
        let path = ["searches", id.as_str()];
        let result = async {
            // slskd ends a search 15 s after the last response; this is only a backstop.
            for _ in 0..90 {
                tokio::time::sleep(self.gap.min(Duration::from_secs(1))).await;
                let status = self.ok(reqwest::Method::GET, &path, None).await?;
                update(status["responseCount"].as_u64().unwrap_or(0));
                if status["isComplete"].as_bool().unwrap_or(false) {
                    break;
                }
            }
            let responses = self
                .ok(
                    reqwest::Method::GET,
                    &["searches", id.as_str(), "responses"],
                    None,
                )
                .await?;
            Ok(serde_json::from_value(responses).unwrap_or_default())
        }
        .await;
        let _ = self.send(reqwest::Method::DELETE, &path, None).await;
        *last = Some(Instant::now());
        result
    }

    /// Everything in a peer's folder, including files the search text did not match.
    pub async fn directory(&self, username: &str, directory: &str) -> Result<Vec<RemoteFile>> {
        let value = self
            .ok(
                reqwest::Method::POST,
                &["users", username, "directory"],
                Some(&json!({"directory": directory})),
            )
            .await?;
        let folders = match value {
            Value::Array(folders) => folders,
            folder @ Value::Object(_) => vec![folder],
            _ => Vec::new(),
        };
        let mut files = Vec::new();
        for folder in folders {
            let name = folder["name"].as_str().unwrap_or(directory).to_string();
            for file in folder["files"].as_array().into_iter().flatten() {
                let Ok(mut file) = serde_json::from_value::<RemoteFile>(file.clone()) else {
                    continue;
                };
                // Folder listings carry bare names; transfers need the full path.
                if !file.filename.contains('\\') {
                    file.filename = format!("{name}\\{}", file.filename);
                }
                files.push(file);
            }
        }
        Ok(files)
    }

    pub async fn enqueue(&self, username: &str, files: &[RemoteFile]) -> Result<()> {
        anyhow::ensure!(!files.is_empty(), "This folder has no files to download");
        let list = files
            .iter()
            .map(|file| json!({"filename": file.filename, "size": file.size}))
            .collect::<Vec<_>>();
        let (status, value) = self
            .send(
                reqwest::Method::POST,
                &["transfers", "downloads", "batches"],
                Some(&json!({"username": username, "files": list})),
            )
            .await?;
        // Batches arrived in slskd 0.26; older versions take the plain list.
        let (status, value) = if matches!(status, 400 | 404 | 405 | 415) {
            self.send(
                reqwest::Method::POST,
                &["transfers", "downloads", username],
                Some(&Value::Array(list)),
            )
            .await?
        } else {
            (status, value)
        };
        match status {
            201 | 207 => Ok(()),
            200 if value["failures"].as_array().is_some_and(|f| !f.is_empty()) => {
                bail!("The peer refused every file in this folder")
            }
            200..300 => Ok(()),
            404 => bail!("That peer is offline. Pick another folder."),
            429 => bail!("slskd is busy. Try again in a moment."),
            status => bail!("slskd returned HTTP {status}"),
        }
    }

    pub async fn downloads(&self, username: &str) -> Result<Value> {
        self.ok(
            reqwest::Method::GET,
            &["transfers", "downloads", username],
            None,
        )
        .await
    }
}
