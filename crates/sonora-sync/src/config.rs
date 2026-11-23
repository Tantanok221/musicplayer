use std::{env, net::SocketAddr, path::PathBuf};
use url::Url;

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("NAVIDROME_URL must be an HTTP(S) URL without embedded credentials, query or fragment")]
    NavidromeUrl,
    #[error("BIND_ADDR must be a socket address")]
    BindAddr,
}

pub struct Config {
    pub navidrome_url: Url,
    pub bind_addr: SocketAddr,
    pub db_path: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let navidrome_url = env::var("NAVIDROME_URL").map_err(|_| ConfigError::NavidromeUrl)?;
        let navidrome_url = Url::parse(&navidrome_url).map_err(|_| ConfigError::NavidromeUrl)?;
        let bind_addr = env::var("BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8088".into())
            .parse()
            .map_err(|_| ConfigError::BindAddr)?;
        let db_path =
            env::var_os("DB_PATH").map_or_else(|| "sonora-sync.sqlite".into(), PathBuf::from);
        Ok(Self {
            navidrome_url,
            bind_addr,
            db_path,
        })
    }
}

pub(crate) fn ping_url(mut url: Url) -> Result<Url, ConfigError> {
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::NavidromeUrl);
    }
    url.path_segments_mut()
        .map_err(|_| ConfigError::NavidromeUrl)?
        .pop_if_empty()
        .extend(["rest", "ping.view"]);
    Ok(url)
}
