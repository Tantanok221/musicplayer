use super::{
    lan_registry::LanRegistry,
    lan_security::{Link, PublicKey, Socket, Trust},
    transport::{Frame, SyncLink, SyncTransport},
};
use anyhow::{Context, Result, ensure};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sonora_sync::{LocalCoordinator, LocalPeer};
use sonora_sync_protocol::{
    ClientMessage, Device, ReconciledPlayback, ServerMessage, VersionedSettings,
};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Semaphore, watch},
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
const OFFER_TTL: Duration = Duration::from_secs(120);
const SERVICE_TYPE: &str = "_sonora._tcp.local.";
const PROTOCOL_VERSION: &str = "1";

#[derive(Clone)]
pub struct LanCode(pub String);

#[derive(Clone, Debug, Default)]
pub enum LanOfferState {
    #[default]
    Idle,
    Available(LanCode),
    UsedOrExpired,
}

impl std::fmt::Debug for LanCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LanCode([REDACTED])")
    }
}

pub(crate) enum PairingOffer {
    Idle,
    Available(LanCode, Instant),
    UsedOrExpired,
}

impl PairingOffer {
    pub(crate) fn state(&self) -> LanOfferState {
        match self {
            Self::Idle => LanOfferState::Idle,
            Self::Available(code, _) => LanOfferState::Available(code.clone()),
            Self::UsedOrExpired => LanOfferState::UsedOrExpired,
        }
    }

    fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Available(_, expires) => Some(*expires),
            _ => None,
        }
    }
}

async fn expire_offers(offers: &watch::Sender<PairingOffer>) {
    let mut changes = offers.subscribe();
    loop {
        let deadline = changes.borrow_and_update().deadline();
        let elapsed = async {
            match deadline {
                Some(deadline) => time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = elapsed => {
                offers.send_if_modified(|offer| {
                    if offer.deadline().is_some_and(|deadline| deadline <= Instant::now()) {
                        *offer = PairingOffer::UsedOrExpired;
                        true
                    } else {
                        false
                    }
                });
            }
            changed = changes.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LanStatus {
    pub port: u16,
    pub peers: Vec<LanPeer>,
    pub discovered: Vec<LanPeer>,
    pub automatic: Vec<LanDevice>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanDevice {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanPeer {
    pub id: String,
    pub name: String,
    pub address: SocketAddr,
}

#[derive(Deserialize, Serialize)]
struct Announcement {
    device: Device,
    port: u16,
}

#[derive(Clone, Serialize, Deserialize)]
struct Pin {
    device: Device,
    address: SocketAddr,
}

struct Network {
    announcement: Announcement,
    identity: [u8; 32],
    group: [u8; 32],
    pins: Mutex<HashMap<PublicKey, Pin>>,
    pins_path: Option<PathBuf>,
    registry: Option<LanRegistry>,
    discovery: Mutex<HashMap<String, Vec<LanPeer>>>,
    change: watch::Sender<()>,
    hosting: watch::Sender<Option<LocalCoordinator>>,
    offer: watch::Sender<PairingOffer>,
}

pub struct LanNode {
    network: Arc<Network>,
    task: JoinHandle<()>,
    mdns: Option<(ServiceDaemon, JoinHandle<()>)>,
}

impl LanNode {
    pub fn paired_count(&self) -> usize {
        self.network.trusted().len()
    }

    pub(crate) fn status(&self) -> LanStatus {
        let mut peers = self
            .network
            .pins
            .lock()
            .map(|pins| {
                pins.values()
                    .filter(|pin| !self.network.revoked(&pin.device.id))
                    .map(|pin| LanPeer {
                        id: pin.device.id.clone(),
                        name: pin.device.name.clone(),
                        address: pin.address,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        peers.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        let automatic: Vec<_> = self
            .network
            .automatic()
            .into_iter()
            .map(|(_, device)| LanDevice {
                id: device.id,
                name: device.name,
            })
            .collect();
        let mut discovered = self
            .network
            .discovery
            .lock()
            .map(|devices| {
                devices
                    .values()
                    .flatten()
                    .filter(|device| {
                        peers.iter().all(|pin| pin.id != device.id)
                            && automatic.iter().all(|peer| peer.id != device.id)
                            && !self.network.revoked(&device.id)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        discovered.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then(left.id.cmp(&right.id))
                .then(left.address.cmp(&right.address))
        });
        discovered.dedup_by(|left, right| left.id == right.id);
        LanStatus {
            port: self.address().port(),
            peers,
            discovered,
            automatic,
        }
    }

    pub(crate) fn changes(&self) -> watch::Receiver<()> {
        self.network.change.subscribe()
    }

    pub fn revoke(&self, id: &str) -> Result<()> {
        if let Some(registry) = &self.network.registry {
            registry.revoke(id)?;
        }
        let mut pins = self
            .network
            .pins
            .lock()
            .map_err(|_| anyhow::anyhow!("LAN pairing unavailable"))?;
        let mut updated = pins.clone();
        updated.retain(|_, pin| pin.device.id != id);
        self.network.save_pins(&updated)?;
        *pins = updated;
        self.network.change.send_replace(());
        Ok(())
    }

    pub(crate) async fn refresh_registry(&self) {
        self.network.refresh_registry().await;
    }

    pub async fn listen(
        device: Device,
        identity: [u8; 32],
        group: [u8; 32],
        port: u16,
    ) -> Result<Self> {
        Self::start(device, identity, group, port, None, None).await
    }

    pub(crate) async fn start(
        device: Device,
        identity: [u8; 32],
        group: [u8; 32],
        port: u16,
        pins_path: Option<PathBuf>,
        registry: Option<LanRegistry>,
    ) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?;
        let pins = if let Some(path) = &pins_path {
            match std::fs::read(path) {
                Ok(bytes) => serde_json::from_slice::<Vec<(PublicKey, Pin)>>(&bytes)?
                    .into_iter()
                    .collect(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
                Err(error) => return Err(error.into()),
            }
        } else {
            HashMap::new()
        };
        let discover = pins_path.is_some();
        let network = Arc::new(Network {
            announcement: Announcement {
                device,
                port: listener.local_addr()?.port(),
            },
            identity,
            group,
            pins: Mutex::new(pins),
            pins_path,
            registry,
            discovery: Mutex::default(),
            change: watch::channel(()).0,
            hosting: watch::channel(None).0,
            offer: watch::Sender::new(PairingOffer::Idle),
        });
        let accepted = network.clone();
        let task = tokio::spawn(async move {
            let limit = Arc::new(Semaphore::new(24));
            let mut sessions = tokio::task::JoinSet::new();
            let expiry = expire_offers(&accepted.offer);
            tokio::pin!(expiry);
            loop {
                tokio::select! {
                    _ = &mut expiry => break,
                    _ = sessions.join_next(), if !sessions.is_empty() => {}
                    Ok((stream, address)) = listener.accept() => {
                        if let Ok(permit) = limit.clone().try_acquire_owned() {
                            let network = accepted.clone();
                            sessions.spawn(async move {
                                let _permit = permit;
                                let _ = network.accept(stream, address.ip()).await;
                            });
                        }
                    }
                }
            }
        });
        let mdns = discover
            .then(|| discover_peers(network.clone()).ok())
            .flatten();
        Ok(Self {
            network,
            task,
            mdns,
        })
    }

    pub fn address(&self) -> SocketAddr {
        (Ipv4Addr::LOCALHOST, self.network.announcement.port).into()
    }

    pub fn offer_code(&self) -> String {
        let code = format!("{:06}", rand::rng().random_range(0..1_000_000));
        self.network.offer.send_replace(PairingOffer::Available(
            LanCode(code.clone()),
            Instant::now() + OFFER_TTL,
        ));
        code
    }

    pub(crate) fn offers(&self) -> watch::Receiver<PairingOffer> {
        self.network.offer.subscribe()
    }

    pub async fn pair(&self, address: SocketAddr, code: &str) -> Result<()> {
        time::timeout(TIMEOUT, async {
            let socket = dial(address, "/pair").await?;
            let (mut link, key) = Link::establish(
                socket,
                true,
                &self.network.group,
                &self.network.identity,
                Trust::Pair(code),
            )
            .await?;
            link.send(&serde_json::to_vec(&self.network.announcement)?)
                .await?;
            self.network.remember(
                address.ip(),
                key,
                serde_json::from_slice(&link.recv().await?)?,
            )
        })
        .await?
    }

    pub async fn connect(&self, address: SocketAddr) -> Result<(LanConnection, ServerMessage)> {
        if address == self.address() {
            return self
                .host(None, None)
                .await
                .context("LAN connection unavailable");
        }
        let pin = self
            .network
            .pins
            .lock()
            .ok()
            .and_then(|pins| {
                pins.iter()
                    .find_map(|(key, pin)| (pin.address == address).then_some(*key))
            })
            .or_else(|| {
                let discovered = self.network.discovery.lock().ok()?;
                self.network
                    .automatic()
                    .into_iter()
                    .find_map(|(key, device)| {
                        discovered
                            .values()
                            .flatten()
                            .any(|peer| peer.id == device.id && peer.address == address)
                            .then_some(key)
                    })
            });
        LanTransport {
            node: self,
            address,
            pin,
        }
        .connect()
        .await
        .context("LAN connection unavailable")
    }

    pub(crate) async fn preferred(
        &self,
        authority: Option<&str>,
        playback: Option<&ReconciledPlayback>,
        settings: Option<&VersionedSettings>,
    ) -> Option<(LanConnection, ServerMessage)> {
        if self.paired_count() == 0 {
            return None;
        }
        let own = &self.network.announcement.device.id;
        if authority == Some(own) {
            return self.host(playback, settings).await;
        }
        if let Some(preferred) = self.preferred_remote(authority).await {
            return Some(preferred);
        }
        if authority.is_some() {
            return None;
        }
        self.host(playback, settings).await
    }

    async fn host(
        &self,
        playback: Option<&ReconciledPlayback>,
        settings: Option<&VersionedSettings>,
    ) -> Option<(LanConnection, ServerMessage)> {
        let network = &self.network;
        let coordinator = LocalCoordinator::new().ok()?;
        let (peer, mut welcome) = coordinator
            .join(network.announcement.device.clone())
            .await
            .ok()?;
        let playback =
            playback.filter(|snapshot| snapshot.device_id == network.announcement.device.id);
        if playback.is_some() || settings.is_some() {
            peer.send(ClientMessage::Reconcile {
                playback: playback.cloned(),
                settings: settings.cloned(),
            })
            .await
            .ok()?;
            if let ServerMessage::Welcome {
                state,
                settings: shared,
                settings_version,
                ..
            } = &mut welcome
            {
                *state = playback.map(|snapshot| snapshot.state.clone());
                if let Some(settings) = settings {
                    *shared = settings.settings.clone().into();
                    *settings_version = settings.version;
                }
            }
        }
        network.hosting.send_replace(Some(coordinator));
        Some((
            LanConnection(Connection::Local(peer, network.hosting.clone())),
            welcome,
        ))
    }

    pub(crate) async fn preferred_remote(
        &self,
        authority: Option<&str>,
    ) -> Option<(LanConnection, ServerMessage)> {
        let options = {
            let trusted = self.network.trusted();
            let own = &self.network.announcement.device.id;
            let discovered = self.network.discovery.lock().ok()?;
            let mut options = Vec::new();
            for (key, (device, fallback)) in trusted
                .into_iter()
                .filter(|(_, (device, _))| authority.map_or(&device.id < own, |id| device.id == id))
            {
                let addresses = discovered
                    .values()
                    .flatten()
                    .filter(|peer| peer.id == device.id)
                    .map(|peer| peer.address);
                options.extend(
                    addresses
                        .chain(fallback)
                        .map(|address| (device.id.clone(), key, address)),
                );
            }
            options.sort_unstable();
            options.dedup();
            options
        };
        time::timeout(Duration::from_secs(2), async {
            for (_, key, address) in options {
                let transport = LanTransport {
                    node: self,
                    address,
                    pin: Some(key),
                };
                if let Ok(Some(session)) =
                    time::timeout(Duration::from_millis(650), transport.connect()).await
                {
                    return Some(session);
                }
            }
            None
        })
        .await
        .ok()?
    }
}

pub fn user_hash(account: &crate::platform::Account) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(account.url.as_bytes());
    hash.update(account.username.as_bytes());
    hash.finalize().into()
}

impl Drop for LanNode {
    fn drop(&mut self) {
        self.task.abort();
        if let Some((daemon, task)) = self.mdns.take() {
            task.abort();
            let _ = daemon.shutdown();
        }
    }
}

fn discover_peers(network: Arc<Network>) -> Result<(ServiceDaemon, JoinHandle<()>)> {
    let hash = network
        .group
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let device = &network.announcement.device;
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
        network.announcement.port,
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
                        .filter(|id| *id != network.announcement.device.id && id.len() <= 128)
                    else {
                        continue;
                    };
                    if addresses.len() >= 64 && !addresses.contains_key(service.get_fullname()) {
                        continue;
                    }
                    let name = service.get_property_val_str("device_name").unwrap_or(id);
                    let endpoints = service
                        .get_addresses_v4()
                        .into_iter()
                        .map(|ip| LanPeer {
                            id: id.to_owned(),
                            name: name.to_owned(),
                            address: (ip, service.get_port()).into(),
                        })
                        .collect();
                    addresses.insert(service.get_fullname().to_owned(), endpoints);
                    network.change.send_replace(());
                }
                ServiceEvent::ServiceRemoved(_, name) if addresses.remove(&name).is_some() => {
                    network.change.send_replace(());
                }
                _ => {}
            }
        }
    });
    Ok((daemon, task))
}

impl Network {
    fn automatic(&self) -> Vec<(PublicKey, Device)> {
        self.registry
            .as_ref()
            .map_or_else(Vec::new, LanRegistry::devices)
    }

    fn revoked(&self, id: &str) -> bool {
        self.registry
            .as_ref()
            .is_some_and(|registry| registry.revoked(id))
    }

    fn trusted(&self) -> HashMap<PublicKey, (Device, Option<SocketAddr>)> {
        let automatic = self.automatic();
        let mut trusted: HashMap<_, _> = self
            .pins
            .lock()
            .map(|pins| {
                pins.iter()
                    .filter(|(key, pin)| {
                        !self.revoked(&pin.device.id)
                            && automatic
                                .iter()
                                .all(|(known, device)| device.id != pin.device.id || known == *key)
                    })
                    .map(|(key, pin)| (*key, (pin.device.clone(), Some(pin.address))))
                    .collect()
            })
            .unwrap_or_default();
        for (key, device) in automatic {
            let address = trusted.get(&key).and_then(|(_, address)| *address);
            trusted.insert(key, (device, address));
        }
        trusted
    }

    async fn refresh_registry(&self) {
        if let Some(registry) = &self.registry
            && registry.refresh().await.unwrap_or(false)
        {
            self.change.send_replace(());
        }
    }

    fn take_offer(&self) -> Option<LanCode> {
        let mut available = None;
        self.offer.send_if_modified(|offer| {
            let PairingOffer::Available(code, expires) = offer else {
                return false;
            };
            if Instant::now() < *expires {
                available = Some(code.clone());
            }
            *offer = PairingOffer::UsedOrExpired;
            true
        });
        available
    }

    fn remember(&self, ip: IpAddr, key: PublicKey, peer: Announcement) -> Result<()> {
        let device = peer.device;
        ensure!(
            peer.port != 0
                && !device.id.is_empty()
                && device.id.len() <= 128
                && !device.name.is_empty()
                && device.name.len() <= 128
                && device.os.len() <= 64,
            "Invalid LAN device"
        );
        ensure!(
            device.id != self.announcement.device.id && !self.revoked(&device.id),
            "Cannot pair with this LAN device"
        );
        ensure!(
            !self
                .automatic()
                .iter()
                .any(|(known, auto)| auto.id == device.id && known != &key),
            "LAN device identifier already has a trusted key"
        );
        let mut pins = self
            .pins
            .lock()
            .map_err(|_| anyhow::anyhow!("LAN pairing unavailable"))?;
        ensure!(
            !pins
                .iter()
                .any(|(known, pin)| known != &key && pin.device.id == device.id),
            "LAN device identifier is already paired"
        );
        let mut updated = pins.clone();
        updated.insert(
            key,
            Pin {
                device,
                address: (ip, peer.port).into(),
            },
        );
        self.save_pins(&updated)?;
        *pins = updated;
        self.change.send_replace(());
        Ok(())
    }

    fn save_pins(&self, pins: &HashMap<PublicKey, Pin>) -> Result<()> {
        ensure!(pins.len() <= 32, "Too many paired LAN devices");
        if let Some(path) = &self.pins_path {
            let data = serde_json::to_vec(&pins.iter().collect::<Vec<_>>())?;
            crate::platform::atomic_write(path, &data)?;
        }
        Ok(())
    }

    async fn accept(&self, stream: TcpStream, ip: IpAddr) -> Result<()> {
        let mut hosting = self.hosting.subscribe();
        let Some((mut link, mut peer, welcome, key)) =
            time::timeout(TIMEOUT, self.accept_link(stream, ip)).await??
        else {
            return Ok(());
        };
        let mut changed = self.change.subscribe();
        if !self.has_key(&key) || hosting.borrow_and_update().is_none() {
            return Ok(());
        }
        time::timeout(TIMEOUT, link.send(&serde_json::to_vec(&welcome)?)).await??;
        let idle = Duration::from_secs(45);
        loop {
            tokio::select! {
                biased;
                update = changed.changed() => {
                    if update.is_err() || !self.has_key(&key) { return Ok(()); }
                }
                message = peer.receive() => {
                    let Some(message) = message else { return Ok(()) };
                    time::timeout(TIMEOUT, link.send(&serde_json::to_vec(&message)?)).await??;
                }
                _ = hosting.changed() => return Ok(()),
                data = time::timeout(idle, link.recv()) => {
                    let mut message = serde_json::from_slice(&data??)?;
                    let own = &self.announcement.device.id;
                    match &mut message {
                        ClientMessage::Command { target_device_id, .. } if target_device_id != own => continue,
                        ClientMessage::Reconcile { playback, .. }
                            if playback.as_ref().is_some_and(|snapshot| &snapshot.device_id != own) => {
                            *playback = None;
                        }
                        _ => {}
                    }
                    peer.send(message).await?;
                }
            }
        }
    }

    #[allow(clippy::result_large_err)]
    async fn accept_link(
        &self,
        stream: TcpStream,
        ip: IpAddr,
    ) -> Result<Option<(Link, LocalPeer, ServerMessage, PublicKey)>> {
        let mut pairing = false;
        let socket = accept_hdr_async_with_config(
            stream,
            |req: &Request, resp: Response| match req.uri().path() {
                "/pair" => {
                    pairing = true;
                    Ok(resp)
                }
                "/ws" => Ok(resp),
                _ => {
                    let mut rejected =
                        tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(None);
                    *rejected.status_mut() = StatusCode::NOT_FOUND;
                    Err(rejected)
                }
            },
            Some(socket_config()),
        )
        .await?;
        if pairing {
            let code = self.take_offer().context("No active pairing code")?;
            let (mut link, key) = Link::establish(
                socket,
                false,
                &self.group,
                &self.identity,
                Trust::Pair(&code.0),
            )
            .await?;
            self.remember(ip, key, serde_json::from_slice(&link.recv().await?)?)?;
            link.send(&serde_json::to_vec(&self.announcement)?).await?;
            return Ok(None);
        }
        let (link, key) = Link::establish_with(
            socket,
            false,
            &self.group,
            &self.identity,
            None,
            |key| async move {
                if !self.has_key(&key) {
                    self.refresh_registry().await;
                }
                self.has_key(&key)
            },
        )
        .await?;
        let coordinator = self
            .hosting
            .borrow()
            .clone()
            .context("LAN coordinator is not active")?;
        let (device, _) = self.trusted().remove(&key).context("Unpaired LAN device")?;
        let (peer, welcome) = coordinator.join(device).await?;
        Ok(Some((link, peer, welcome, key)))
    }

    fn has_key(&self, key: &PublicKey) -> bool {
        self.trusted().contains_key(key)
    }
}

struct LanTransport<'a> {
    node: &'a LanNode,
    address: SocketAddr,
    pin: Option<PublicKey>,
}

impl SyncTransport for LanTransport<'_> {
    type Link = LanConnection;

    async fn connect(&self) -> Option<(Self::Link, ServerMessage)> {
        let network = &self.node.network;
        let pin = self.pin?;
        if !network.has_key(&pin) {
            return None;
        }
        time::timeout(TIMEOUT, async {
            let socket = dial(self.address, "/ws").await.ok()?;
            let (mut link, _) = Link::establish(
                socket,
                true,
                &network.group,
                &network.identity,
                Trust::Paired(&[pin]),
            )
            .await
            .ok()?;
            let welcome = serde_json::from_slice(&link.recv().await.ok()?).ok()?;
            matches!(welcome, ServerMessage::Welcome { .. }).then_some((
                LanConnection(Connection::Remote(Box::new(Remote {
                    link,
                    key: pin,
                    network: network.clone(),
                    changes: network.change.subscribe(),
                }))),
                welcome,
            ))
        })
        .await
        .ok()?
    }
}

pub struct LanConnection(Connection);

enum Connection {
    Local(LocalPeer, watch::Sender<Option<LocalCoordinator>>),
    Remote(Box<Remote>),
}

struct Remote {
    link: Link,
    key: PublicKey,
    network: Arc<Network>,
    changes: watch::Receiver<()>,
}

impl LanConnection {
    pub(crate) fn is_local(&self) -> bool {
        matches!(self.0, Connection::Local(..))
    }

    pub async fn send(&mut self, message: ClientMessage) -> Result<()> {
        match &mut self.0 {
            Connection::Local(peer, _) => peer.send(message).await?,
            Connection::Remote(remote) => {
                ensure!(
                    remote.network.has_key(&remote.key),
                    "LAN device was revoked"
                );
                time::timeout(TIMEOUT, remote.link.send(&serde_json::to_vec(&message)?)).await??
            }
        }
        Ok(())
    }

    pub async fn receive(&mut self) -> Result<ServerMessage> {
        match &mut self.0 {
            Connection::Local(peer, _) => peer.receive().await.context("LAN connection closed"),
            Connection::Remote(remote) => loop {
                ensure!(
                    remote.network.has_key(&remote.key),
                    "LAN device was revoked"
                );
                tokio::select! {
                    update = remote.changes.changed() => update?,
                    message = remote.link.recv() => return Ok(serde_json::from_slice(&message?)?),
                }
            },
        }
    }
}

impl Drop for LanConnection {
    fn drop(&mut self) {
        if let Connection::Local(_, hosting) = &self.0 {
            hosting.send_replace(None);
        }
    }
}

impl SyncLink for LanConnection {
    async fn send(&mut self, message: ClientMessage) -> Option<()> {
        self.send(message).await.ok()
    }

    async fn receive(&mut self) -> Option<Frame> {
        self.receive().await.ok().map(Frame::Data)
    }
}

fn socket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .max_message_size(Some(65_535))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unused_offer_expires_and_notifies_subscribers() {
        let (offer, mut updates) = watch::channel(PairingOffer::Available(
            LanCode("123456".into()),
            Instant::now() + Duration::from_millis(10),
        ));
        time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                _ = expire_offers(&offer) => panic!("offer expiry watcher stopped"),
                result = updates.wait_for(|offer| matches!(offer, PairingOffer::UsedOrExpired)) => {
                    assert!(result.is_ok());
                }
            }
        })
        .await
        .expect("unused code should expire");
        assert!(matches!(
            updates.borrow().state(),
            LanOfferState::UsedOrExpired
        ));
    }
}
