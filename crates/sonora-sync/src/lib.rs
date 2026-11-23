mod auth;
mod config;
mod local;
mod session;
mod store;
mod ws;

use axum::{Router, routing::get};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicU64},
};
use url::Url;

pub use config::{Config, ConfigError};
pub use local::{LocalCoordinator, LocalPeer, SessionError};
pub use store::StoreError;

#[derive(thiserror::Error, Debug)]
pub enum StartupError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("Could not initialize Navidrome HTTP client")]
    Http(#[from] reqwest::Error),
}

pub fn router(navidrome_url: Url, db_path: &Path) -> Result<Router, StartupError> {
    let relay = ws::Relay {
        auth: auth::NavidromeAuthenticator::new(config::ping_url(navidrome_url)?)?,
        hub: session::SessionHub::new(store::Store::open(db_path)?),
        generations: Arc::new(AtomicU64::new(0)),
    };
    Ok(Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(ws::upgrade))
        .with_state(relay))
}
