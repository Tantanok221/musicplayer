use crate::store::{Store, StoreError};
use sonora_sync_protocol::{ClientMessage, Device, Hello, PlaybackState, ServerMessage};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

pub(crate) const PRESENCE_GRACE: Duration = Duration::from_secs(30);

#[derive(thiserror::Error, Debug)]
pub(crate) enum SessionError {
    #[error("Relay database error")]
    Store(#[from] StoreError),
    #[error("Relay session unavailable")]
    Unavailable,
}

#[derive(Clone)]
pub(crate) struct SessionHub(Arc<Mutex<Hub>>);

struct Hub {
    store: Store,
    accounts: HashMap<String, AccountSession>,
}

pub(crate) struct Peer {
    pub generation: u64,
    pub out: mpsc::Sender<ServerMessage>,
    pub cancel: watch::Sender<bool>,
}

impl Peer {
    fn send(&self, msg: ServerMessage) -> bool {
        if self.out.try_send(msg).is_err() {
            let _ = self.cancel.send(true);
            return false;
        }
        true
    }
}

#[derive(Default)]
struct AccountSession {
    devices: BTreeMap<String, Device>,
    peers: HashMap<String, Peer>,
    active: Option<String>,
    active_deadline: Option<Instant>,
    state: Option<PlaybackState>,
}

impl AccountSession {
    fn device_list(&self) -> Vec<Device> {
        self.devices.values().cloned().collect()
    }

    fn broadcast(&self, except: &str, msg: ServerMessage) {
        for (id, peer) in &self.peers {
            if id != except {
                peer.send(msg.clone());
            }
        }
    }

    fn devices_changed(&self, except: &str) {
        self.broadcast(
            except,
            ServerMessage::DevicesChanged {
                devices: self.device_list(),
            },
        );
    }

    fn expire(&mut self, now: Instant) {
        if self.active_deadline.is_none_or(|deadline| now < deadline) {
            return;
        }
        self.active_deadline = None;
        if let Some(id) = self.active.take() {
            if let Some(device) = self.devices.get_mut(&id) {
                device.online = false;
            }
            self.devices_changed("");
            self.broadcast("", ServerMessage::ActiveDeviceChanged { device_id: None });
        }
    }

    fn accepts_state(&self, id: &str, state: &PlaybackState) -> Result<bool, InvalidState> {
        if self.active.as_deref().is_some_and(|active| active != id) {
            return Ok(false);
        }
        if !state.valid() {
            return Err(InvalidState);
        }
        Ok(self
            .state
            .as_ref()
            .is_none_or(|previous| state.seq > previous.seq))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct InvalidState;

pub(crate) fn error(code: &str, message: &str) -> ServerMessage {
    ServerMessage::Error {
        code: code.into(),
        message: message.into(),
    }
}

fn wall_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

impl SessionHub {
    pub fn new(store: Store) -> Self {
        Self(Arc::new(Mutex::new(Hub {
            store,
            accounts: HashMap::new(),
        })))
    }

    async fn with<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Hub) -> Result<T, SessionError> + Send + 'static,
    ) -> Result<T, SessionError> {
        let hub = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut hub = hub.lock().map_err(|_| SessionError::Unavailable)?;
            operation(&mut hub)
        })
        .await
        .map_err(|_| SessionError::Unavailable)?
    }

    pub async fn join(&self, hello: Hello, peer: Peer) -> Result<ServerMessage, SessionError> {
        self.with(move |hub| hub.join(hello, peer)).await
    }

    pub async fn receive(
        &self,
        user: String,
        id: String,
        generation: u64,
        msg: ClientMessage,
    ) -> Result<(), SessionError> {
        self.with(move |hub| hub.receive(&user, &id, generation, msg))
            .await
    }

    pub async fn disconnect(
        &self,
        user: String,
        id: String,
        generation: u64,
    ) -> Result<(), SessionError> {
        let timer_user = user.clone();
        let result = self
            .with(move |hub| hub.disconnect(&user, &id, generation))
            .await;
        if matches!(result, Ok(true) | Err(_)) {
            let hub = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(PRESENCE_GRACE).await;
                if hub
                    .with(move |hub| {
                        if let Some(account) = hub.accounts.get_mut(&timer_user) {
                            account.expire(Instant::now());
                            if account.peers.is_empty() && account.active.is_none() {
                                hub.accounts.remove(&timer_user);
                            }
                        }
                        Ok(())
                    })
                    .await
                    .is_err()
                {
                    tracing::warn!("Could not expire relay presence");
                }
            });
        }
        result.map(|_| ())
    }
}

impl Hub {
    fn join(&mut self, hello: Hello, peer: Peer) -> Result<ServerMessage, SessionError> {
        let user = hello.username;
        let account = match self.accounts.entry(user.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => entry.insert(AccountSession {
                devices: self
                    .store
                    .devices(&user)?
                    .into_iter()
                    .map(|d| (d.id.clone(), d))
                    .collect(),
                state: self.store.playback(&user)?,
                ..Default::default()
            }),
        };
        account.expire(Instant::now());
        let id = hello.device_id;
        let device = Device {
            id: id.clone(),
            name: hello.device_name,
            os: hello.os,
            online: true,
            last_seen: wall_ms(),
        };
        self.store.save_device(&user, &device)?;
        account.devices.insert(id.clone(), device);
        if let Some(old) = account.peers.insert(id.clone(), peer) {
            let _ = old.cancel.send(true);
        }
        if account.active.is_none() {
            account.active = Some(id.clone());
            account.broadcast(
                &id,
                ServerMessage::ActiveDeviceChanged {
                    device_id: account.active.clone(),
                },
            );
        }
        if account.active.as_deref() == Some(id.as_str()) {
            account.active_deadline = None;
        }
        account.devices_changed(&id);
        Ok(ServerMessage::Welcome {
            devices: account.device_list(),
            active_device_id: account.active.clone(),
            state: account.state.clone(),
            settings: serde_json::json!({}),
            settings_version: 0,
        })
    }

    fn disconnect(&mut self, user: &str, id: &str, generation: u64) -> Result<bool, SessionError> {
        let Some(account) = self.accounts.get_mut(user) else {
            return Ok(false);
        };
        if account
            .peers
            .get(id)
            .is_none_or(|peer| peer.generation != generation)
        {
            return Ok(false);
        }
        account.peers.remove(id);
        let active = account.active.as_deref() == Some(id);
        if active {
            account.active_deadline = Some(Instant::now() + PRESENCE_GRACE);
        }
        let result = account
            .devices
            .get_mut(id)
            .map(|device| {
                device.last_seen = wall_ms();
                device.online = active;
                self.store.save_device(user, device)
            })
            .transpose();
        account.devices_changed("");
        if account.peers.is_empty() && account.active.is_none() {
            self.accounts.remove(user);
        }
        result?;
        Ok(active)
    }

    fn receive(
        &mut self,
        user: &str,
        id: &str,
        generation: u64,
        msg: ClientMessage,
    ) -> Result<(), SessionError> {
        let Some(account) = self.accounts.get_mut(user) else {
            return Ok(());
        };
        account.expire(Instant::now());
        let Some(peer) = account
            .peers
            .get(id)
            .filter(|peer| peer.generation == generation)
        else {
            return Ok(());
        };
        let reply = match msg {
            ClientMessage::StateUpdate { mut state } => match account.accepts_state(id, &state) {
                Ok(true) => {
                    state.captured_at_ms = wall_ms();
                    self.store.save_playback(user, &state)?;
                    account.state = Some(state.clone());
                    if account.active.is_none() {
                        account.active = Some(id.into());
                        account.broadcast(
                            "",
                            ServerMessage::ActiveDeviceChanged {
                                device_id: account.active.clone(),
                            },
                        );
                    }
                    account.broadcast(
                        id,
                        ServerMessage::State {
                            state,
                            from_device_id: id.into(),
                        },
                    );
                    return Ok(());
                }
                Ok(false) => return Ok(()),
                Err(_) => error("invalid_state", "Invalid playback state"),
            },
            ClientMessage::Command {
                target_device_id,
                cmd,
            } => {
                if !cmd.valid() {
                    error("invalid_command", "Invalid playback command")
                } else if account.peers.get(&target_device_id).is_some_and(|target| {
                    target.send(ServerMessage::Command {
                        cmd,
                        from_device_id: id.into(),
                    })
                }) {
                    return Ok(());
                } else {
                    error("device_unavailable", "Target device is not available")
                }
            }
            ClientMessage::Ping => ServerMessage::Pong,
            ClientMessage::Hello(_) => error("unexpected_hello", "Already authenticated"),
            _ => error("unsupported", "Message is not supported by this relay"),
        };
        peer.send(reply);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_active_device_can_advance_sequence() {
        let account = AccountSession {
            active: Some("a".into()),
            state: Some(PlaybackState {
                seq: 7,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut incoming = PlaybackState {
            seq: 7,
            ..Default::default()
        };
        assert_eq!(account.accepts_state("a", &incoming), Ok(false));
        incoming.seq = 8;
        assert_eq!(account.accepts_state("b", &incoming), Ok(false));
        assert_eq!(account.accepts_state("a", &incoming), Ok(true));
    }

    #[test]
    fn expiry_releases_active_device_without_losing_last_state() {
        let now = Instant::now();
        let mut account = AccountSession {
            active: Some("a".into()),
            active_deadline: Some(now + PRESENCE_GRACE),
            state: Some(PlaybackState {
                seq: 7,
                ..Default::default()
            }),
            ..Default::default()
        };
        account.expire(now);
        assert_eq!(account.active.as_deref(), Some("a"));
        account.expire(now + PRESENCE_GRACE);
        assert_eq!(account.active, None);
        assert_eq!(account.state.map(|state| state.seq), Some(7));
    }
}
