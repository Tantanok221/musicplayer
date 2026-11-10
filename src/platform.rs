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
