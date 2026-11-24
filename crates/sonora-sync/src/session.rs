use crate::store::{Store, StoreError};
use sonora_sync_protocol::{
    ClientMessage, Device, Hello, PlaybackCommand, PlaybackState, ReconciledPlayback,
    ServerMessage, TRANSFER_TIMEOUT, VersionedSettings, valid_settings,
};
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
pub enum SessionError {
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
    next_transfer: u64,
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
    transfer: Option<Transfer>,
    settings: VersionedSettings,
}

struct Transfer {
    id: String,
    requester: String,
    target: String,
    generation: u64,
    state: PlaybackState,
    start_playing: bool,
    deadline: Instant,
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
        if self
            .transfer
            .as_ref()
            .is_some_and(|transfer| now >= transfer.deadline)
        {
            self.fail_transfer("Playback transfer timed out");
        }
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

    fn transfer_result(&self, transfer: Transfer, failure: Option<&str>) {
        let reply = ServerMessage::TransferResult {
            transfer_id: transfer.id,
            ok: failure.is_none(),
            error: failure.map(str::to_owned),
        };
        if let Some(peer) = self.peers.get(&transfer.requester) {
            peer.send(reply.clone());
        }
        if transfer.target != transfer.requester
            && let Some(peer) = self.peers.get(&transfer.target)
        {
            peer.send(reply);
        }
    }

    fn fail_transfer(&mut self, reason: &str) {
        if let Some(transfer) = self.transfer.take() {
            self.transfer_result(transfer, Some(reason));
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
            next_transfer: 0,
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
        let timer_user = user.clone();
        let deadline = self
            .with(move |hub| hub.receive(&user, &id, generation, msg))
            .await?;
        if let Some(deadline) = deadline {
            let hub = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep_until(deadline).await;
                if hub
                    .with(move |hub| {
                        if let Some(account) = hub.accounts.get_mut(&timer_user) {
                            account.expire(Instant::now());
                        }
                        Ok(())
                    })
                    .await
                    .is_err()
                {
                    tracing::warn!("Could not expire device sync session");
                }
            });
        }
        Ok(())
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
                settings: self.store.settings(&user)?,
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
        if account
            .transfer
            .as_ref()
            .is_some_and(|transfer| transfer.target == id)
        {
            account.fail_transfer("Target device reconnected");
        }
        if let Some(old) = account.peers.insert(id.clone(), peer) {
            let _ = old.cancel.send(true);
        }
        if account.active.is_none()
            && account
                .state
                .as_ref()
                .is_none_or(|state| state.index.is_none())
        {
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
            settings: account.settings.settings.clone().into(),
            settings_version: account.settings.version,
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
        if account
            .transfer
            .as_ref()
            .is_some_and(|transfer| transfer.target == id)
        {
            account.fail_transfer("Target device disconnected");
        }
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
    ) -> Result<Option<Instant>, SessionError> {
        let Some(account) = self.accounts.get_mut(user) else {
            return Ok(None);
        };
        account.expire(Instant::now());
        if account
            .peers
            .get(id)
            .is_none_or(|peer| peer.generation != generation)
        {
            return Ok(None);
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
                    return Ok(None);
                }
                Ok(false) => return Ok(None),
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
                    return Ok(None);
                } else {
                    error("device_unavailable", "Target device is not available")
                }
            }
            ClientMessage::Transfer {
                target_device_id,
                start_playing,
            } => {
                self.next_transfer = self.next_transfer.wrapping_add(1);
                let transfer_id = self.next_transfer.to_string();
                match begin_transfer(account, id, target_device_id, start_playing, transfer_id) {
                    Ok(deadline) => return Ok(Some(deadline)),
                    Err(message) => error("transfer_unavailable", message),
                }
            }
            ClientMessage::TransferAck {
                transfer_id, ok, ..
            } => {
                if account.transfer.as_ref().is_none_or(|transfer| {
                    transfer.id != transfer_id
                        || transfer.target != id
                        || transfer.generation != generation
                }) {
                    error("unexpected_transfer", "No matching playback transfer")
                } else {
                    if ok {
                        finish_transfer(&self.store, user, account)?;
                    } else {
                        account.fail_transfer("Target could not prepare playback");
                    }
                    return Ok(None);
                }
            }
            ClientMessage::Ping => ServerMessage::Pong,
            ClientMessage::Reconcile { playback, settings } => {
                if playback.as_ref().is_some_and(|candidate| {
                    !candidate.state.valid()
                        || candidate.device_id.is_empty()
                        || candidate.device_id.len() > 128
                }) || settings
                    .as_ref()
                    .is_some_and(|candidate| !valid_settings(&candidate.settings))
                {
                    error("invalid_snapshot", "Invalid playback or settings snapshot")
                } else {
                    return Ok(reconcile(&self.store, user, account, playback, settings)?);
                }
            }
            ClientMessage::Hello(_) => error("unexpected_hello", "Already authenticated"),
            ClientMessage::SettingsPut {
                settings,
                base_version,
            } => match next_settings(&account.settings, settings, base_version) {
                Ok(settings) => {
                    self.store.save_settings(user, &settings)?;
                    account.broadcast(
                        "",
                        ServerMessage::Settings {
                            settings: settings.settings.clone().into(),
                            version: settings.version,
                        },
                    );
                    account.settings = settings;
                    return Ok(None);
                }
                Err(rejection) => rejection.reply(&account.settings),
            },
        };
        if let Some(peer) = account.peers.get(id) {
            peer.send(reply);
        }
        Ok(None)
    }
}

fn reconcile(
    store: &Store,
    user: &str,
    account: &mut AccountSession,
    playback: Option<ReconciledPlayback>,
    settings: Option<VersionedSettings>,
) -> Result<Option<Instant>, StoreError> {
    let mut deadline = None;
    if let Some(ReconciledPlayback {
        device_id,
        mut state,
    }) = playback
        && state.seq > account.state.as_ref().map_or(0, |previous| previous.seq)
    {
        let now = wall_ms();
        if state.playing && state.captured_at_ms > 0 {
            state.position_ms = state
                .position_ms
                .saturating_add(now.saturating_sub(state.captured_at_ms).max(0) as u64);
        }
        state.captured_at_ms = now;
        store.save_playback(user, &state)?;
        account.fail_transfer("Playback changed on another device");
        if account.active.as_ref() != Some(&device_id) {
            if let Some(previous) = account.active.as_ref().and_then(|id| account.peers.get(id)) {
                previous.send(ServerMessage::Command {
                    cmd: PlaybackCommand::Pause,
                    from_device_id: device_id.clone(),
                });
            }
            account.active = Some(device_id.clone());
            account.broadcast(
                "",
                ServerMessage::ActiveDeviceChanged {
                    device_id: account.active.clone(),
                },
            );
        }
        deadline =
            (!account.peers.contains_key(&device_id)).then(|| Instant::now() + PRESENCE_GRACE);
        account.active_deadline = deadline;
        account.state = Some(state.clone());
        account.broadcast(
            "",
            ServerMessage::State {
                state,
                from_device_id: device_id,
            },
        );
    }
    if let Some(settings) = settings
        && settings.version > account.settings.version
    {
        store.save_settings(user, &settings)?;
        account.broadcast(
            "",
            ServerMessage::Settings {
                settings: settings.settings.clone().into(),
                version: settings.version,
            },
        );
        account.settings = settings;
    }
    Ok(deadline)
}

#[derive(Debug)]
enum SettingsRejection {
    Conflict,
    Invalid,
    Exhausted,
}

impl SettingsRejection {
    fn reply(self, current: &VersionedSettings) -> ServerMessage {
        match self {
            Self::Conflict => ServerMessage::SettingsConflict {
                settings: current.settings.clone().into(),
                version: current.version,
            },
            Self::Invalid => error("invalid_settings", "Invalid shared settings"),
            Self::Exhausted => error("settings_unavailable", "Settings version is exhausted"),
        }
    }
}

fn next_settings(
    current: &VersionedSettings,
    proposed: serde_json::Value,
    base_version: u64,
) -> Result<VersionedSettings, SettingsRejection> {
    if base_version != current.version {
        return Err(SettingsRejection::Conflict);
    }
    let serde_json::Value::Object(settings) = proposed else {
        return Err(SettingsRejection::Invalid);
    };
    if !valid_settings(&settings) {
        return Err(SettingsRejection::Invalid);
    }
    let Some(version) = current.version.checked_add(1) else {
        return Err(SettingsRejection::Exhausted);
    };
    Ok(VersionedSettings { settings, version })
}

fn begin_transfer(
    account: &mut AccountSession,
    requester: &str,
    target: String,
    start_playing: bool,
    transfer_id: String,
) -> Result<Instant, &'static str> {
    if account.transfer.is_some() {
        return Err("A playback transfer is already pending");
    }
    if account.active.as_ref() == Some(&target) {
        return Err("Target device is already active");
    }
    let peer = account
        .peers
        .get(&target)
        .ok_or("Target device is not available")?;
    let mut state = account
        .state
        .clone()
        .filter(|state| state.index.is_some())
        .ok_or("There is no playback to transfer")?;
    if account
        .active
        .as_ref()
        .is_none_or(|active| !account.peers.contains_key(active))
    {
        state.playing = false;
    }
    let deadline = Instant::now() + TRANSFER_TIMEOUT;
    if !peer.send(ServerMessage::TransferRequest {
        transfer_id: transfer_id.clone(),
        state: state.clone(),
        start_playing,
    }) {
        return Err("Target device is not available");
    }
    account.transfer = Some(Transfer {
        id: transfer_id,
        requester: requester.into(),
        target,
        generation: peer.generation,
        state,
        start_playing,
        deadline,
    });
    Ok(deadline)
}

fn finish_transfer(
    store: &Store,
    user: &str,
    account: &mut AccountSession,
) -> Result<(), SessionError> {
    let Some(transfer) = account.transfer.take() else {
        return Ok(());
    };
    let Some(seq) = account
        .state
        .as_ref()
        .and_then(|state| state.seq.checked_add(1))
    else {
        account.transfer_result(transfer, Some("Playback sequence is exhausted"));
        return Ok(());
    };
    let mut state = transfer.state.clone();
    let now = wall_ms();
    if state.playing && transfer.start_playing {
        state.position_ms = state
            .position_ms
            .saturating_add(now.saturating_sub(state.captured_at_ms).max(0) as u64);
    }
    state.seq = seq;
    state.playing = transfer.start_playing;
    state.captured_at_ms = now;
    if let Err(error) = store.save_playback(user, &state) {
        account.transfer_result(transfer, Some("Could not save playback transfer"));
        return Err(error.into());
    }
    if let Some(old) = account
        .active
        .as_deref()
        .and_then(|id| account.peers.get(id))
    {
        old.send(ServerMessage::Command {
            cmd: PlaybackCommand::Pause,
            from_device_id: transfer.target.clone(),
        });
    }
    account.active = Some(transfer.target.clone());
    account.active_deadline = None;
    account.state = Some(state.clone());
    account.broadcast(
        "",
        ServerMessage::ActiveDeviceChanged {
            device_id: account.active.clone(),
        },
    );
    account.broadcast(
        "",
        ServerMessage::State {
            state,
            from_device_id: transfer.target.clone(),
        },
    );
    account.transfer_result(transfer, None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_conflict_returns_current_snapshot() {
        let previous = next_settings(
            &VersionedSettings::default(),
            serde_json::json!({"dark": true}),
            0,
        )
        .expect("initial settings");
        assert!(
            matches!(next_settings(&previous, serde_json::json!({"dark": false}), 0)
            .unwrap_err().reply(&previous),
            ServerMessage::SettingsConflict { settings, version: 1 }
                if settings == serde_json::json!({"dark": true}))
        );
    }

    #[test]
    fn transfer_deadline_leaves_source_and_playback_unchanged() {
        let now = Instant::now();
        let state = PlaybackState {
            seq: 11,
            playing: true,
            ..Default::default()
        };
        let mut account = AccountSession {
            active: Some("source".into()),
            state: Some(state.clone()),
            transfer: Some(Transfer {
                id: "1".into(),
                requester: "source".into(),
                target: "target".into(),
                generation: 1,
                state: state.clone(),
                start_playing: true,
                deadline: now + TRANSFER_TIMEOUT,
            }),
            ..Default::default()
        };
        account.expire(now + TRANSFER_TIMEOUT - Duration::from_millis(1));
        assert!(account.transfer.is_some());
        account.expire(now + TRANSFER_TIMEOUT);
        assert!(account.transfer.is_none());
        assert_eq!(account.active.as_deref(), Some("source"));
        assert_eq!(account.state, Some(state));
    }

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
