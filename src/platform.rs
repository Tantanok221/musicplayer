use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const UI_FONT: &str = if cfg!(target_os = "macos") {
    ".AppleSystemUIFont"
} else {
    "sans-serif"
};

pub const TITLEBAR_CONTROLS_INSET: f32 = if cfg!(target_os = "linux") { 0. } else { 78. };

pub const fn shortcut(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") {
        mac
    } else {
        other
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub url: String,
    pub username: String,
}
impl Account {
    pub fn namespace(&self) -> String {
        blake3::hash(format!("{}\n{}", self.url, self.username).as_bytes())
            .to_hex()
            .to_string()
    }
}

// This reusable token grants server access. Never derive Debug or log its contents.
#[derive(Clone, Serialize, Deserialize)]
pub struct ServerCredentials {
    pub(crate) salt: String,
    pub(crate) token: String,
}
impl ServerCredentials {
    pub fn from_password(password: &str) -> Result<Self> {
        anyhow::ensure!(!password.is_empty(), "Enter your Navidrome password");
        let salt = format!("{:032x}", rand::random::<u128>());
        let token = format!("{:x}", md5::compute(format!("{password}{salt}")));
        Ok(Self { salt, token })
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.salt.is_empty()
                && self.token.len() == 32
                && self.token.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Saved login is invalid. Sign in again."
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct NativeToken {
    token: String,
}

impl NativeToken {
    pub fn new(token: String) -> Result<Self> {
        anyhow::ensure!(
            !token.is_empty()
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
            "Invalid Navidrome login token. Sign in again."
        );
        Ok(Self { token })
    }

    pub(crate) fn value(&self) -> &str {
        &self.token
    }
}

/// The main window's last frame, kept outside any library so it survives
/// rebuilds and account changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowFrame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
    pub fullscreen: bool,
}

#[derive(Clone)]
pub struct Paths {
    pub data: PathBuf,
    pub cache: PathBuf,
}
impl Paths {
    pub fn new() -> Result<Self> {
        let dirs = ProjectDirs::from("app", "Sonora", "Sonora")
            .context("Cannot locate app directories")?;
        let data = dirs.data_dir().join("library");
        let cache = dirs.cache_dir().join("library");
        std::fs::create_dir_all(&data)?;
        std::fs::create_dir_all(&cache)?;
        Ok(Self { data, cache })
    }
    pub fn account(&self) -> Result<Option<Account>> {
        let path = self.data.join("account.json");
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&std::fs::read(path)?)?))
    }
    /// The saved window frame; a missing or unreadable file means the default.
    pub fn window_frame(&self) -> Option<WindowFrame> {
        let bytes = std::fs::read(self.data.join("window.json")).ok()?;
        serde_json::from_slice::<WindowFrame>(&bytes)
            .ok()
            .filter(|frame| frame.width > 0. && frame.height > 0.)
    }
    pub fn save_window_frame(&self, frame: &WindowFrame) -> Result<()> {
        atomic_write(
            &self.data.join("window.json"),
            &serde_json::to_vec_pretty(frame)?,
        )
    }
    pub fn save_account(&self, account: &Account) -> Result<()> {
        atomic_write(
            &self.data.join("account.json"),
            &serde_json::to_vec_pretty(account)?,
        )
    }

    fn credentials_path(&self, account: &Account) -> PathBuf {
        self.data
            .join("credentials")
            .join(format!("{}.json", account.namespace()))
    }

    fn native_token_path(&self, account: &Account) -> PathBuf {
        self.data
            .join("credentials")
            .join(format!("{}.native.json", account.namespace()))
    }

    pub fn native_token(&self, account: &Account) -> Result<Option<NativeToken>> {
        let bytes = match std::fs::read(self.native_token_path(account)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => anyhow::bail!("Could not read the saved Navidrome login. Sign in again."),
        };
        let token: NativeToken = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid Navidrome login token. Sign in again."))?;
        NativeToken::new(token.token).map(Some)
    }

    pub fn save_native_token(&self, account: &Account, token: &NativeToken) -> Result<()> {
        let save = || -> Result<()> {
            NativeToken::new(token.token.clone())?;
            let directory = self.data.join("credentials");
            #[cfg(unix)]
            {
                use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&directory)?;
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
            }
            #[cfg(not(unix))]
            std::fs::create_dir_all(&directory)?;
            atomic_write(
                &self.native_token_path(account),
                &serde_json::to_vec(token)?,
            )
        };
        save().map_err(|_| anyhow::anyhow!("Could not save the Navidrome login locally"))
    }

    pub fn clear_native_token(&self, account: &Account) -> Result<()> {
        match std::fs::remove_file(self.native_token_path(account)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => anyhow::bail!("Could not clear the saved Navidrome login"),
        }
    }

    pub fn credentials(&self, account: &Account) -> Result<Option<ServerCredentials>> {
        let path = self.credentials_path(account);
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => anyhow::bail!("Could not read saved login. Sign in again."),
        };
        // Serde's detailed errors can quote input; keep credential-file errors generic.
        let credentials: ServerCredentials = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Saved login is invalid. Sign in again."))?;
        credentials.validate()?;
        Ok(Some(credentials))
    }

    pub(crate) fn sync_device_id(&self) -> Result<String> {
        let path = self.data.join("sync-device-id");
        match std::fs::read_to_string(&path) {
            Ok(id) if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) => return Ok(id),
            Ok(_) => anyhow::bail!("Stored sync device ID is invalid"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => anyhow::bail!("Could not read sync device ID"),
        }
        let id = format!("{:032x}", rand::random::<u128>());
        atomic_write(&path, id.as_bytes()).context("Could not save sync device ID")?;
        Ok(id)
    }

    pub(crate) fn sync_signing_key(&self) -> Result<[u8; 32]> {
        let path = self.data.join("sync-signing-key");
        match std::fs::read(&path) {
            Ok(bytes) => {
                return bytes
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("Stored sync signing key is invalid"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => anyhow::bail!("Could not read sync signing key"),
        }
        let key = rand::random::<[u8; 32]>();
        atomic_write(&path, &key).context("Could not save sync signing key")?;
        Ok(key)
    }

    pub fn save_credentials(
        &self,
        account: &Account,
        credentials: &ServerCredentials,
    ) -> Result<()> {
        credentials.validate()?;
        let directory = self.data.join("credentials");
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&directory)?;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(&directory)?;
        atomic_write(
            &self.credentials_path(account),
            &serde_json::to_vec(credentials)?,
        )
        .context("Could not save login locally")
    }

    /// The key or token a history service was given in Settings, if any.
    pub fn history_secret(
        &self,
        provider: crate::history::HistoryProvider,
    ) -> Result<Option<String>> {
        match std::fs::read_to_string(self.data.join("credentials").join(provider.secret_file())) {
            Ok(text) => Ok(Some(text.trim().to_string()).filter(|key| !key.is_empty())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => anyhow::bail!(
                "Could not read the saved {} {}. Enter it again.",
                provider.name(),
                provider.secret_name()
            ),
        }
    }

    /// Saves a history service's key or token; an empty field keeps the saved one.
    pub fn save_history_secret(
        &self,
        provider: crate::history::HistoryProvider,
        secret: &str,
    ) -> Result<()> {
        let secret = secret.trim();
        if secret.is_empty() {
            return Ok(());
        }
        self.credentials_directory()?;
        atomic_write(
            &self.data.join("credentials").join(provider.secret_file()),
            secret.as_bytes(),
        )
        .with_context(|| {
            format!(
                "Could not save the {} {}",
                provider.name(),
                provider.secret_name()
            )
        })
    }

    fn credentials_directory(&self) -> Result<()> {
        let directory = self.data.join("credentials");
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&directory)?;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(&directory)?;
        Ok(())
    }

    fn slskd_key_path(&self) -> PathBuf {
        self.data.join("credentials").join("slskd.key")
    }

    /// The slskd API key or login. It never goes in the catalog, which other
    /// users can read. Nothing saved means slskd is contacted without a login.
    pub fn slskd_auth(&self) -> Result<crate::slskd::SlskdAuth> {
        let text = match std::fs::read_to_string(self.slskd_key_path()) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(crate::slskd::SlskdAuth::None);
            }
            Err(_) => anyhow::bail!("Could not read the saved slskd login. Enter it again."),
        };
        // Parse errors can quote the file; keep them generic.
        crate::slskd::SlskdAuth::from_file(&text)
            .map_err(|_| anyhow::anyhow!("The saved slskd login is invalid. Enter it again."))
    }

    pub fn save_slskd_auth(&self, auth: &crate::slskd::SlskdAuth) -> Result<()> {
        self.credentials_directory()?;
        atomic_write(&self.slskd_key_path(), auth.to_file().as_bytes())
            .context("Could not save the slskd login")
    }
}

pub fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().context("Missing parent directory")?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
