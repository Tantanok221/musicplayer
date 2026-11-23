use super::{Command, Event, persist_preferences};
use crate::{
    catalog::Catalog,
    model::{PlaybackSnapshot, Preferences, SavedQueue},
    platform::{Account, Paths},
    playback::{PlayerCommand, PlayerHandle},
    sync::{
        DeviceStatus, RelayEvent, position_at,
        relay::{self, RelayTask},
        wall_ms,
    },
};
use anyhow::Result;
use flume::Sender;
use sonora_sync_protocol::{
    ClientMessage, Hello, PlaybackCommand, PlaybackState, ServerMessage, TRANSFER_TIMEOUT,
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
    configuration: Option<(String, String, blake3::Hash)>,
    task: Option<JoinHandle<()>>,
    out: Option<mpsc::Sender<ClientMessage>>,
    runtime: Handle,
    tx: Sender<Command>,
    playback: watch::Receiver<PlaybackSnapshot>,
    transfer: Option<PreparedTransfer>,
}

struct PreparedTransfer {
    id: String,
    backup: SavedQueue,
    was_playing: bool,
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
            runtime,
            tx,
            playback,
            transfer: None,
        }
    }

    pub fn configure(
        &mut self,
        paths: &Paths,
        account: Option<&Account>,
        url: &str,
        events: &Sender<Event>,
    ) {
        if let Err(error) = self.configure_inner(paths, account, url, events) {
            self.stop();
            let _ = events.send(Event::DeviceSync(self.status.clone()));
            let _ = events.send(Event::Error(error.to_string()));
        }
    }

    fn configure_inner(
        &mut self,
        paths: &Paths,
        account: Option<&Account>,
        url: &str,
        events: &Sender<Event>,
    ) -> Result<()> {
        let Some((account, endpoint)) = account.zip(relay::endpoint(url)?) else {
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
            endpoint.as_str().to_owned(),
            digest.finalize(),
        );
        if self.configuration.as_ref() == Some(&key) {
            return Ok(());
        }
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
        self.status.enabled = true;
        self.status.local_device_id.clone_from(&device_id);
        events.send(Event::DeviceSync(self.status.clone()))?;
        self.task = Some(
            self.runtime.spawn(
                RelayTask {
                    endpoint,
                    hello: Hello {
                        username: account.username.clone(),
                        subsonic_token: credentials.token,
                        subsonic_salt: credentials.salt,
                        device_id,
                        device_name,
                        os: os.into(),
                        client_version: env!("CARGO_PKG_VERSION").into(),
                    },
                    namespace,
                    service: self.tx.clone(),
                    generation: self.generation,
                    playback: self.playback.clone(),
                }
                .run(),
            ),
        );
        Ok(())
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
                        ..
                    },
                out,
            } => {
                self.out = Some(out);
                self.status.connected = true;
                self.status.devices = devices;
                self.status.active_device_id = active_device_id;
                self.status.playback = None;
                if let Some(state) = state {
                    self.status.accept(state);
                }
            }
            RelayEvent::Offline => {
                self.out = None;
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
        events.send(Event::DeviceSync(self.status.clone()))?;
        Ok(())
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
        if let PlayerCommand::Replace(tracks, index) = command {
            self.replace_remote_queue(&tracks, index);
        }
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
        self.transfer = None;
        self.status = DeviceStatus::default();
    }
}

impl Drop for DeviceSync {
    fn drop(&mut self) {
        self.stop();
    }
}
