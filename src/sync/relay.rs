use crate::{model::PlaybackSnapshot, service::Command};
use futures_util::{SinkExt, StreamExt};
use sonora_sync_protocol::{ClientMessage, Hello, PlaybackState, ServerMessage};
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

pub(crate) struct RelayTask {
    pub endpoint: Url,
    pub hello: Hello,
    pub namespace: String,
    pub service: flume::Sender<Command>,
    pub generation: u64,
    pub playback: watch::Receiver<PlaybackSnapshot>,
}

impl RelayTask {
    pub async fn run(self) {
        let mut attempts = 0;
        loop {
            if let Ok(Some((socket, welcome))) =
                time::timeout(CONNECT_TIMEOUT, self.connect()).await
            {
                let (out, incoming) = mpsc::channel(32);
                if !self.emit(RelayEvent::Online {
                    welcome: welcome.clone(),
                    out,
                }) {
                    break;
                }
                self.session(socket, incoming, welcome).await;
                if !self.emit(RelayEvent::Offline) {
                    break;
                }
                attempts = 0;
            }
            let max_ms = (500u64 << attempts).min(30_000);
            time::sleep(Duration::from_millis(rand::random_range(
                max_ms / 2..=max_ms,
            )))
            .await;
            attempts = (attempts + 1).min(6);
        }
    }

    fn emit(&self, event: RelayEvent) -> bool {
        self.service
            .send(Command::DeviceSync(self.generation, event))
            .is_ok()
    }

    async fn connect(&self) -> Option<(Socket, ServerMessage)> {
        let config = WebSocketConfig::default()
            .read_buffer_size(16 * 1024)
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (mut socket, _) =
            connect_async_with_config(self.endpoint.as_str(), Some(config), false)
                .await
                .ok()?;
        send(&mut socket, ClientMessage::Hello(self.hello.clone())).await?;
        let Message::Text(text) = socket.next().await?.ok()? else {
            return None;
        };
        let welcome = serde_json::from_str(text.as_str()).ok()?;
        matches!(welcome, ServerMessage::Welcome { .. }).then_some((socket, welcome))
    }

    async fn session(
        &self,
        mut socket: Socket,
        mut outgoing: mpsc::Receiver<ClientMessage>,
        welcome: ServerMessage,
    ) -> Option<()> {
        let ServerMessage::Welcome {
            active_device_id,
            state,
            ..
        } = welcome
        else {
            return None;
        };
        let mut publisher = Publisher {
            active: active_device_id,
            latest_seq: state.map(|state| state.seq),
            ..Default::default()
        };
        let mut timer = time::interval(Duration::from_millis(100));
        timer.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut last_received = Instant::now();
        let mut last_heartbeat = Instant::now();
        loop {
            tokio::select! {
                frame = socket.next() => {
                    last_received = Instant::now();
                    match frame?.ok()? {
                        Message::Text(text) => {
                            let Ok(msg) = serde_json::from_str(text.as_str()) else { return None };
                            match &msg {
                                ServerMessage::State { state, .. } => {
                                    publisher.latest_seq = Some(publisher.latest_seq.map_or(state.seq, |seq| seq.max(state.seq)));
                                }
                                ServerMessage::ActiveDeviceChanged { device_id } => {
                                    publisher.active.clone_from(device_id);
                                    publisher.last_sent = None;
                                }
                                ServerMessage::Pong => continue,
                                _ => {}
                            }
                            if !self.emit(RelayEvent::Incoming(msg)) { return None; }
                        }
                        Message::Ping(_) | Message::Pong(_) => {}
                        _ => return None,
                    }
                }
                message = outgoing.recv() => send(&mut socket, message?).await?,
                _ = timer.tick() => {
                    let now = Instant::now();
                    if now.duration_since(last_received) >= SILENCE_LIMIT { return None; }
                    if now.duration_since(last_heartbeat) >= HEARTBEAT {
                        send(&mut socket, ClientMessage::Ping).await?;
                        last_heartbeat = now;
                    }
                    let sample = self.playback.borrow().clone();
                    if let Some(state) = publisher.update(&self.hello.device_id, &self.namespace, &sample, now) {
                        send(&mut socket, ClientMessage::StateUpdate { state }).await?;
                    }
                }
            }
        }
    }
}

async fn send(socket: &mut Socket, msg: ClientMessage) -> Option<()> {
    let text = serde_json::to_string(&msg).ok()?;
    time::timeout(CONNECT_TIMEOUT, socket.send(Message::Text(text.into())))
        .await
        .ok()?
        .ok()
}

#[derive(Default)]
struct Publisher {
    active: Option<String>,
    latest_seq: Option<u64>,
    stamp: Option<(u64, Option<usize>, bool, bool)>,
    changed: Option<Instant>,
    last_sent: Option<Instant>,
}

impl Publisher {
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
        let active = self.active.as_deref();
        if active.is_some_and(|active| active != device) || (active.is_none() && !sample.playing) {
            return None;
        }
        let stamp = (
            observed.revision,
            sample.index,
            sample.playing,
            sample.buffering,
        );
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
