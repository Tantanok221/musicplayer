use super::{
    lan::LanNode,
    transport::{Frame, SyncLink, SyncTransport},
};
use crate::{model::PlaybackSnapshot, service::Command};
use futures_util::{SinkExt, StreamExt};
use sonora_sync_protocol::{
    ClientMessage, Hello, PlaybackState, ReconciledPlayback, ServerMessage, VersionedSettings,
};
use std::time::Duration;
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch},
    time::{self, Instant},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use url::{Host, Url};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT: Duration = Duration::from_secs(15);
const SILENCE_LIMIT: Duration = Duration::from_secs(45);
const DEBOUNCE: Duration = Duration::from_millis(300);
const STATE_INTERVAL: Duration = Duration::from_secs(5);
const MAX_MESSAGE: usize = 512 * 1024;

#[derive(thiserror::Error, Debug)]
#[error("Use a complete https:// or wss:// sync server URL")]
pub struct InvalidRelayUrl;

pub fn endpoint(value: &str) -> Result<Option<Url>, InvalidRelayUrl> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let mut url = Url::parse(value).map_err(|_| InvalidRelayUrl)?;
    let loopback = match url.host() {
        Some(Host::Ipv4(addr)) => addr.is_loopback(),
        Some(Host::Ipv6(addr)) => addr.is_loopback(),
        Some(Host::Domain("localhost")) => true,
        _ => false,
    };
    let scheme = match url.scheme() {
        "https" | "wss" => "wss",
        "http" | "ws" if loopback => "ws",
        _ => return Err(InvalidRelayUrl),
    };
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(InvalidRelayUrl);
    }
    url.set_scheme(scheme).map_err(|_| InvalidRelayUrl)?;
    let has_ws_suffix = url.path().trim_end_matches('/').ends_with("/ws");
    let mut parts = url.path_segments_mut().map_err(|_| InvalidRelayUrl)?;
    parts.pop_if_empty();
    if !has_ws_suffix {
        parts.push("ws");
    }
    drop(parts);
    Ok(Some(url))
}

#[derive(Clone)]
pub enum RelayEvent {
    Online {
        welcome: ServerMessage,
        out: mpsc::Sender<ClientMessage>,
    },
    Incoming(ServerMessage),
    Offline,
}

pub(crate) struct RelayTransport {
    pub endpoint: Url,
    pub hello: Hello,
}

pub(crate) struct SyncTask {
    pub relay: Option<RelayTransport>,
    pub device_id: String,
    pub namespace: String,
    pub service: flume::Sender<Command>,
    pub generation: u64,
    pub playback: watch::Receiver<PlaybackSnapshot>,
}

impl SyncTask {
    pub async fn run(self, lan: Option<&LanNode>) {
        let mut attempts = 0;
        let mut next_attempt = Instant::now();
        let mut publisher = Publisher::default();
        let mut ready = None;
        loop {
            if ready.is_none() && Instant::now() >= next_attempt {
                ready = self.try_relay().await;
                if ready.is_none() {
                    next_attempt = retry_at(&mut attempts);
                }
            }
            if let Some((socket, welcome)) = ready.take() {
                let connected_at = Instant::now();
                if self.serve(socket, welcome, &mut publisher).await.is_none() {
                    break;
                }
                if connected_at.elapsed() >= HEARTBEAT {
                    attempts = 0;
                }
                next_attempt = retry_at(&mut attempts);
            }
            if let Some(lan) = lan {
                let (socket, welcome) = lan.session();
                let retry = async {
                    if self.relay.is_none() {
                        return std::future::pending().await;
                    }
                    loop {
                        time::sleep_until(next_attempt).await;
                        if let Some(link) = self.try_relay().await {
                            break Some(link);
                        }
                        next_attempt = retry_at(&mut attempts);
                    }
                };
                tokio::select! {
                    biased;
                    relay = retry => {
                        if !self.emit(RelayEvent::Offline) { break; }
                        ready = relay;
                        continue;
                    }
                    done = self.serve(socket, welcome, &mut publisher) => {
                        if done.is_none() { break; }
                    }
                }
            }
            time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn try_relay(&self) -> Option<(Socket, ServerMessage)> {
        time::timeout(CONNECT_TIMEOUT, self.relay.as_ref()?.connect())
            .await
            .ok()?
    }

    async fn serve(
        &self,
        mut socket: impl SyncLink,
        welcome: ServerMessage,
        publisher: &mut Publisher,
    ) -> Option<()> {
        if let ServerMessage::Welcome { state, .. } = &welcome
            && let Some(playback) = publisher
                .snapshot
                .as_ref()
                .filter(|known| known.state.seq > state.as_ref().map_or(0, |remote| remote.seq))
            && socket
                .send(ClientMessage::Reconcile {
                    playback: Some(playback.clone()),
                    settings: None,
                })
                .await
                .is_none()
        {
            return Some(());
        }
        let (out, incoming) = mpsc::channel(32);
        if !self.emit(RelayEvent::Online {
            welcome: welcome.clone(),
            out,
        }) {
            return None;
        }
        self.session(socket, incoming, welcome, publisher).await;
        self.emit(RelayEvent::Offline).then_some(())
    }

    fn emit(&self, event: RelayEvent) -> bool {
        self.service
            .send(Command::DeviceSync(self.generation, event))
            .is_ok()
    }

    async fn session(
        &self,
        mut socket: impl SyncLink,
        mut outgoing: mpsc::Receiver<ClientMessage>,
        welcome: ServerMessage,
        publisher: &mut Publisher,
    ) -> Option<()> {
        let ServerMessage::Welcome {
            active_device_id,
            state,
            settings,
            settings_version,
            ..
        } = welcome
        else {
            return None;
        };
        if let (Some(active), Some(state)) = (&active_device_id, &state) {
            publisher.remember(active, state);
        }
        publisher.set_active(
            active_device_id,
            stamp(&self.playback.borrow(), &self.namespace),
        );
        publisher.latest_seq = publisher.latest_seq.max(state.map(|state| state.seq));
        publisher.remember_settings(&settings, settings_version);
        publisher.stamp = None;
        publisher.changed = None;
        publisher.last_sent = None;
        let mut timer = time::interval(Duration::from_millis(100));
        timer.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut last_received = Instant::now();
        let mut last_heartbeat = Instant::now();
        loop {
            tokio::select! {
                frame = socket.receive() => {
                    last_received = Instant::now();
                    match frame? {
                        Frame::Data(msg) => {
                            match &msg {
                                ServerMessage::State { state, from_device_id } => {
                                    publisher.latest_seq = Some(publisher.latest_seq.map_or(state.seq, |seq| seq.max(state.seq)));
                                    publisher.remember(from_device_id, state);
                                }
                                ServerMessage::ActiveDeviceChanged { device_id } => {
                                    publisher.set_active(device_id.clone(), stamp(&self.playback.borrow(), &self.namespace));
                                    publisher.last_sent = None;
                                }
                                ServerMessage::Settings { settings, version }
                                | ServerMessage::SettingsConflict { settings, version } => {
                                    publisher.remember_settings(settings, *version);
                                }
                                ServerMessage::Pong => continue,
                                _ => {}
                            }
                            if !self.emit(RelayEvent::Incoming(msg)) { return None; }
                        }
                        Frame::Control => {}
                    }
                }
                message = outgoing.recv() => socket.send(message?).await?,
                _ = timer.tick() => {
                    let now = Instant::now();
                    if now.duration_since(last_received) >= SILENCE_LIMIT { return None; }
                    if now.duration_since(last_heartbeat) >= HEARTBEAT {
                        socket.send(ClientMessage::Ping).await?;
                        last_heartbeat = now;
                    }
                    let sample = self.playback.borrow().clone();
                    if let Some(mut state) = publisher.update(&self.device_id, &self.namespace, &sample, now) {
                        state.captured_at_ms = super::wall_ms();
                        publisher.remember(&self.device_id, &state);
                        socket.send(ClientMessage::StateUpdate { state }).await?;
                    }
                }
            }
        }
    }
}

fn retry_at(attempts: &mut u32) -> Instant {
    let max_ms = (500u64 << *attempts).min(30_000);
    *attempts = (*attempts + 1).min(6);
    Instant::now() + Duration::from_millis(rand::random_range(max_ms / 2..=max_ms))
}

impl SyncTransport for RelayTransport {
    type Link = Socket;

    async fn connect(&self) -> Option<(Self::Link, ServerMessage)> {
        let config = WebSocketConfig::default()
            .read_buffer_size(16 * 1024)
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (mut socket, _) =
            connect_async_with_config(self.endpoint.as_str(), Some(config), false)
                .await
                .ok()?;
        SyncLink::send(&mut socket, ClientMessage::Hello(self.hello.clone())).await?;
        let Frame::Data(welcome) = socket.receive().await? else {
            return None;
        };
        matches!(welcome, ServerMessage::Welcome { .. }).then_some((socket, welcome))
    }
}

impl SyncLink for Socket {
    async fn send(&mut self, message: ClientMessage) -> Option<()> {
        let text = serde_json::to_string(&message).ok()?;
        time::timeout(
            CONNECT_TIMEOUT,
            SinkExt::send(self, Message::Text(text.into())),
        )
        .await
        .ok()?
        .ok()
    }

    async fn receive(&mut self) -> Option<Frame> {
        match self.next().await?.ok()? {
            Message::Text(text) => Some(Frame::Data(serde_json::from_str(text.as_str()).ok()?)),
            Message::Ping(_) | Message::Pong(_) => Some(Frame::Control),
            _ => None,
        }
    }
}

/// What identifies a change in local playback: queue revision, index, playing, buffering.
type Stamp = (u64, Option<usize>, bool, bool);

fn stamp(sample: &PlaybackSnapshot, namespace: &str) -> Option<Stamp> {
    let observed = sample
        .sync
        .as_ref()
        .filter(|observed| observed.namespace == namespace)?;
    Some((
        observed.revision,
        sample.index,
        sample.playing,
        sample.buffering,
    ))
}

#[derive(Default)]
struct Publisher {
    active: Option<String>,
    latest_seq: Option<u64>,
    snapshot: Option<ReconciledPlayback>,
    settings: Option<VersionedSettings>,
    stamp: Option<Stamp>,
    changed: Option<Instant>,
    last_sent: Option<Instant>,
    /// This device's playback when another device became active. Playing
    /// something new here after that takes over; leftover playback does not.
    yielded: Option<Stamp>,
    taking_over: bool,
}

impl Publisher {
    fn remember_settings(&mut self, settings: &serde_json::Value, version: u64) {
        if self
            .settings
            .as_ref()
            .is_none_or(|known| version > known.version)
            && let Some(settings) = settings.as_object()
        {
            self.settings = Some(VersionedSettings {
                settings: settings.clone(),
                version,
            });
        }
    }

    fn remember(&mut self, device_id: &str, state: &PlaybackState) {
        if self
            .snapshot
            .as_ref()
            .is_none_or(|known| state.seq >= known.state.seq)
        {
            self.snapshot = Some(ReconciledPlayback {
                device_id: device_id.to_owned(),
                state: state.clone(),
            });
        }
    }

    /// `current` is this device's playback at the moment, kept as the baseline
    /// that must change before playing here takes over from another device.
    fn set_active(&mut self, active: Option<String>, current: Option<Stamp>) {
        self.active = active;
        self.yielded = current;
        self.taking_over = false;
    }

    fn update(
        &mut self,
        device: &str,
        namespace: &str,
        sample: &PlaybackSnapshot,
        now: Instant,
    ) -> Option<PlaybackState> {
        let observed = sample
            .sync
            .as_ref()
            .filter(|observed| observed.namespace == namespace)?;
        let stamp = stamp(sample, namespace)?;
        let active = self.active.as_deref();
        if active.is_some_and(|active| active != device) && !self.taking_over {
            // Another device is playing. Starting playback here takes over;
            // what was playing when it took over (and is being paused) does not.
            if !sample.playing {
                self.yielded = Some(stamp);
                return None;
            }
            if self.yielded == Some(stamp) {
                return None;
            }
            self.taking_over = true;
        }
        if active.is_none() && !sample.playing {
            return None;
        }
        if self.stamp != Some(stamp) {
            self.stamp = Some(stamp);
            self.changed = Some(now);
        }
        let changed = self
            .changed
            .is_some_and(|at| now.duration_since(at) >= DEBOUNCE);
        let periodic = sample.playing
            && self
                .last_sent
                .is_some_and(|at| now.duration_since(at) >= STATE_INTERVAL);
        if !(changed || periodic || (self.changed.is_none() && self.last_sent.is_none())) {
            return None;
        }
        let seq = self.latest_seq.unwrap_or(0).checked_add(1)?;
        let state = PlaybackState {
            queue: sample.queue.iter().map(|track| track.id.clone()).collect(),
            index: sample.index,
            position_ms: super::milliseconds(sample.position),
            playing: sample.playing,
            volume: observed.volume,
            shuffle: observed.shuffle,
            repeat: observed.repeat,
            captured_at_ms: 0,
            seq,
        };
        self.last_sent = Some(now);
        self.changed = None;
        if !state.valid() {
            return None;
        }
        self.latest_seq = Some(seq);
        Some(state)
    }
}
