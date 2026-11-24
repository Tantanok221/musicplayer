//! Self-update for Linux builds from signed GitHub releases.
//!
//! Every commit on main that passes CI is released as a Linux binary with a
//! manifest naming its commit and SHA-256, signed with the release key. A build
//! installs a release only when the signature and checksum match and GitHub
//! says the release's commit is ahead of its own, so it never moves backwards.
use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use url::Url;

/// The commit this build was made from, or empty when it isn't known.
pub const COMMIT: &str = env!("SONORA_COMMIT");
/// Verifies release manifests. The private half is the release workflow's secret.
const RELEASE_KEY: [u8; 32] =
    hex32("a6c3b6e15f8aa01ea6fc8099a3e3c84c150acfd8218eaa485a0b9ebba8ebfc53");
const RELEASES: &str = "https://api.github.com/repos/Tantanok221/musicplayer/";
pub const TARGET: &str = "linux-x86_64";
const MANIFEST_HEADER: &str = "sonora-update-v1";
const MAX_BINARY: u64 = 256 * 1024 * 1024;
const MAX_SMALL: u64 = 64 * 1024;

/// The first characters of a commit, as shown to people.
pub fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    /// This build can't update itself (not Linux, or its commit is unknown).
    Unavailable,
    /// Waiting for the next automatic check.
    Idle,
    Checking,
    UpToDate,
    /// The release with this commit is installed and runs after a restart.
    Ready(String),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateStatus {
    pub current: String,
    pub state: UpdateState,
}

impl UpdateStatus {
    pub fn ready(&self) -> Option<&str> {
        match &self.state {
            UpdateState::Ready(commit) => Some(commit),
            _ => None,
        }
    }

    pub fn label(&self) -> String {
        let version = if self.current.is_empty() {
            "Version unknown".to_owned()
        } else {
            format!("Version {}", short(&self.current))
        };
        let state = match &self.state {
            UpdateState::Unavailable => "Updates aren't available for this build".to_owned(),
            UpdateState::Idle => "Updates install automatically".to_owned(),
            UpdateState::Checking => "Checking for updates…".to_owned(),
            UpdateState::UpToDate => "Up to date".to_owned(),
            UpdateState::Ready(commit) => {
                format!("Update {} is ready; restart to use it", short(commit))
            }
            UpdateState::Failed(error) => format!("Couldn't update: {error}"),
        };
        format!("{version} · {state}")
    }
}

/// What a signed manifest says about one release.
#[derive(Debug, PartialEq, Eq)]
pub struct Manifest {
    pub commit: String,
    pub sha256: [u8; 32],
}

/// Checks the signature first, then reads the manifest. Only `TARGET` is accepted.
pub fn verify_manifest(key: &VerifyingKey, text: &[u8], signature: &[u8]) -> Result<Manifest> {
    let signature = Signature::from_slice(signature).context("Malformed update signature")?;
    key.verify_strict(text, &signature)
        .context("The update isn't signed by the Sonora release key")?;
    let text = std::str::from_utf8(text).context("Malformed update manifest")?;
    let mut lines = text.lines();
    ensure!(
        lines.next() == Some(MANIFEST_HEADER),
        "Unsupported update manifest"
    );
    let (mut target, mut commit, mut sha256) = (None, None, None);
    for line in lines {
        let (field, value) = line.split_once('=').context("Malformed update manifest")?;
        let slot = match field {
            "target" => &mut target,
            "commit" => &mut commit,
            "sha256" => &mut sha256,
            _ => continue,
        };
        ensure!(slot.is_none(), "Malformed update manifest");
        *slot = Some(value);
    }
    ensure!(target == Some(TARGET), "The update is for another platform");
    let commit = commit
        .filter(|commit| is_commit(commit))
        .context("Malformed update manifest")?;
    let sha256 = sha256
        .and_then(decode_hex::<32>)
        .context("Malformed update manifest")?;
    Ok(Manifest {
        commit: commit.to_owned(),
        sha256,
    })
}

fn is_commit(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn decode_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != N * 2 {
        return None;
    }
    let mut out = [0; N];
    for (index, pair) in text.as_bytes().chunks(2).enumerate() {
        out[index] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

const fn hex32(text: &str) -> [u8; 32] {
    const fn digit(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => panic!("invalid hex"),
        }
    }
    let bytes = text.as_bytes();
    assert!(bytes.len() == 64);
    let mut out = [0; 32];
    let mut index = 0;
    while index < 32 {
        out[index] = digit(bytes[index * 2]) << 4 | digit(bytes[index * 2 + 1]);
        index += 1;
    }
    out
}

/// What a check did.
#[derive(Debug, PartialEq, Eq)]
pub enum Checked {
    UpToDate,
    /// The release with this commit replaced the executable.
    Installed(String),
}

type Relaunch = Arc<dyn Fn(&Path) -> Result<()> + Send + Sync>;

pub struct Updater {
    client: reqwest::Client,
    api: Url,
    current: String,
    executable: PathBuf,
    key: VerifyingKey,
    relaunch: Relaunch,
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct Comparison {
    status: String,
}

impl Updater {
    /// The updater for this build, when it can replace itself: a Linux x86-64
    /// build that knows its commit and wasn't run from a Cargo target folder.
    pub fn github() -> Option<Self> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) || !is_commit(COMMIT) {
            return None;
        }
        let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
        if executable
            .components()
            .any(|part| part.as_os_str() == "target")
        {
            return None;
        }
        let key = VerifyingKey::from_bytes(&RELEASE_KEY).ok()?;
        Self::new(RELEASES, COMMIT, executable, key)
            .ok()
            .map(|updater| updater.with_relaunch(Arc::new(relaunch_after_exit)))
    }

    /// An updater using `api` as the GitHub repository API root.
    pub fn new(api: &str, current: &str, executable: PathBuf, key: VerifyingKey) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("Sonora-updater"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        Ok(Self {
            client: reqwest::Client::builder()
                .default_headers(headers)
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(600))
                .build()?,
            api: Url::parse(api)?,
            current: current.to_owned(),
            executable,
            key,
            relaunch: Arc::new(|_| bail!("Restarting isn't available")),
        })
    }

    pub fn with_relaunch(mut self, relaunch: Relaunch) -> Self {
        self.relaunch = relaunch;
        self
    }

    pub fn current(&self) -> &str {
        &self.current
    }

    /// Starts the installed executable once this process exits.
    pub fn relaunch(&self) -> Result<()> {
        (self.relaunch)(&self.executable)
    }

    /// Installs the latest release when it is newer than this build and than
    /// `installed`, the release this process already installed.
    pub async fn check(&self, installed: Option<&str>) -> Result<Checked> {
        let release: Release = self.json(self.api.join("releases/latest")?).await?;
        let asset = |name: &str| {
            release
                .assets
                .iter()
                .find(|asset| asset.name == name)
                .with_context(|| format!("The latest release has no {name}"))
                .and_then(|asset| Ok(Url::parse(&asset.browser_download_url)?))
        };
        let binary = format!("sonora-{TARGET}");
        let manifest_text = self
            .download(asset(&format!("{binary}.manifest"))?, MAX_SMALL)
            .await?;
        let signature = self
            .download(asset(&format!("{binary}.manifest.sig"))?, MAX_SMALL)
            .await?;
        let manifest = verify_manifest(&self.key, &manifest_text, &signature)?;
        if manifest.commit == self.current || Some(manifest.commit.as_str()) == installed {
            return Ok(Checked::UpToDate);
        }
        if !self.is_ahead(&manifest.commit).await? {
            return Ok(Checked::UpToDate);
        }
        let bytes = self.download(asset(&binary)?, MAX_BINARY).await?;
        ensure!(
            Sha256::digest(&bytes).as_slice() == manifest.sha256,
            "The downloaded update doesn't match its checksum"
        );
        install(&self.executable, &bytes)?;
        Ok(Checked::Installed(manifest.commit))
    }

    /// Whether `commit` comes after this build's commit on GitHub.
    async fn is_ahead(&self, commit: &str) -> Result<bool> {
        let url = self
            .api
            .join(&format!("compare/{}...{commit}", self.current))?;
        let response = self.client.get(url).send().await?;
        // GitHub doesn't know a commit that was built locally and never pushed.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        let comparison: Comparison = response.error_for_status()?.json().await?;
        Ok(comparison.status == "ahead")
    }

    async fn json<T: serde::de::DeserializeOwned>(&self, url: Url) -> Result<T> {
        let bytes = self.download(url, MAX_SMALL * 16).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn download(&self, url: Url, limit: u64) -> Result<Vec<u8>> {
        let response = self.client.get(url).send().await?.error_for_status()?;
        ensure!(
            response
                .content_length()
                .is_none_or(|length| length <= limit),
            "The update is too large"
        );
        let mut bytes = Vec::new();
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            bytes.extend_from_slice(&chunk?);
            ensure!(bytes.len() as u64 <= limit, "The update is too large");
        }
        Ok(bytes)
    }
}

/// Replaces `executable` with `bytes`. The running process keeps its open copy;
/// a failure leaves the installed file unchanged.
fn install(executable: &Path, bytes: &[u8]) -> Result<()> {
    let dir = executable
        .parent()
        .context("The installed executable has no folder")?;
    let mut staged = tempfile::Builder::new()
        .prefix(".sonora-update.")
        .tempfile_in(dir)
        .context("Couldn't write next to the installed executable")?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    staged
        .persist(executable)
        .context("Couldn't replace the installed executable")?;
    Ok(())
}

/// Starts `executable` after this process has exited, so the new copy doesn't
/// compete with this one for the library database or the LAN port.
fn relaunch_after_exit(executable: &Path) -> Result<()> {
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec "$2""#)
        .arg("sonora-relaunch")
        .arg(std::process::id().to_string())
        .arg(executable)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Couldn't restart Sonora")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signed with OpenSSL (`pkeyutl -sign -rawin`), as the release workflow does.
    const VECTOR_KEY: [u8; 32] =
        hex32("b7fc59b2bc2cc961b62e49e2bcb32999a49d14d46bfe381d10b19e54f65b5fab");
    const VECTOR_SIGNATURE: &str = "17ffa58ff42b62bc9e8e6eba6602e28cdf900f1643c6747b5ba42673b24d6ac7fd0ea9831b18fecfc34b398d059587d015e476d4a0a71210b6fae0fdaabd5604";
    const VECTOR: &str = "sonora-update-v1\ntarget=linux-x86_64\ncommit=1111111111111111111111111111111111111111\nsha256=2222222222222222222222222222222222222222222222222222222222222222\n";

    fn vector_key() -> VerifyingKey {
        VerifyingKey::from_bytes(&VECTOR_KEY).unwrap()
    }

    fn signature() -> Vec<u8> {
        decode_hex::<64>(VECTOR_SIGNATURE).unwrap().to_vec()
    }

    #[test]
    fn accepts_a_manifest_signed_by_the_release_tooling() {
        let manifest = verify_manifest(&vector_key(), VECTOR.as_bytes(), &signature()).unwrap();
        assert_eq!(manifest.commit, "1".repeat(40));
        assert_eq!(manifest.sha256, [0x22; 32]);
        assert!(VerifyingKey::from_bytes(&RELEASE_KEY).is_ok());
    }

    #[test]
    fn rejects_changed_or_unsigned_manifests() {
        let changed = VECTOR.replace("commit=1", "commit=3");
        assert!(verify_manifest(&vector_key(), changed.as_bytes(), &signature()).is_err());
        let release = VerifyingKey::from_bytes(&RELEASE_KEY).unwrap();
        assert!(verify_manifest(&release, VECTOR.as_bytes(), &signature()).is_err());
        assert!(verify_manifest(&vector_key(), VECTOR.as_bytes(), &[0; 10]).is_err());
    }

    #[test]
    fn labels_say_what_happened() {
        let status = |state| UpdateStatus {
            current: "a".repeat(40),
            state,
        };
        assert_eq!(
            status(UpdateState::UpToDate).label(),
            "Version aaaaaaa · Up to date"
        );
        assert_eq!(
            status(UpdateState::Ready("b".repeat(40))).label(),
            "Version aaaaaaa · Update bbbbbbb is ready; restart to use it"
        );
        assert_eq!(
            status(UpdateState::Ready("b".repeat(40))).ready(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(
            UpdateStatus {
                current: String::new(),
                state: UpdateState::Unavailable
            }
            .label(),
            "Version unknown · Updates aren't available for this build"
        );
    }
}
