use super::{Command, Event, persist_preferences};
use crate::{
    catalog::Catalog,
    model::{PlaybackSnapshot, Preferences, SavedQueue},
    platform::{Account, Paths},
    playback::{PlayerCommand, PlayerHandle},
    server::Server,
    sync::{
        DeviceStatus, RelayEvent, SharedPreferences,
        lan::{LanNode, Port, user_hash},
        lan_registry::LanRegistry,
        position_at,
        relay::{self, RelayTransport, SyncTask},
        settings::SettingsState,
        wall_ms,
    },
};
use crate::{playback::QueueEdit, sync::local_placement};
use anyhow::Result;
use flume::Sender;
use sonora_sync_protocol::{
    ClientMessage, Device, Hello, PlaybackCommand, PlaybackState, QueueChange, ServerMessage,
    TRANSFER_TIMEOUT, VersionedSettings, queue_fingerprint, valid_settings,
};
use std::time::Duration;
use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
    task::JoinHandle,
};

pub(super) struct DeviceSync {
    pub status: DeviceStatus,
    generation: u64,
    configuration: Option<(String, String, blake3::Hash, u16)>,
    task: Option<JoinHandle<()>>,
    out: Option<mpsc::Sender<ClientMessage>>,
    lan: Option<Sender<LanAction>>,
    runtime: Handle,
    tx: Sender<Command>,
    playback: watch::Receiver<PlaybackSnapshot>,
    transfer: Option<PreparedTransfer>,
    settings_flight: Option<VersionedSettings>,
}

struct PreparedTransfer {
    id: String,
    backup: SavedQueue,
    was_playing: bool,
}

pub(super) enum LanAction {
    Revoke(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsFrame {
    Welcome,
    Update,
    Conflict,
}

impl DeviceSync {
    pub fn new(
        runtime: Handle,
        tx: Sender<Command>,
        playback: watch::Receiver<PlaybackSnapshot>,
    ) -> Self {
        Self {
            status: DeviceStatus::default(),
            generation: 0,
            configuration: None,
            task: None,
            out: None,
            lan: None,
            runtime,
            tx,
            playback,
            transfer: None,
            settings_flight: None,
        }
    }

    pub fn configure(
        &mut self,
        paths: &Paths,
        account: Option<&Account>,
        prefs: &Preferences,
        events: &Sender<Event>,
    ) {
        if let Err(error) = self.configure_inner(paths, account, prefs, events) {
            self.stop();
            let _ = events.send(Event::DeviceSync(self.status.clone()));
            let _ = events.send(Event::Error(error.to_string()));
        }
    }

    fn configure_inner(
        &mut self,
        paths: &Paths,
        account: Option<&Account>,
        prefs: &Preferences,
        events: &Sender<Event>,
    ) -> Result<()> {
        let endpoint = relay::endpoint(&prefs.sync_server_url)?;
        let port = prefs.lan_port.unwrap_or(0);
        let Some(account) = account else {
            if self.configuration.is_some() {
                self.stop();
                events.send(Event::DeviceSync(self.status.clone()))?;
            }
            return Ok(());
        };
        let Some(credentials) = paths.credentials(account)? else {
            self.stop();
            events.send(Event::DeviceSync(self.status.clone()))?;
            return Ok(());
        };
        let mut digest = blake3::Hasher::new();
        digest.update(credentials.token.as_bytes());
        digest.update(credentials.salt.as_bytes());
        let namespace = account.namespace();
        let key = (
            namespace.clone(),
            endpoint
                .as_ref()
                .map_or_else(String::new, |endpoint| endpoint.as_str().to_owned()),
            digest.finalize(),
            port,
        );
        if self.configuration.as_ref() == Some(&key) {
            return Ok(());
        }
        let previous_enabled = self.status.enabled;
        self.stop();
        let device_id = paths.sync_device_id()?;
        let os = std::env::consts::OS;
        let system_label = match os {
            "macos" => "Mac",
            "windows" => "Windows",
            "linux" => "Linux",
            _ => "Desktop",
        };
        let device_name = format!("{system_label} · {}", &device_id[..6]);
        self.configuration = Some(key);
        self.status.enabled = endpoint.is_some();
        self.status.local_device_id.clone_from(&device_id);
        if self.status.enabled || previous_enabled {
            events.send(Event::DeviceSync(self.status.clone()))?;
        }
        let registry_server = Server::with_credentials(account.clone(), credentials.clone()).ok();
        let relay = endpoint.map(|endpoint| RelayTransport {
            endpoint,
            hello: Hello {
                username: account.username.clone(),
                subsonic_token: credentials.token,
                subsonic_salt: credentials.salt,
                device_id: device_id.clone(),
                device_name: device_name.clone(),
                os: os.into(),
                client_version: env!("CARGO_PKG_VERSION").into(),
            },
        });
        let device = Device {
            id: device_id.clone(),
            name: device_name,
            os: os.into(),
            online: true,
            last_seen: 0,
        };
        let identity = paths.sync_signing_key()?;
        let group = user_hash(account);
        let port = match port {
            0 => Port::Remembered(paths.data.join("sync-lan-port")),
            port => Port::Fixed(port),
        };
        let registry = registry_server.and_then(|server| {
            LanRegistry::open(
                server,
                &device,
                &identity,
                paths.data.join(format!("sync-devices-{namespace}.json")),
            )
            .ok()
        });
        let task = SyncTask {
            relay,
            device_id,
            namespace,
            service: self.tx.clone(),
            generation: self.generation,
            playback: self.playback.clone(),
        };
        let (actions, requests) = flume::bounded::<LanAction>(8);
        self.lan = Some(actions);
        let events = events.clone();
        self.task = Some(self.runtime.spawn(async move {
            let Ok(node) = LanNode::start(device, identity, group, port, registry, Vec::new()).await else {
                let _ = events.send(Event::LanStatus(Default::default()));
                let _ = events.send(Event::Error("Could not start local device sync".into()));
                Box::pin(task.run(None)).await;
                return;
            };
            let control = async {
                let mut changes = node.changes();
                let _ = events.send(Event::LanStatus(node.status()));
                loop {
                    tokio::select! {
                        action = requests.recv_async() => match action {
                            Ok(LanAction::Revoke(id)) => {
                                if node.revoke(&id).is_err() {
                                    let _ = events.send(Event::Error("Could not revoke LAN device".into()));
                                } else {
                                    let _ = events.send(Event::LanStatus(node.status()));
                                    node.refresh_registry().await;
                                }
                            }
                            Err(_) => break,
                        },
                        changed = changes.changed() => {
                            if changed.is_err() || events.send(Event::LanStatus(node.status())).is_err() { break; }
                        }
                    }
                }
            };
            let directory = async {
                let mut changes = node.changes();
                let mut refresh = tokio::time::interval(Duration::from_secs(5));
                refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = refresh.tick() => node.refresh_registry().await,
                        changed = changes.changed() => {
                            if changed.is_err() { break; }
                            node.refresh_registry().await;
                        }
                    }
                }
            };
            tokio::select! {
                _ = Box::pin(task.run(Some(&node))) => {}
                _ = control => {}
                _ = directory => {}
            }
        }));
        Ok(())
    }

    pub fn lan(&self, action: LanAction) -> bool {
        self.lan
            .as_ref()
            .is_some_and(|commands| commands.try_send(action).is_ok())
    }

    pub fn receive(
        &mut self,
        generation: u64,
        event: RelayEvent,
        catalog: &Catalog,
        player: &PlayerHandle,
        events: &Sender<Event>,
    ) -> Result<()> {
        if generation != self.generation {
            return Ok(());
        }
        let was_remote = self.status.remote_id().is_some();
        match event {
            RelayEvent::Online {
                welcome:
                    ServerMessage::Welcome {
                        devices,
                        active_device_id,
                        state,
                        settings,
                        settings_version,
                    },
                out,
            } => {
                self.out = Some(out);
                self.status.connected = true;
                self.status.enabled = true;
                self.status.devices = devices;
                self.status.active_device_id = active_device_id;
                self.status.playback = None;
                if let Some(state) = state {
                    self.status.accept(state);
                }
                self.settings_flight = None;
                self.receive_settings(
                    (settings, settings_version),
                    SettingsFrame::Welcome,
                    catalog,
                    player,
                    events,
                )?;
            }
            RelayEvent::Offline => {
                self.out = None;
                self.settings_flight = None;
                self.status.connected = false;
                self.rollback_transfer(catalog, player)?;
            }
            RelayEvent::Incoming(ServerMessage::State {
                state,
                from_device_id,
            }) => {
                if self.status.active_device_id.as_deref() != Some(&from_device_id)
                    || !self.status.accept(state)
                {
                    return Ok(());
                }
            }
            RelayEvent::Incoming(ServerMessage::ActiveDeviceChanged { device_id }) => {
                self.status.active_device_id = device_id;
            }
            RelayEvent::Incoming(ServerMessage::DevicesChanged { devices }) => {
                self.status.devices = devices;
            }
            RelayEvent::Incoming(ServerMessage::Command { cmd, .. }) => {
                self.apply_remote(cmd, catalog, player, events)?;
                return Ok(());
            }
            RelayEvent::Incoming(ServerMessage::Settings { settings, version }) => {
                self.receive_settings(
                    (settings, version),
                    SettingsFrame::Update,
                    catalog,
                    player,
                    events,
                )?;
                return Ok(());
            }
            RelayEvent::Incoming(ServerMessage::SettingsConflict { settings, version }) => {
                self.receive_settings(
                    (settings, version),
                    SettingsFrame::Conflict,
                    catalog,
                    player,
                    events,
                )?;
                return Ok(());
            }
            RelayEvent::Incoming(ServerMessage::TransferRequest {
                transfer_id,
                state,
                start_playing,
            }) => {
                self.prepare_transfer(transfer_id, state, start_playing, catalog, player)?;
                return Ok(());
            }
            RelayEvent::Incoming(ServerMessage::TransferResult {
                transfer_id,
                ok,
                error,
            }) => {
                if self
                    .transfer
                    .as_ref()
                    .is_some_and(|pending| pending.id == transfer_id)
                {
                    if ok {
                        self.transfer = None;
                        let (reply, _) = flume::bounded(1);
                        player.send(PlayerCommand::SaveQueue(reply));
                    } else {
                        self.rollback_transfer(catalog, player)?;
                    }
                }
                if !ok {
                    events.send(Event::Error(
                        error.unwrap_or_else(|| "Playback transfer failed".into()),
                    ))?;
                }
                return Ok(());
            }
            RelayEvent::Incoming(ServerMessage::Error { message, .. }) => {
                events.send(Event::Error(message))?;
                return Ok(());
            }
            _ => return Ok(()),
        }
        if !was_remote && self.status.remote_id().is_some() && self.playback.borrow().playing {
            player.send(PlayerCommand::Pause);
        }
        self.status.track = self
            .status
            .track_id()
            .map(|id| catalog.by_ids(&[id.into()]))
            .transpose()?
            .and_then(|mut tracks| tracks.pop());
        let ids = self
            .status
            .playback
            .as_ref()
            .filter(|_| self.status.remote_id().is_some())
            .map_or(&[][..], |state| state.queue.as_slice());
        if !ids
            .iter()
            .eq(self.status.queue.iter().map(|track| &track.id))
        {
            self.status.queue = resolve_queue(catalog, ids)?.into();
        }
        events.send(Event::DeviceSync(self.status.clone()))?;
        Ok(())
    }

    pub fn share_preferences(
        &mut self,
        before: &Preferences,
        after: &Preferences,
        catalog: &Catalog,
    ) -> Result<()> {
        if self.status.enabled {
            let mut state: SettingsState = catalog.state("sync:settings")?;
            state.record(before, after);
            catalog.set_state("sync:settings", &state)?;
            self.publish_settings(&state);
        }
        Ok(())
    }

    fn receive_settings(
        &mut self,
        (value, version): (serde_json::Value, u64),
        frame: SettingsFrame,
        catalog: &Catalog,
        player: &PlayerHandle,
        events: &Sender<Event>,
    ) -> Result<()> {
        let serde_json::Value::Object(settings) = value else {
            return Ok(());
        };
        if !valid_settings(&settings) {
            return Ok(());
        }
        let mut state: SettingsState = catalog.state("sync:settings")?;
        if version < state.remote.version {
            if frame == SettingsFrame::Welcome {
                self.outbound(ClientMessage::Reconcile {
                    playback: None,
                    settings: Some(state.remote.clone()),
                });
            }
            return Ok(());
        }
        if frame == SettingsFrame::Conflict
            || self.settings_flight.as_ref().is_some_and(|flight| {
                flight.version.checked_add(1) == Some(version) && flight.settings == settings
            })
        {
            self.settings_flight = None;
        }
        let mut prefs: Preferences = catalog.state("preferences")?;
        if frame == SettingsFrame::Welcome
            && version == 0
            && settings.is_empty()
            && state.draft().is_none()
        {
            state.record(&Preferences::default(), &prefs);
        }
        state.receive(VersionedSettings { settings, version });
        let prior = SharedPreferences::from_preferences(&prefs);
        let merged = state.merged();
        if merged.apply(&mut prefs).is_err() {
            return Ok(());
        }
        catalog.set_state("sync:settings", &state)?;
        if prior != SharedPreferences::from_preferences(&prefs) {
            persist_preferences(catalog, player, prefs)?;
            events.send(Event::SharedPreferences(merged))?;
        }
        self.publish_settings(&state);
        Ok(())
    }

    fn publish_settings(&mut self, state: &SettingsState) {
        if self.settings_flight.is_none()
            && let Some(draft) = state.draft()
            && self.outbound(ClientMessage::SettingsPut {
                settings: draft.settings.clone().into(),
                base_version: draft.version,
            })
        {
            self.settings_flight = Some(draft);
        }
    }

    pub fn transfer_to(&self, target_device_id: String, start_playing: bool) -> bool {
        self.outbound(ClientMessage::Transfer {
            target_device_id,
            start_playing,
        })
    }

    pub fn preparing_transfer(&self) -> bool {
        self.transfer.is_some()
    }

    pub fn can_save_queue(&self) -> bool {
        !self.preparing_transfer() && self.status.remote_id().is_none()
    }

    pub fn can_offer_queue_resume(&self) -> bool {
        self.can_save_queue()
            && self
                .configuration
                .as_ref()
                .is_none_or(|(_, relay, _, _)| relay.is_empty())
    }

    fn prepare_transfer(
        &mut self,
        id: String,
        state: PlaybackState,
        start_playing: bool,
        catalog: &Catalog,
        player: &PlayerHandle,
    ) -> Result<()> {
        let Some(index) = state
            .index
            .filter(|_| state.valid() && self.transfer.is_none())
        else {
            self.transfer_ack(id, false);
            return Ok(());
        };
        let tracks = catalog.by_ids(&state.queue)?;
        if tracks.len() != state.queue.len() {
            self.transfer_ack(id, false);
            return Ok(());
        }
        let position = position_at(&state, wall_ms()) as f64 / 1000.;
        let position = tracks.get(index).map_or(position, |track| {
            if track.duration > 0. {
                position.min(track.duration)
            } else {
                position
            }
        });
        self.transfer = Some(PreparedTransfer {
            id: id.clone(),
            backup: catalog.state("queue")?,
            was_playing: self.playback.borrow().playing,
        });
        let (reply, prepared) = flume::bounded(1);
        let mut playback = self.playback.clone();
        playback.mark_unchanged();
        player.send(PlayerCommand::Restore(tracks, index, position, None));
        if start_playing {
            player.send(PlayerCommand::Play);
        }
        player.send(PlayerCommand::SaveQueue(reply));

        let generation = self.generation;
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let began = tokio::time::Instant::now();
            let ready = async {
                let Ok(queue) = prepared.recv_async().await else {
                    return false;
                };
                if queue.ids != state.queue || queue.index != Some(index) {
                    return false;
                }
                if !start_playing {
                    return true;
                }
                while playback.changed().await.is_ok() {
                    let sample = playback.borrow_and_update();
                    if sample.index == Some(index)
                        && sample.queue.iter().map(|track| &track.id).eq(&state.queue)
                        && sample.playing
                        && !sample.buffering
                        && sample.error.is_none()
                    {
                        return true;
                    }
                }
                false
            };
            let ok = tokio::time::timeout(TRANSFER_TIMEOUT - Duration::from_secs(1), ready)
                .await
                .unwrap_or(false);
            let _ = tx.send(Command::TransferPrepared(generation, id.clone(), ok));
            if ok {
                tokio::time::sleep_until(began + TRANSFER_TIMEOUT).await;
                let _ = tx.send(Command::TransferExpired(generation, id));
            }
        });
        Ok(())
    }

    pub fn transfer_prepared(
        &mut self,
        generation: u64,
        id: String,
        ok: bool,
        catalog: &Catalog,
        player: &PlayerHandle,
    ) -> Result<()> {
        if generation == self.generation
            && self
                .transfer
                .as_ref()
                .is_some_and(|pending| pending.id == id)
        {
            let sent = self.transfer_ack(id, ok);
            if !(ok && sent) {
                self.rollback_transfer(catalog, player)?;
            }
        }
        Ok(())
    }

    pub fn transfer_expired(
        &mut self,
        generation: u64,
        id: &str,
        catalog: &Catalog,
        player: &PlayerHandle,
    ) -> Result<()> {
        if generation == self.generation
            && self
                .transfer
                .as_ref()
                .is_some_and(|pending| pending.id == id)
        {
            self.rollback_transfer(catalog, player)?;
        }
        Ok(())
    }

    pub fn rollback_transfer(&mut self, catalog: &Catalog, player: &PlayerHandle) -> Result<()> {
        if let Some(transfer) = self.transfer.take() {
            player.send(PlayerCommand::Restore(
                catalog.by_ids(&transfer.backup.ids)?,
                transfer.backup.index.unwrap_or(0),
                transfer.backup.position,
                transfer.backup.listen,
            ));
            if transfer.was_playing {
                player.send(PlayerCommand::Play);
            }
        }
        Ok(())
    }

    fn transfer_ack(&self, transfer_id: String, ok: bool) -> bool {
        self.outbound(ClientMessage::TransferAck {
            transfer_id,
            ok,
            error: (!ok).then(|| "Target could not prepare playback".into()),
        })
    }

    fn apply_remote(
        &self,
        command: PlaybackCommand,
        catalog: &Catalog,
        player: &PlayerHandle,
        events: &Sender<Event>,
    ) -> Result<()> {
        if !command.valid() {
            return Ok(());
        }
        let command = match command {
            PlaybackCommand::Play => PlayerCommand::Play,
            PlaybackCommand::Pause => PlayerCommand::Pause,
            PlaybackCommand::Next => PlayerCommand::Next,
            PlaybackCommand::Prev => PlayerCommand::Previous,
            PlaybackCommand::Seek { ms } => PlayerCommand::Seek(ms as f64 / 1000.),
            PlaybackCommand::PlayIndex { i } => PlayerCommand::JumpTo(i),
            PlaybackCommand::SetVolume { volume } => {
                let mut prefs: Preferences = catalog.state("preferences")?;
                prefs.volume = f32::from(volume) / 100.;
                let volume = prefs.volume;
                persist_preferences(catalog, player, prefs)?;
                events.send(Event::LocalVolume(volume))?;
                return Ok(());
            }
            PlaybackCommand::EditQueue { queue, change } => {
                let (current, revision) = {
                    let playback = self.playback.borrow();
                    let ids: Vec<_> = playback
                        .queue
                        .iter()
                        .map(|track| track.id.clone())
                        .collect();
                    (queue_fingerprint(&ids), playback.queue_revision)
                };
                let command = match change {
                    QueueChange::Insert {
                        track_ids,
                        placement,
                    } => {
                        let tracks = catalog.by_ids(&track_ids)?;
                        if tracks.len() != track_ids.len() {
                            return Ok(());
                        }
                        PlayerCommand::Insert(tracks, local_placement(placement))
                    }
                    // Positions refer to the queue the controller saw; skip edits to an older queue.
                    _ if queue != current => return Ok(()),
                    QueueChange::RemoveAt { index } => PlayerCommand::Remove(index),
                    QueueChange::MoveTo { from, to } => PlayerCommand::Move(from, to),
                    QueueChange::Remove { indices } => {
                        PlayerCommand::EditQueue(revision, QueueEdit::Remove(indices))
                    }
                    QueueChange::Move { indices, before } => {
                        PlayerCommand::EditQueue(revision, QueueEdit::Move { indices, before })
                    }
                    QueueChange::ClearUpcoming => {
                        PlayerCommand::EditQueue(revision, QueueEdit::ClearUpcoming)
                    }
                };
                player.send(command);
                return Ok(());
            }
            PlaybackCommand::SetQueue {
                track_ids,
                index,
                position_ms,
            } => {
                let tracks = catalog.by_ids(&track_ids)?;
                if tracks.len() != track_ids.len() {
                    return Ok(());
                }
                player.send(PlayerCommand::Restore(
                    tracks,
                    index.unwrap_or(0),
                    position_ms as f64 / 1000.,
                    None,
                ));
                if self.playback.borrow().playing {
                    player.send(PlayerCommand::Play);
                }
                return Ok(());
            }
        };
        player.send(command);
        Ok(())
    }

    pub fn send(&self, cmd: PlaybackCommand) -> bool {
        let Some(target) = self.status.remote_id() else {
            return false;
        };
        self.outbound(ClientMessage::Command {
            target_device_id: target.into(),
            cmd,
        })
    }

    fn outbound(&self, message: ClientMessage) -> bool {
        self.out
            .as_ref()
            .is_some_and(|out| out.try_send(message).is_ok())
    }

    pub fn playback_intent(&self, player: &PlayerHandle, command: PlayerCommand) {
        if self.status.remote_id().is_none() {
            player.send(command);
            return;
        }
        if let PlayerCommand::Replace(tracks, index) = &command {
            self.replace_remote_queue(tracks, *index);
        } else if let Some(change) = crate::sync::queue_change(&command) {
            self.send(PlaybackCommand::EditQueue {
                queue: self.remote_fingerprint(),
                change,
            });
        }
    }

    /// The fingerprint of the active remote device's queue as last reported.
    fn remote_fingerprint(&self) -> String {
        queue_fingerprint(
            self.status
                .playback
                .as_ref()
                .map_or(&[][..], |state| state.queue.as_slice()),
        )
    }

    pub fn replace_remote_queue(
        &self,
        tracks: &[std::sync::Arc<crate::model::Track>],
        index: usize,
    ) {
        let queued = self.send(PlaybackCommand::SetQueue {
            track_ids: tracks.iter().map(|track| track.id.clone()).collect(),
            index: (!tracks.is_empty()).then_some(index),
            position_ms: 0,
        });
        if queued && !tracks.is_empty() {
            self.send(PlaybackCommand::Play);
        }
    }

    fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.generation = self.generation.wrapping_add(1);
        self.configuration = None;
        self.out = None;
        self.lan = None;
        self.transfer = None;
        self.settings_flight = None;
        self.status = DeviceStatus::default();
    }
}

/// The tracks of a remote queue in order; IDs missing here show as untitled entries.
fn resolve_queue(
    catalog: &Catalog,
    ids: &[String],
) -> Result<Vec<std::sync::Arc<crate::model::Track>>> {
    let known: std::collections::HashMap<_, _> = catalog
        .by_ids(ids)?
        .into_iter()
        .map(|track| (track.id.clone(), track))
        .collect();
    Ok(ids
        .iter()
        .map(|id| {
            known.get(id).cloned().unwrap_or_else(|| {
                std::sync::Arc::new(crate::model::Track {
                    id: id.clone(),
                    title: "Unknown track".into(),
                    ..Default::default()
                })
            })
        })
        .collect())
}

impl Drop for DeviceSync {
    fn drop(&mut self) {
        self.stop();
    }
}
