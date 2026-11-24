//! Direct device-to-device sync on the local network.
//!
//! Every device listens on a remembered port and keeps one encrypted link to each
//! device trusted through the Navidrome device list. Addresses come from that list,
//! from multicast discovery, and from incoming links, so one reachable direction is
//! enough. [`Mesh`] owns the shared state; this module only moves bytes.

pub use super::lan_registry::Trusted;
use super::{
    lan_registry::{LanRegistry, MAX_ADDRESSES},
    lan_security::{Link, PublicKey, Socket, Trust},
    mesh::{Effect, Mesh, PeerMessage},
    transport::{Frame, SyncLink},
};
use anyhow::{Context, Result};
use futures_util::{StreamExt, stream::FuturesUnordered};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use sonora_sync_protocol::{ClientMessage, Device, ServerMessage};
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc, watch},
    task::JoinHandle,
    time::{self, Instant},
};
use tokio_tungstenite::{
    accept_hdr_async_with_config, client_async_with_config,
    tungstenite::{
        handshake::server::{Request, Response},
        http::StatusCode,
        protocol::WebSocketConfig,
    },
};

const TIMEOUT: Duration = Duration::from_secs(7);
const DIAL_TIMEOUT: Duration = Duration::from_millis(1500);
const REDIAL: Duration = Duration::from_secs(3);
/// How long a replaced duplicate link keeps reading messages already sent on it.
const REPLACED_GRACE: Duration = Duration::from_secs(1);
const HEARTBEAT: Duration = Duration::from_secs(15);
const SILENCE_LIMIT: Duration = Duration::from_secs(45);
const SERVICE_TYPE: &str = "_sonora._tcp.local.";
const PROTOCOL_VERSION: &str = "2";

#[derive(Clone, Debug, Default)]
pub struct LanStatus {
    pub port: u16,
    /// Every trusted device and whether this device can reach it.
    pub devices: Vec<LanDevice>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanDevice {
    pub id: String,
    pub name: String,
    pub reach: LanReach,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LanReach {
    #[default]
    NotFound,
    Found(SocketAddr),
    Connected(SocketAddr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LanPeer {
    id: String,
    address: SocketAddr,
}

/// How the LAN listener picks its port.
pub(crate) enum Port {
    /// Exactly this port; 0 picks any free port each time.
    Fixed(u16),
    /// The port saved in this file, or a new free one that is then saved.
    Remembered(PathBuf),
}

impl Port {
    async fn bind(self) -> Result<TcpListener> {
        let path = match self {
            Self::Fixed(port) => {
                return Ok(TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?);
            }
            Self::Remembered(path) => path,
        };
        let saved = std::fs::read_to_string(&path)
            .ok()
            .and_then(|port| port.trim().parse::<u16>().ok())
            .filter(|port| *port != 0);
        if let Some(port) = saved
            && let Ok(listener) = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await
        {
            return Ok(listener);
        }
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
        crate::platform::atomic_write(&path, listener.local_addr()?.port().to_string().as_bytes())?;
        Ok(listener)
    }
}

struct Slot {
    serial: u64,
    /// The device that dialed this link; duplicates keep the lower ID's dial.
    dialer: String,
    address: SocketAddr,
    out: mpsc::Sender<PeerMessage>,
    close: watch::Sender<bool>,
}

struct Network {
    own: Device,
    port: u16,
    identity: [u8; 32],
    group: [u8; 32],
    registry: Option<LanRegistry>,
    /// Trust supplied directly, used only without a Navidrome device list.
    fixed: Mutex<Vec<Trusted>>,
    discovery: Mutex<HashMap<String, Vec<LanPeer>>>,
    change: watch::Sender<()>,
    links: Mutex<HashMap<String, Slot>>,
    dialing: Mutex<HashSet<String>>,
    serial: AtomicU64,
    mesh: Mutex<Mesh>,
    session: Mutex<Option<mpsc::Sender<ServerMessage>>>,
}

pub struct LanNode {
    network: Arc<Network>,
    tasks: Vec<JoinHandle<()>>,
    mdns: Option<(ServiceDaemon, JoinHandle<()>)>,
}

impl LanNode {
    /// Starts a node that trusts exactly `trusted`, without a Navidrome device list.
    pub async fn listen(
        device: Device,
        identity: [u8; 32],
        group: [u8; 32],
        port: u16,
        trusted: Vec<Trusted>,
    ) -> Result<Self> {
        Self::start(device, identity, group, Port::Fixed(port), None, trusted).await
    }

    pub(crate) async fn start(
        device: Device,
        identity: [u8; 32],
        group: [u8; 32],
        port: Port,
        registry: Option<LanRegistry>,
        fixed: Vec<Trusted>,
    ) -> Result<Self> {
        let listener = port.bind().await?;
        // Test builds never use multicast discovery, so scenarios prove the Navidrome addresses alone.
        let discover = registry.is_some() && !cfg!(feature = "test-support");
        let network = Arc::new(Network {
            mesh: Mutex::new(Mesh::new(device.clone())),
            own: device,
            port: listener.local_addr()?.port(),
            identity,
            group,
            registry,
            fixed: Mutex::new(fixed),
            discovery: Mutex::default(),
            change: watch::channel(()).0,
            links: Mutex::default(),
            dialing: Mutex::default(),
            serial: AtomicU64::new(0),
            session: Mutex::default(),
        });
        network.publish_addresses();
        let tasks = vec![
            tokio::spawn(accept_links(network.clone(), listener)),
            tokio::spawn(dial_links(network.clone())),
        ];
        let mdns = discover
            .then(|| discover_peers(network.clone()).ok())
            .flatten();
        Ok(Self {
            network,
            tasks,
            mdns,
        })
    }

    /// A local address of this node's listener, for tests on one machine.
    pub fn address(&self) -> SocketAddr {
        (Ipv4Addr::LOCALHOST, self.network.port).into()
    }

    pub fn paired_count(&self) -> usize {
        self.network.trusted().len()
    }

    /// IDs of the devices this node currently has a live link with.
    pub fn linked(&self) -> Vec<String> {
        let mut linked: Vec<_> = self
            .network
            .links
            .lock()
            .map(|links| links.keys().cloned().collect())
            .unwrap_or_default();
        linked.sort();
        linked
    }

    pub(crate) fn status(&self) -> LanStatus {
        let links: HashMap<String, SocketAddr> = self
            .network
            .links
            .lock()
            .map(|links| {
                links
                    .iter()
                    .map(|(id, slot)| (id.clone(), slot.address))
                    .collect()
            })
            .unwrap_or_default();
        let mut devices: Vec<_> = self
            .network
            .trusted()
            .into_values()
            .map(|trusted| LanDevice {
                reach: match links.get(&trusted.device.id) {
                    Some(address) => LanReach::Connected(*address),
                    None => self
                        .network
                        .addresses(&trusted)
                        .first()
                        .map_or(LanReach::NotFound, |address| LanReach::Found(*address)),
                },
                id: trusted.device.id,
                name: trusted.device.name,
            })
            .collect();
        devices.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        LanStatus {
            port: self.network.port,
            devices,
        }
    }

    pub(crate) fn changes(&self) -> watch::Receiver<()> {
        self.network.change.subscribe()
    }

    pub fn revoke(&self, id: &str) -> Result<()> {
        if let Some(registry) = &self.network.registry {
            registry.revoke(id)?;
        }
        if let Ok(mut fixed) = self.network.fixed.lock() {
            fixed.retain(|trusted| trusted.device.id != id);
        }
        self.network.change.send_replace(());
        Ok(())
    }

    pub(crate) async fn refresh_registry(&self) {
        self.network.refresh_registry().await;
    }

    /// Opens this device's sync session over its LAN links. A newer session
    /// replaces an older one.
    pub fn session(&self) -> (LanSession, ServerMessage) {
        let (sender, inbox) = mpsc::channel(256);
        if let Ok(mut session) = self.network.session.lock() {
            *session = Some(sender.clone());
        }
        let welcome = self
            .network
            .mesh
            .lock()
            .map(|mesh| mesh.welcome())
            .unwrap_or(ServerMessage::Welcome {
                devices: Vec::new(),
                active_device_id: None,
                state: None,
                settings: serde_json::json!({}),
                settings_version: 0,
            });
        (
            LanSession {
                network: self.network.clone(),
                sender,
                inbox,
            },
            welcome,
        )
    }
}

impl Drop for LanNode {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        if let Ok(links) = self.network.links.lock() {
            for slot in links.values() {
                slot.close.send_replace(true);
            }
        }
        if let Some((daemon, task)) = self.mdns.take() {
            task.abort();
            let _ = daemon.shutdown();
        }
    }
}

/// Domain separation for LAN handshakes: the devices of one Navidrome user,
/// whichever server address each of them signed in with.
pub fn user_hash(account: &crate::platform::Account) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"sonora-lan-user:");
    hash.update(account.username.trim().to_lowercase().as_bytes());
    hash.finalize().into()
}

/// This device's sync session over the LAN mesh.
pub struct LanSession {
    network: Arc<Network>,
    sender: mpsc::Sender<ServerMessage>,
    inbox: mpsc::Receiver<ServerMessage>,
}

impl LanSession {
    pub fn send(&mut self, message: ClientMessage) {
        let effects = self
            .network
            .mesh
            .lock()
            .map(|mut mesh| mesh.session_message(message, Instant::now()))
            .unwrap_or_default();
        self.network.apply(effects);
    }

    pub async fn receive(&mut self) -> Option<ServerMessage> {
        loop {
            let deadline = self
                .network
                .mesh
                .lock()
                .ok()
                .and_then(|mesh| mesh.next_deadline());
            let expiry = async {
                match deadline {
                    Some(deadline) => time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                message = self.inbox.recv() => return message,
                () = expiry => {
                    let effects = self
                        .network
                        .mesh
                        .lock()
                        .map(|mut mesh| mesh.expire(Instant::now()))
                        .unwrap_or_default();
                    self.network.apply(effects);
                }
            }
        }
    }
}

impl Drop for LanSession {
    fn drop(&mut self) {
        if let Ok(mut session) = self.network.session.lock()
            && session
                .as_ref()
                .is_some_and(|current| current.same_channel(&self.sender))
        {
            *session = None;
        }
    }
}

impl SyncLink for LanSession {
    async fn send(&mut self, message: ClientMessage) -> Option<()> {
        LanSession::send(self, message);
        Some(())
    }

    async fn receive(&mut self) -> Option<Frame> {
        LanSession::receive(self).await.map(Frame::Data)
    }
}

impl Network {
    fn revoked(&self, id: &str) -> bool {
        self.registry
            .as_ref()
            .is_some_and(|registry| registry.revoked(id))
    }

    fn trusted(&self) -> HashMap<PublicKey, Trusted> {
        let devices = match &self.registry {
            Some(registry) => registry.devices(),
            None => self
                .fixed
                .lock()
                .map(|fixed| fixed.clone())
                .unwrap_or_default(),
        };
        devices
            .into_iter()
            .filter(|trusted| trusted.device.id != self.own.id && !self.revoked(&trusted.device.id))
            .map(|trusted| (trusted.key, trusted))
            .collect()
    }

    fn has_key(&self, key: &PublicKey) -> bool {
        self.trusted().contains_key(key)
    }

    /// Where a trusted device may be listening: what discovery found, then what it published.
    fn addresses(&self, trusted: &Trusted) -> Vec<SocketAddr> {
        let mut addresses: Vec<_> = self
            .discovery
            .lock()
            .map(|found| {
                found
                    .values()
                    .flatten()
                    .filter(|peer| peer.id == trusted.device.id)
                    .map(|peer| peer.address)
                    .collect()
            })
            .unwrap_or_default();
        for address in &trusted.addresses {
            if !addresses.contains(address) {
                addresses.push(*address);
            }
        }
        addresses
    }

    fn publish_addresses(&self) {
        if let Some(registry) = &self.registry {
            registry.set_addresses(local_addresses(self.port));
        }
    }

    async fn refresh_registry(&self) {
        self.publish_addresses();
        if let Some(registry) = &self.registry
            && registry.refresh().await.unwrap_or(false)
        {
            self.change.send_replace(());
        }
    }

    fn apply(&self, effects: Vec<Effect>) {
        if effects.is_empty() {
            return;
        }
        let session = self.session.lock().ok().and_then(|session| session.clone());
        let Ok(links) = self.links.lock() else {
            return;
        };
        for effect in effects {
            match effect {
                Effect::Session(message) => {
                    if let Some(session) = &session {
                        let _ = session.try_send(message);
                    }
                }
                Effect::Peer(id, message) => {
                    if let Some(slot) = links.get(&id)
                        && slot.out.try_send(message).is_err()
                    {
                        slot.close.send_replace(true);
                    }
                }
                Effect::Broadcast(message) => {
                    for slot in links.values() {
                        if slot.out.try_send(message.clone()).is_err() {
                            slot.close.send_replace(true);
                        }
                    }
                }
            }
        }
    }

    /// Runs one authenticated link until it closes, then forgets it.
    /// Applies messages the peer already sent on a link this device is dropping.
    async fn drain(&self, link: &mut Link, id: &str) -> Result<()> {
        while let Ok(Ok(data)) = time::timeout(REPLACED_GRACE, link.recv()).await {
            let message: PeerMessage = serde_json::from_slice(&data)?;
            let effects = self
                .mesh
                .lock()
                .map(|mut mesh| mesh.peer_message(id, message))
                .unwrap_or_default();
            self.apply(effects);
        }
        Ok(())
    }

    async fn run_link(
        self: Arc<Self>,
        mut link: Link,
        key: PublicKey,
        dialer: String,
        address: impl FnOnce(u16) -> SocketAddr,
    ) -> Result<()> {
        let hello = self
            .mesh
            .lock()
            .map_err(|_| anyhow::anyhow!("LAN sync unavailable"))?
            .hello(self.port);
        time::timeout(TIMEOUT, link.send(&serde_json::to_vec(&hello)?)).await??;
        let hello: PeerMessage =
            serde_json::from_slice(&time::timeout(TIMEOUT, link.recv()).await??)?;
        let PeerMessage::Hello { device, port, .. } = &hello else {
            anyhow::bail!("Expected a LAN hello");
        };
        let trusted = self
            .trusted()
            .remove(&key)
            .context("Untrusted LAN device")?;
        anyhow::ensure!(
            device.id == trusted.device.id && *port != 0,
            "LAN device mismatch"
        );
        let id = trusted.device.id.clone();
        let address = address(*port);
        let serial = self.serial.fetch_add(1, Ordering::Relaxed);
        let (out, mut outgoing) = mpsc::channel(256);
        let (close, mut closed) = watch::channel(false);
        let duplicate = {
            let mut links = self
                .links
                .lock()
                .map_err(|_| anyhow::anyhow!("LAN sync unavailable"))?;
            // Both devices may dial at once; both keep the link the lower ID dialed.
            let preferred = std::cmp::min(&self.own.id, &id).clone();
            match links.get(&id) {
                Some(existing) if existing.dialer == preferred || dialer != preferred => true,
                existing => {
                    if let Some(existing) = existing {
                        existing.close.send_replace(true);
                    }
                    links.insert(
                        id.clone(),
                        Slot {
                            serial,
                            dialer,
                            address,
                            out,
                            close,
                        },
                    );
                    false
                }
            }
        };
        if duplicate {
            // The peer may not have seen the kept link yet and is using this one.
            self.drain(&mut link, &id).await?;
            return Ok(());
        }
        let effects = self
            .mesh
            .lock()
            .map(|mut mesh| mesh.linked(trusted.device.clone(), hello))
            .unwrap_or_default();
        self.apply(effects);
        self.change.send_replace(());

        let mut trust = self.change.subscribe();
        let mut heartbeat = time::interval(HEARTBEAT);
        heartbeat.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let result = async {
            loop {
                tokio::select! {
                    biased;
                    () = async { let _ = closed.wait_for(|closed| *closed).await; } => {
                        // A newer link replaced this one. Deliver what is already in
                        // flight both ways so nothing sent during the switch is lost.
                        while let Ok(message) = outgoing.try_recv() {
                            time::timeout(TIMEOUT, link.send(&serde_json::to_vec(&message)?)).await??;
                        }
                        self.drain(&mut link, &id).await?;
                        return Ok(());
                    }
                    changed = trust.changed() => {
                        if changed.is_err() || !self.has_key(&key) {
                            return Ok(());
                        }
                    }
                    message = outgoing.recv() => {
                        let Some(message) = message else { return Ok(()) };
                        time::timeout(TIMEOUT, link.send(&serde_json::to_vec(&message)?)).await??;
                    }
                    _ = heartbeat.tick() => {
                        let ping = serde_json::to_vec(&PeerMessage::Ping)?;
                        time::timeout(TIMEOUT, link.send(&ping)).await??;
                    }
                    data = time::timeout(SILENCE_LIMIT, link.recv()) => {
                        let message: PeerMessage = serde_json::from_slice(&data??)?;
                        let effects = self
                            .mesh
                            .lock()
                            .map(|mut mesh| mesh.peer_message(&id, message))
                            .unwrap_or_default();
                        self.apply(effects);
                    }
                }
            }
        }
        .await;

        let current = self.links.lock().is_ok_and(|mut links| {
            let current = links.get(&id).is_some_and(|slot| slot.serial == serial);
            if current {
                links.remove(&id);
            }
            current
        });
        if current {
            let effects = self
                .mesh
                .lock()
                .map(|mut mesh| mesh.unlinked(&id))
                .unwrap_or_default();
            self.apply(effects);
            self.change.send_replace(());
        }
        result
    }

    #[allow(clippy::result_large_err)]
    async fn accept(self: Arc<Self>, stream: TcpStream) -> Result<()> {
        let ip = stream.peer_addr()?.ip();
        let (link, key) = time::timeout(TIMEOUT, async {
            let socket = accept_hdr_async_with_config(
                stream,
                |req: &Request, resp: Response| match req.uri().path() {
                    "/ws" => Ok(resp),
                    _ => {
                        let mut rejected =
                            tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(
                                None,
                            );
                        *rejected.status_mut() = StatusCode::NOT_FOUND;
                        Err(rejected)
                    }
                },
                Some(socket_config()),
            )
            .await?;
            let network = self.clone();
            Ok::<_, anyhow::Error>(
                Link::establish_with(
                    socket,
                    false,
                    &self.group,
                    &self.identity,
                    |key| async move {
                        if !network.has_key(&key) {
                            network.refresh_registry().await;
                        }
                        network.has_key(&key)
                    },
                )
                .await?,
            )
        })
        .await??;
        let dialer = self
            .trusted()
            .remove(&key)
            .context("Untrusted LAN device")?
            .device
            .id;
        self.run_link(link, key, dialer, |port| (ip, port).into())
            .await
    }

    /// Tries every known address of `trusted` at once and keeps the first link.
    async fn dial(self: Arc<Self>, trusted: Trusted) {
        let mut attempts: FuturesUnordered<_> = self
            .addresses(&trusted)
            .into_iter()
            .map(|address| {
                let network = self.clone();
                async move {
                    time::timeout(DIAL_TIMEOUT, async {
                        let socket = dial(address, "/ws").await.ok()?;
                        let (link, key) = Link::establish(
                            socket,
                            true,
                            &network.group,
                            &network.identity,
                            Trust::Paired(&[trusted.key]),
                        )
                        .await
                        .ok()?;
                        Some((link, key, address))
                    })
                    .await
                    .ok()
                    .flatten()
                }
            })
            .collect();
        let mut connected = None;
        while let Some(attempt) = attempts.next().await {
            if attempt.is_some() {
                connected = attempt;
                break;
            }
        }
        drop(attempts);
        if let Ok(mut dialing) = self.dialing.lock() {
            dialing.remove(&trusted.device.id);
        }
        if let Some((link, key, address)) = connected {
            let own = self.own.id.clone();
            let _ = self.run_link(link, key, own, |_| address).await;
        }
    }
}

async fn accept_links(network: Arc<Network>, listener: TcpListener) {
    let limit = Arc::new(Semaphore::new(24));
    let mut sessions = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = sessions.join_next(), if !sessions.is_empty() => {}
            Ok((stream, _)) = listener.accept() => {
                if let Ok(permit) = limit.clone().try_acquire_owned() {
                    let network = network.clone();
                    sessions.spawn(async move {
                        let _permit = permit;
                        let _ = network.accept(stream).await;
                    });
                }
            }
        }
    }
}

/// Keeps dialing trusted devices that have no live link.
async fn dial_links(network: Arc<Network>) {
    let mut changes = network.change.subscribe();
    let mut retry = time::interval(REDIAL);
    retry.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let mut dials = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = dials.join_next(), if !dials.is_empty() => continue,
            _ = retry.tick() => {}
            changed = changes.changed() => if changed.is_err() { return },
        }
        let linked: HashSet<String> = network
            .links
            .lock()
            .map(|links| links.keys().cloned().collect())
            .unwrap_or_default();
        for trusted in network.trusted().into_values() {
            let id = trusted.device.id.clone();
            if linked.contains(&id) || network.addresses(&trusted).is_empty() {
                continue;
            }
            let fresh = network
                .dialing
                .lock()
                .is_ok_and(|mut dialing| dialing.insert(id));
            if fresh {
                dials.spawn(network.clone().dial(trusted));
            }
        }
    }
}

/// The IPv4 endpoints other devices on the network may use to reach this listener.
fn local_addresses(port: u16) -> Vec<SocketAddr> {
    const VIRTUAL: [&str; 7] = [
        "docker", "br-", "veth", "virbr", "bridge", "vmnet", "vboxnet",
    ];
    let mut addresses: Vec<SocketAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| {
            !VIRTUAL
                .iter()
                .any(|prefix| interface.name.starts_with(prefix))
        })
        .filter_map(|interface| match interface.ip() {
            IpAddr::V4(ip) if ip.is_private() || is_shared(ip) => Some((ip, port).into()),
            _ => None,
        })
        .collect();
    // Test scenarios run every device on one machine without a private network.
    if cfg!(feature = "test-support") {
        addresses.insert(0, (Ipv4Addr::LOCALHOST, port).into());
    }
    addresses.sort_unstable();
    addresses.dedup();
    addresses.truncate(MAX_ADDRESSES);
    addresses
}

/// Carrier-grade NAT space, which Tailscale uses for device addresses.
fn is_shared(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..128).contains(&b)
}

fn discover_peers(network: Arc<Network>) -> Result<(ServiceDaemon, JoinHandle<()>)> {
    let hash = network
        .group
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let device = &network.own;
    let properties = [
        ("device_id", device.id.as_str()),
        ("device_name", device.name.as_str()),
        ("os", device.os.as_str()),
        ("proto_version", PROTOCOL_VERSION),
        ("user_hash", hash.as_str()),
    ];
    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &device.id,
        &format!("sonora-{}.local.", device.id),
        "",
        network.port,
        &properties[..],
    )?
    .enable_addr_auto();
    let daemon = ServiceDaemon::new()?;
    let events = match daemon
        .register(service)
        .and_then(|()| daemon.browse(SERVICE_TYPE))
    {
        Ok(events) => events,
        Err(error) => {
            let _ = daemon.shutdown();
            return Err(error.into());
        }
    };
    let task = tokio::spawn(async move {
        while let Ok(event) = events.recv_async().await {
            let changed = {
                let Ok(mut addresses) = network.discovery.lock() else {
                    break;
                };
                match event {
                    ServiceEvent::ServiceResolved(service)
                        if service.get_property_val_str("user_hash") == Some(hash.as_str())
                            && service.get_property_val_str("proto_version")
                                == Some(PROTOCOL_VERSION) =>
                    {
                        let Some(id) = service
                            .get_property_val_str("device_id")
                            .filter(|id| *id != network.own.id && id.len() <= 128)
                        else {
                            continue;
                        };
                        if addresses.len() >= 64 && !addresses.contains_key(service.get_fullname())
                        {
                            continue;
                        }
                        let endpoints = service
                            .get_addresses_v4()
                            .into_iter()
                            .map(|ip| LanPeer {
                                id: id.to_owned(),
                                address: (ip, service.get_port()).into(),
                            })
                            .collect();
                        addresses.insert(service.get_fullname().to_owned(), endpoints);
                        true
                    }
                    ServiceEvent::ServiceRemoved(_, name) => addresses.remove(&name).is_some(),
                    _ => false,
                }
            };
            if changed {
                network.change.send_replace(());
            }
        }
    });
    Ok((daemon, task))
}

fn socket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .max_message_size(Some(512 * 1024))
        .max_frame_size(Some(65_535))
}

async fn dial(address: SocketAddr, path: &str) -> Result<Socket> {
    let stream = TcpStream::connect(address).await?;
    let (socket, _) = client_async_with_config(
        format!("ws://{address}{path}"),
        stream,
        Some(socket_config()),
    )
    .await?;
    Ok(socket)
}
