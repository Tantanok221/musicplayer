//! Self-update from signed GitHub releases, for Linux x86-64 and Apple Silicon.
//!
//! Every commit on main that passes CI is released as a Linux binary and a
//! zipped macOS app, each with a manifest naming its target, commit and
//! SHA-256, signed with the release key. A build installs a release only when
//! the signature and checksum match and GitHub says the release's commit is
//! ahead of its own, so it never moves backwards.
//!
//! macOS doesn't assess files Sonora downloads itself (they carry no
//! quarantine flag), so an ad-hoc signed app can replace itself without a
//! Developer ID; the release key provides the authenticity.
mod bundle;

use anyhow::{Context, Result, bail, ensure};
pub use bundle::{BundleTools, swap_dirs};
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
/// The release target this build installs, when it has one.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const TARGET: Option<Target> = Some(Target::LinuxX86_64);
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const TARGET: Option<Target> = Some(Target::MacosAarch64);
#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
pub const TARGET: Option<Target> = None;
const MANIFEST_HEADER: &str = "sonora-update-v1";
const MAX_BINARY: u64 = 256 * 1024 * 1024;
const MAX_SMALL: u64 = 64 * 1024;

/// A platform releases are built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    LinuxX86_64,
    MacosAarch64,
}

impl Target {
    /// The name in manifests and asset names.
    pub fn name(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "linux-x86_64",
            Self::MacosAarch64 => "macos-aarch64",
        }
    }

    /// The release asset the manifest's checksum covers: the Linux binary, or
    /// the zipped macOS app.
    pub fn asset(self) -> String {
        match self {
            Self::LinuxX86_64 => format!("sonora-{}", self.name()),
            Self::MacosAarch64 => format!("sonora-{}.zip", self.name()),
        }
    }

    pub fn manifest(self) -> String {
        format!("sonora-{}.manifest", self.name())
    }

    pub fn signature(self) -> String {
        format!("{}.sig", self.manifest())
    }
}

/// The first characters of a commit, as shown to people.
pub fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    /// This build can't update itself, with the reason to show when it is
    /// one the person can fix.
    Unavailable(Option<String>),
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
            UpdateState::Unavailable(Some(reason)) => reason.clone(),
            UpdateState::Unavailable(None) => "Updates aren't available for this build".to_owned(),
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

/// Checks the signature first, then reads the manifest. Only `target` is accepted.
pub fn verify_manifest(
    key: &VerifyingKey,
    target: Target,
    text: &[u8],
    signature: &[u8],
) -> Result<Manifest> {
    let signature = Signature::from_slice(signature).context("Malformed update signature")?;
    key.verify_strict(text, &signature)
        .context("The update isn't signed by the Sonora release key")?;
    let text = std::str::from_utf8(text).context("Malformed update manifest")?;
    let mut lines = text.lines();
    ensure!(
        lines.next() == Some(MANIFEST_HEADER),
        "Unsupported update manifest"
    );
    let (mut manifest_target, mut commit, mut sha256) = (None, None, None);
    for line in lines {
        let (field, value) = line.split_once('=').context("Malformed update manifest")?;
        let slot = match field {
            "target" => &mut manifest_target,
            "commit" => &mut commit,
            "sha256" => &mut sha256,
            _ => continue,
        };
        ensure!(slot.is_none(), "Malformed update manifest");
        *slot = Some(value);
    }
    ensure!(
        manifest_target == Some(target.name()),
        "The update is for another platform"
    );
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
    /// The release with this commit replaced the installed copy.
    Installed(String),
}

type Relaunch = Arc<dyn Fn(&Path) -> Result<()> + Send + Sync>;

/// What an update replaces.
enum Installed {
    /// A Linux executable, replaced in place.
    Binary(PathBuf),
    /// A macOS app bundle, replaced as a whole.
    Bundle(PathBuf, BundleTools),
}

impl Installed {
    fn path(&self) -> &Path {
        match self {
            Self::Binary(path) | Self::Bundle(path, _) => path,
        }
    }
}

pub struct Updater {
    client: reqwest::Client,
    api: Url,
    current: String,
    target: Target,
    installed: Installed,
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
    /// The updater for this build, when it can replace itself: a release
    /// target build that knows its commit and wasn't run from a Cargo target
    /// folder; on macOS, also an app bundle in a folder this user can write.
    /// `Err` carries the reason to show, when the person can fix it.
    pub fn github() -> Result<Self, Option<String>> {
        let target = TARGET.ok_or(None)?;
        if !is_commit(COMMIT) {
            return Err(None);
        }
        let executable = std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .map_err(|_| None)?;
        if built_locally(&executable) {
            return Err(None);
        }
        let key = VerifyingKey::from_bytes(&RELEASE_KEY).map_err(|_| None)?;
        match target {
            Target::LinuxX86_64 => Self::new(RELEASES, COMMIT, executable, key)
                .map(|updater| {
                    updater.with_relaunch(Arc::new(|path| relaunch_after_exit(path, RUN_BINARY)))
                })
                .map_err(|_| None),
            #[cfg(target_os = "macos")]
            Target::MacosAarch64 => {
                let bundle = bundle::locate(&executable)?;
                Self::bundle(RELEASES, COMMIT, bundle, key, BundleTools::macos())
                    .map(|updater| {
                        updater.with_relaunch(Arc::new(|path| relaunch_after_exit(path, OPEN_APP)))
                    })
                    .map_err(|_| None)
            }
            #[cfg(not(target_os = "macos"))]
            Target::MacosAarch64 => Err(None),
        }
    }

    /// An updater for the Linux executable at `executable`, using `api` as the
    /// GitHub repository API root.
    pub fn new(api: &str, current: &str, executable: PathBuf, key: VerifyingKey) -> Result<Self> {
        Self::with_installed(
            api,
            current,
            Target::LinuxX86_64,
            Installed::Binary(executable),
            key,
        )
    }

    /// An updater for the macOS app at `bundle`, using `tools` for the
    /// platform steps of the install.
    pub fn bundle(
        api: &str,
        current: &str,
        bundle: PathBuf,
        key: VerifyingKey,
        tools: BundleTools,
    ) -> Result<Self> {
        Self::with_installed(
            api,
            current,
            Target::MacosAarch64,
            Installed::Bundle(bundle, tools),
            key,
        )
    }

    fn with_installed(
        api: &str,
        current: &str,
        target: Target,
        installed: Installed,
        key: VerifyingKey,
    ) -> Result<Self> {
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
            target,
            installed,
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

    /// Starts the installed copy once this process exits.
    pub fn relaunch(&self) -> Result<()> {
        (self.relaunch)(self.installed.path())
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
        let manifest_text = self
            .download(asset(&self.target.manifest())?, MAX_SMALL)
            .await?;
        let signature = self
            .download(asset(&self.target.signature())?, MAX_SMALL)
            .await?;
        let manifest = verify_manifest(&self.key, self.target, &manifest_text, &signature)?;
        if manifest.commit == self.current || Some(manifest.commit.as_str()) == installed {
            return Ok(Checked::UpToDate);
        }
        if !self.is_ahead(&manifest.commit).await? {
            return Ok(Checked::UpToDate);
        }
        let bytes = self
            .download(asset(&self.target.asset())?, MAX_BINARY)
            .await?;
        ensure!(
            Sha256::digest(&bytes).as_slice() == manifest.sha256,
            "The downloaded update doesn't match its checksum"
        );
        match &self.installed {
            Installed::Binary(executable) => install(executable, &bytes)?,
            Installed::Bundle(bundle, tools) => {
                // Unpacking and checking the signature take a few seconds.
                let (bundle, tools) = (bundle.clone(), tools.clone());
                tokio::task::spawn_blocking(move || bundle::install(&bundle, &bytes, &tools))
                    .await??;
            }
        }
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

/// Whether this executable was built in a Cargo target folder rather than
/// installed; such builds update from source instead.
fn built_locally(executable: &Path) -> bool {
    executable
        .components()
        .any(|part| part.as_os_str() == "target")
}

/// Shell commands that start the installed copy, passed as `$2`.
const RUN_BINARY: &str = r#"exec "$2""#;
#[cfg(target_os = "macos")]
const OPEN_APP: &str = r#"exec /usr/bin/open -n "$2""#;

/// Runs `start` with `path` after this process has exited, so the new copy
/// doesn't compete with this one for the library database or the LAN port.
fn relaunch_after_exit(path: &Path, start: &str) -> Result<()> {
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; {start}"#
        ))
        .arg("sonora-relaunch")
        .arg(std::process::id().to_string())
        .arg(path)
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
        let manifest = verify_manifest(
            &vector_key(),
            Target::LinuxX86_64,
            VECTOR.as_bytes(),
            &signature(),
        )
        .unwrap();
        assert_eq!(manifest.commit, "1".repeat(40));
        assert_eq!(manifest.sha256, [0x22; 32]);
        assert!(VerifyingKey::from_bytes(&RELEASE_KEY).is_ok());
    }

    #[test]
    fn rejects_changed_or_unsigned_manifests() {
        let changed = VECTOR.replace("commit=1", "commit=3");
        let linux = Target::LinuxX86_64;
        assert!(verify_manifest(&vector_key(), linux, changed.as_bytes(), &signature()).is_err());
        let release = VerifyingKey::from_bytes(&RELEASE_KEY).unwrap();
        assert!(verify_manifest(&release, linux, VECTOR.as_bytes(), &signature()).is_err());
        assert!(verify_manifest(&vector_key(), linux, VECTOR.as_bytes(), &[0; 10]).is_err());
    }

    #[test]
    fn a_manifest_only_installs_on_its_own_target() {
        // The vector is a correctly signed Linux manifest.
        let error = verify_manifest(
            &vector_key(),
            Target::MacosAarch64,
            VECTOR.as_bytes(),
            &signature(),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "The update is for another platform");

        let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let mac = VECTOR.replace("linux-x86_64", "macos-aarch64");
        let signed = ed25519_dalek::Signer::sign(&key, mac.as_bytes()).to_bytes();
        let key = key.verifying_key();
        assert!(verify_manifest(&key, Target::MacosAarch64, mac.as_bytes(), &signed).is_ok());
        let error =
            verify_manifest(&key, Target::LinuxX86_64, mac.as_bytes(), &signed).unwrap_err();
        assert_eq!(error.to_string(), "The update is for another platform");
    }

    #[test]
    fn asset_names_come_from_the_target() {
        assert_eq!(Target::LinuxX86_64.asset(), "sonora-linux-x86_64");
        assert_eq!(
            Target::LinuxX86_64.signature(),
            "sonora-linux-x86_64.manifest.sig"
        );
        assert_eq!(Target::MacosAarch64.asset(), "sonora-macos-aarch64.zip");
        assert_eq!(
            Target::MacosAarch64.manifest(),
            "sonora-macos-aarch64.manifest"
        );
    }

    #[test]
    fn builds_from_a_cargo_target_folder_update_from_source() {
        assert!(built_locally(Path::new(
            "/src/sonora/target/release/sonora"
        )));
        assert!(!built_locally(Path::new("/home/me/.local/bin/sonora")));
        assert!(!built_locally(Path::new(
            "/Applications/Sonora.app/Contents/MacOS/sonora"
        )));
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
                state: UpdateState::Unavailable(None)
            }
            .label(),
            "Version unknown · Updates aren't available for this build"
        );
        assert_eq!(
            status(UpdateState::Unavailable(Some(bundle::TRANSLOCATED.into()))).label(),
            "Version aaaaaaa · Move Sonora to Applications to enable updates"
        );
    }
}
