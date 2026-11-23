use super::{Command, Event, persist_preferences};
use crate::{
    catalog::Catalog,
    model::{PlaybackSnapshot, Preferences},
    platform::{Account, Paths},
    playback::{PlayerCommand, PlayerHandle},
    sync::{
        DeviceStatus, RelayEvent,
        relay::{self, RelayTask},
    },
};
use anyhow::Result;
use flume::Sender;
use sonora_sync_protocol::{ClientMessage, Hello, PlaybackCommand, ServerMessage};
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
        let Some((out, target)) = self.out.as_ref().zip(self.status.remote_id()) else {
            return false;
        };
        out.try_send(ClientMessage::Command {
            target_device_id: target.into(),
            cmd,
        })
        .is_ok()
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
        self.status = DeviceStatus::default();
    }
}

impl Drop for DeviceSync {
    fn drop(&mut self) {
        self.stop();
    }
}
