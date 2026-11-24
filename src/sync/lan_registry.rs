use super::lan_security::PublicKey;
use crate::{model::Playlist, platform::atomic_write, server::Server};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use sonora_sync_protocol::Device;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    path::PathBuf,
    sync::Mutex,
    time::Duration,
};
use tokio::{sync::Mutex as AsyncMutex, time::Instant};

pub(crate) const NAME: &str = "Sonora devices (sync keys — do not edit)";
const MARKER: &str =
    "Sonora's device list. This private playlist stores Sonora device-sync keys. Do not edit.";
const PREFIX: &str = "Sonora devices v1:";
const MAX_DEVICES: usize = 32;
pub(crate) const MAX_ADDRESSES: usize = 8;
const MAX_REVOKED: usize = 128;
const MAX_BYTES: usize = 16 * 1024;
const MIN_REFRESH_DELAY: Duration = Duration::from_secs(2);

pub(crate) fn is_device_list(playlist: &Playlist) -> bool {
    playlist.comment.starts_with(MARKER)
        || playlist.name == NAME && playlist.comment.is_empty() && playlist.song_count == 0
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    name: String,
    os: String,
    key: String,
    /// LAN endpoints the device listens on. Older versions omit this and ignore it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    addresses: Vec<SocketAddr>,
}

/// A device trusted through the Navidrome device list, with the endpoints it published.
#[derive(Clone, Debug)]
pub struct Trusted {
    pub key: PublicKey,
    pub device: Device,
    pub addresses: Vec<SocketAddr>,
}

impl Entry {
    fn key(&self) -> Option<PublicKey> {
        let mut key = [0; 32];
        (URL_SAFE_NO_PAD.decode_slice(&self.key, &mut key).ok()? == key.len()).then_some(key)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Roster {
    devices: BTreeMap<String, Entry>,
    revoked: BTreeSet<String>,
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Roster {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.devices.len() <= MAX_DEVICES && self.revoked.len() <= MAX_REVOKED,
            "Sonora device list is full"
        );
        let mut keys = BTreeSet::new();
        ensure!(
            self.revoked.iter().all(|id| valid_id(id))
                && self.devices.iter().all(|(id, entry)| {
                    valid_id(id)
                        && !entry.name.is_empty()
                        && entry.name.len() <= 128
                        && entry.os.len() <= 64
                        && entry.addresses.len() <= MAX_ADDRESSES
                        && entry
                            .addresses
                            .iter()
                            .all(|address| address.port() != 0 && !address.ip().is_unspecified())
                        && entry.key().is_some_and(|key| keys.insert(key))
                }),
            "Invalid Sonora device list"
        );
        Ok(())
    }

    fn parse(comment: &str) -> Result<Self> {
        if comment.is_empty() {
            return Ok(Self::default());
        }
        ensure!(
            comment.len() <= MAX_BYTES,
            "Sonora device list is too large"
        );
        let payload = comment
            .strip_prefix(MARKER)
            .and_then(|body| body.strip_prefix('\n'))
            .and_then(|body| body.strip_prefix(PREFIX))
            .context("Unsupported Sonora device list format")?;
        let roster: Self = serde_json::from_str(payload)?;
        roster.validate()?;
        Ok(roster)
    }

    fn comment(&self) -> Result<String> {
        self.validate()?;
        let payload = serde_json::to_string(self)?;
        let comment = format!("{MARKER}\n{PREFIX}{payload}");
        ensure!(
            comment.len() <= MAX_BYTES,
            "Sonora device list is too large"
        );
        Ok(comment)
    }

    fn merge(&mut self, other: Self) -> Result<()> {
        self.revoked.extend(other.revoked);
        for (id, entry) in other.devices {
            if self.revoked.contains(&id) {
                continue;
            }
            if let Some(current) = self.devices.get(&id) {
                ensure!(current.key == entry.key, "Conflicting Sonora device keys");
            } else {
                self.devices.insert(id, entry);
            }
        }
        self.devices.retain(|id, _| !self.revoked.contains(id));
        self.validate()
    }
}

pub(crate) struct LanRegistry {
    server: Server,
    local_id: String,
    local_entry: Mutex<Entry>,
    path: PathBuf,
    roster: Mutex<Roster>,
    last_refresh: AsyncMutex<Option<Instant>>,
}

impl LanRegistry {
    pub fn open(
        server: Server,
        device: &Device,
        identity: &[u8; 32],
        path: PathBuf,
    ) -> Result<Self> {
        let roster = match std::fs::read_to_string(&path) {
            Ok(cache) => Roster::parse(&cache)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Roster::default(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            server,
            local_id: device.id.clone(),
            local_entry: Mutex::new(Entry {
                name: device.name.clone(),
                os: device.os.clone(),
                key: URL_SAFE_NO_PAD
                    .encode(SigningKey::from_bytes(identity).verifying_key().as_bytes()),
                addresses: Vec::new(),
            }),
            path,
            roster: Mutex::new(roster),
            last_refresh: AsyncMutex::new(None),
        })
    }

    /// Replaces the LAN endpoints this device publishes on the next refresh.
    pub fn set_addresses(&self, mut addresses: Vec<SocketAddr>) {
        addresses.sort_unstable();
        addresses.dedup();
        addresses.truncate(MAX_ADDRESSES);
        if let Ok(mut entry) = self.local_entry.lock() {
            entry.addresses = addresses;
        }
    }

    pub fn devices(&self) -> Vec<Trusted> {
        let Ok(roster) = self.roster.lock() else {
            return Vec::new();
        };
        roster
            .devices
            .iter()
            .filter_map(|(id, entry)| {
                if id == &self.local_id || roster.revoked.contains(id) {
                    return None;
                }
                Some(Trusted {
                    key: entry.key()?,
                    device: Device {
                        id: id.clone(),
                        name: entry.name.clone(),
                        os: entry.os.clone(),
                        online: false,
                        last_seen: 0,
                    },
                    addresses: entry.addresses.clone(),
                })
            })
            .collect()
    }

    pub fn revoked(&self, id: &str) -> bool {
        self.roster
            .lock()
            .map_or(true, |roster| roster.revoked.contains(id))
    }

    pub fn revoke(&self, id: &str) -> Result<()> {
        if !valid_id(id) || id == self.local_id {
            return Ok(());
        }
        let mut roster = self
            .roster
            .lock()
            .map_err(|_| anyhow::anyhow!("LAN trust unavailable"))?;
        let mut updated = roster.clone();
        updated.revoked.insert(id.to_owned());
        updated.devices.remove(id);
        atomic_write(&self.path, updated.comment()?.as_bytes())?;
        *roster = updated;
        Ok(())
    }

    pub async fn refresh(&self) -> Result<bool> {
        let mut last = self.last_refresh.lock().await;
        if last.is_some_and(|last| last.elapsed() < MIN_REFRESH_DELAY) {
            return Ok(false);
        }
        *last = Some(Instant::now());
        let (changed, duplicates) =
            tokio::time::timeout(Duration::from_secs(4), self.reconcile()).await??;
        let _ = tokio::time::timeout(Duration::from_millis(500), async {
            for id in duplicates {
                let _ = self.server.delete_device_list(&id).await;
            }
        })
        .await;
        Ok(changed)
    }

    async fn read(&self) -> Result<(Vec<Playlist>, Roster)> {
        let mut candidates = self.server.device_lists().await?;
        candidates.retain(|list| {
            is_device_list(list)
                && !list.public
                && list.song_count == 0
                && list.owner == self.server.account.username
        });
        candidates.sort_by(|left, right| left.id.cmp(&right.id));
        let mut roster = Roster::default();
        for list in &candidates {
            roster.merge(Roster::parse(&list.comment)?)?;
        }
        Ok((candidates, roster))
    }

    async fn reconcile(&self) -> Result<(bool, Vec<String>)> {
        let (mut lists, mut roster) = self.read().await?;
        let mut observed = Roster::default();
        for _ in 0..3 {
            observed.merge(roster)?;
            let desired = {
                let current = self
                    .roster
                    .lock()
                    .map_err(|_| anyhow::anyhow!("LAN trust unavailable"))?;
                let mut desired = observed.clone();
                desired.revoked.extend(current.revoked.iter().cloned());
                desired
                    .devices
                    .retain(|id, _| !desired.revoked.contains(id));
                if !desired.revoked.contains(&self.local_id) {
                    let local = self
                        .local_entry
                        .lock()
                        .map_err(|_| anyhow::anyhow!("LAN trust unavailable"))?
                        .clone();
                    desired.devices.insert(self.local_id.clone(), local);
                }
                desired
            };
            let comment = desired.comment()?;
            if let Some((target, duplicates)) = lists.split_first() {
                if target.comment == comment && target.name == NAME {
                    return Ok((
                        self.store(desired)?,
                        duplicates.iter().map(|list| list.id.clone()).collect(),
                    ));
                }
                self.server.update_device_list(&target.id, &comment).await?;
            } else {
                self.server.create_device_list().await?;
            }
            (lists, roster) = self.read().await?;
        }
        observed.merge(roster)?;
        Ok((self.store(observed)?, Vec::new()))
    }

    fn store(&self, mut updated: Roster) -> Result<bool> {
        let mut current = self
            .roster
            .lock()
            .map_err(|_| anyhow::anyhow!("LAN trust unavailable"))?;
        updated.revoked.extend(current.revoked.iter().cloned());
        updated
            .devices
            .retain(|id, _| !updated.revoked.contains(id));
        if *current == updated {
            return Ok(false);
        }
        atomic_write(&self.path, updated.comment()?.as_bytes())?;
        *current = updated;
        Ok(true)
    }
}
