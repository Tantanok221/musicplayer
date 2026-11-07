use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
}

pub fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().context("Missing parent directory")?)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn save_password(account: &Account, password: &str) -> Result<()> {
    security_framework::passwords::set_generic_password(
        "app.sonora.navidrome",
        &account.namespace(),
        password.as_bytes(),
    )
    .context("Could not save password in macOS Keychain")
}
#[cfg(target_os = "macos")]
pub fn password(account: &Account) -> Result<String> {
    let bytes = security_framework::passwords::get_generic_password(
        "app.sonora.navidrome",
        &account.namespace(),
    )
    .context("Could not read password from macOS Keychain. Sign in again.")?;
    Ok(String::from_utf8(bytes)?)
}
#[cfg(not(target_os = "macos"))]
pub fn save_password(_: &Account, _: &str) -> Result<()> {
    anyhow::bail!("Keychain requires macOS")
}
#[cfg(not(target_os = "macos"))]
pub fn password(_: &Account) -> Result<String> {
    anyhow::bail!("Keychain requires macOS")
}
