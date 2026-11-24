use reqwest::{Client, redirect::Policy};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sonora_sync_protocol::Hello;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};
use url::Url;

const AUTH_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_CACHE_ENTRIES: usize = 4_096;

#[derive(Clone)]
pub(crate) struct NavidromeAuthenticator {
    client: Client,
    ping: Url,
    cached: Arc<Mutex<HashMap<[u8; 32], Instant>>>,
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum AuthError {
    #[error("Navidrome rejected the login")]
    Rejected,
    #[error("Could not validate login with Navidrome")]
    Unavailable,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "subsonic-response")]
    response: PingResponse,
}

#[derive(Deserialize)]
struct PingResponse {
    status: String,
}

impl NavidromeAuthenticator {
    pub fn new(ping: Url) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            client,
            ping,
            cached: Arc::default(),
        })
    }

    pub async fn check(&self, hello: &Hello) -> Result<(), AuthError> {
        let mut hash = Sha256::new();
        for field in [&hello.username, &hello.subsonic_token, &hello.subsonic_salt] {
            hash.update(field.len().to_le_bytes());
            hash.update(field.as_bytes());
        }
        let key = hash.finalize().into();
        if self
            .cached
            .lock()
            .await
            .get(&key)
            .is_some_and(|until| *until > Instant::now())
        {
            return Ok(());
        }
        let response = self
            .client
            .get(self.ping.clone())
            .query(&[
                ("u", hello.username.as_str()),
                ("t", hello.subsonic_token.as_str()),
                ("s", hello.subsonic_salt.as_str()),
                ("v", "1.16.1"),
                ("c", "SonoraSync"),
                ("f", "json"),
            ])
            .send()
            .await
            .map_err(|_| AuthError::Unavailable)?;
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(AuthError::Rejected);
        }
        if !response.status().is_success() {
            return Err(AuthError::Unavailable);
        }
        let envelope = response
            .json::<Envelope>()
            .await
            .map_err(|_| AuthError::Unavailable)?;
        if envelope.response.status != "ok" {
            return Err(AuthError::Rejected);
        }
        let now = Instant::now();
        let mut cache = self.cached.lock().await;
        cache.retain(|_, until| *until > now);
        if cache.len() < MAX_CACHE_ENTRIES {
            cache.insert(key, now + AUTH_TTL);
        }
        Ok(())
    }
}
