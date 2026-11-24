//! Device-to-device sync state for the LAN, independent of sockets.
//!
//! Every device stays authoritative for its own player. Linked devices exchange
//! [`PeerMessage`]s; the device that most recently started playing holds the
//! active [`Claim`]. [`Mesh`] turns peer and local-session input into
//! [`Effect`]s, which the LAN layer delivers.

use super::{position_at, wall_ms};
use serde::{Deserialize, Serialize};
use sonora_sync_protocol::{
    ClientMessage, Device, PlaybackCommand, PlaybackState, ServerMessage, TRANSFER_TIMEOUT,
    VersionedSettings, valid_settings,
};
use std::collections::{BTreeMap, HashMap};
use tokio::time::Instant;

/// The active device. Higher claims win; ties go to the higher device ID.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Claim {
    pub epoch: u64,
    pub device_id: String,
}

/// Shared preferences with the device that produced this version, for tie-breaks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Settings {
    pub value: VersionedSettings,
    pub origin: String,
}

impl Settings {
    fn newer_than(&self, other: &Self) -> bool {
        (self.value.version, &self.origin) > (other.value.version, &other.origin)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum PeerMessage {
    Hello {
        device: Device,
        port: u16,
        claim: Option<Claim>,
        state: Option<PlaybackState>,
        settings: Settings,
    },
    Claim {
        claim: Claim,
    },
    State {
        state: PlaybackState,
    },
    Command {
        cmd: PlaybackCommand,
    },
    Transfer {
        transfer_id: String,
        state: PlaybackState,
        start_playing: bool,
    },
    TransferResult {
        transfer_id: String,
        ok: bool,
        error: Option<String>,
    },
    Settings {
        settings: Settings,
    },
    Ping,
    Pong,
}

#[derive(Debug)]
pub(crate) enum Effect {
    /// Deliver to this device's sync session.
    Session(ServerMessage),
    /// Send to one linked device.
    Peer(String, PeerMessage),
    /// Send to every linked device.
    Broadcast(PeerMessage),
}

struct OutgoingTransfer {
    target: String,
    deadline: Instant,
}

pub(crate) struct Mesh {
    own: Device,
    peers: BTreeMap<String, Device>,
    claim: Option<Claim>,
    states: HashMap<String, PlaybackState>,
    settings: Settings,
    transfers: HashMap<String, OutgoingTransfer>,
    /// Transfers this device was asked to take: source device and handed-over state.
    incoming: HashMap<String, (String, PlaybackState)>,
    next_transfer: u64,
}

impl Mesh {
    pub fn new(own: Device) -> Self {
        Self {
            own,
            peers: BTreeMap::new(),
            claim: None,
            states: HashMap::new(),
            settings: Settings::default(),
            transfers: HashMap::new(),
            incoming: HashMap::new(),
            next_transfer: 0,
        }
    }

    /// The active device, if it is this device or a linked one.
    pub fn active(&self) -> Option<&str> {
        self.claim
            .as_ref()
            .map(|claim| claim.device_id.as_str())
            .filter(|id| *id == self.own.id || self.peers.contains_key(*id))
    }

    pub fn hello(&self, port: u16) -> PeerMessage {
        PeerMessage::Hello {
            device: self.own.clone(),
            port,
            claim: self.claim.clone(),
            state: self.states.get(&self.own.id).cloned(),
            settings: self.settings.clone(),
        }
    }

    pub fn devices(&self) -> Vec<Device> {
        std::iter::once(&self.own)
            .chain(self.peers.values())
            .map(|device| Device {
                online: true,
                ..device.clone()
            })
            .collect()
    }

    pub fn welcome(&self) -> ServerMessage {
        let active = self.active().map(str::to_owned);
        ServerMessage::Welcome {
            devices: self.devices(),
            state: active.as_ref().and_then(|id| self.states.get(id).cloned()),
            active_device_id: active,
            settings: self.settings.value.settings.clone().into(),
            settings_version: self.settings.value.version,
        }
    }

    /// A link to `device` finished its handshake and delivered `hello`.
    pub fn linked(&mut self, device: Device, hello: PeerMessage) -> Vec<Effect> {
        let PeerMessage::Hello {
            claim,
            state,
            settings,
            ..
        } = hello
        else {
            return Vec::new();
        };
        let id = device.id.clone();
        let before = self.active().map(str::to_owned);
        self.peers.insert(id.clone(), device);
        if let Some(state) = state.filter(PlaybackState::valid) {
            self.states.insert(id.clone(), state);
        }
        let mut effects = vec![Effect::Session(ServerMessage::DevicesChanged {
            devices: self.devices(),
        })];
        if let Some(claim) = claim {
            effects.extend(self.consider_claim(claim, Some(&id)));
        }
        effects.extend(self.consider_settings(settings, Some(&id)));
        effects.extend(self.active_changed(before));
        effects
    }

    pub fn unlinked(&mut self, id: &str) -> Vec<Effect> {
        let before = self.active().map(str::to_owned);
        if self.peers.remove(id).is_none() {
            return Vec::new();
        }
        self.states.remove(id);
        let mut effects = vec![Effect::Session(ServerMessage::DevicesChanged {
            devices: self.devices(),
        })];
        let failed: Vec<_> = self
            .transfers
            .iter()
            .filter(|(_, transfer)| transfer.target == id)
            .map(|(transfer_id, _)| transfer_id.clone())
            .collect();
        for transfer_id in failed {
            self.transfers.remove(&transfer_id);
            effects.push(transfer_failed(transfer_id, "Target device disconnected"));
        }
        self.incoming.retain(|_, (source, _)| source != id);
        effects.extend(self.active_changed(before));
        effects
    }

    pub fn peer_message(&mut self, id: &str, message: PeerMessage) -> Vec<Effect> {
        if !self.peers.contains_key(id) {
            return Vec::new();
        }
        match message {
            PeerMessage::Claim { claim } => {
                let before = self.active().map(str::to_owned);
                let mut effects = self.consider_claim(claim, Some(id));
                effects.extend(self.active_changed(before));
                effects
            }
            PeerMessage::State { state } => {
                if !state.valid() {
                    return Vec::new();
                }
                self.states.insert(id.to_owned(), state.clone());
                if self.active() != Some(id) {
                    return Vec::new();
                }
                vec![Effect::Session(ServerMessage::State {
                    state,
                    from_device_id: id.to_owned(),
                })]
            }
            PeerMessage::Command { cmd } if cmd.valid() => {
                vec![Effect::Session(ServerMessage::Command {
                    cmd,
                    from_device_id: id.to_owned(),
                })]
            }
            PeerMessage::Transfer {
                transfer_id,
                state,
                start_playing,
            } => {
                if !state.valid() || transfer_id.is_empty() || transfer_id.len() > 128 {
                    return vec![Effect::Peer(
                        id.to_owned(),
                        PeerMessage::TransferResult {
                            transfer_id,
                            ok: false,
                            error: Some("Invalid playback transfer".into()),
                        },
                    )];
                }
                self.incoming
                    .insert(transfer_id.clone(), (id.to_owned(), state.clone()));
                vec![Effect::Session(ServerMessage::TransferRequest {
                    transfer_id,
                    state,
                    start_playing,
                })]
            }
            PeerMessage::TransferResult {
                transfer_id,
                ok,
                error,
            } => {
                if self
                    .transfers
                    .get(&transfer_id)
                    .is_none_or(|transfer| transfer.target != id)
                {
                    return Vec::new();
                }
                self.transfers.remove(&transfer_id);
                vec![Effect::Session(ServerMessage::TransferResult {
                    transfer_id,
                    ok,
                    error,
                })]
            }
            PeerMessage::Settings { settings } => self.consider_settings(settings, Some(id)),
            PeerMessage::Ping => vec![Effect::Peer(id.to_owned(), PeerMessage::Pong)],
            PeerMessage::Command { .. } | PeerMessage::Hello { .. } | PeerMessage::Pong => {
                Vec::new()
            }
        }
    }

    pub fn session_message(&mut self, message: ClientMessage, now: Instant) -> Vec<Effect> {
        let own = self.own.id.clone();
        match message {
            ClientMessage::StateUpdate { state } => self.publish(state),
            ClientMessage::Command {
                target_device_id,
                cmd,
            } => {
                if !cmd.valid() {
                    return vec![error("invalid_command", "Invalid playback command")];
                }
                if target_device_id == own {
                    return vec![Effect::Session(ServerMessage::Command {
                        cmd,
                        from_device_id: own,
                    })];
                }
                if !self.peers.contains_key(&target_device_id) {
                    return vec![error(
                        "device_unavailable",
                        "Target device is not available",
                    )];
                }
                vec![Effect::Peer(target_device_id, PeerMessage::Command { cmd })]
            }
            ClientMessage::Transfer {
                target_device_id,
                start_playing,
            } => self.transfer(target_device_id, start_playing, now),
            ClientMessage::TransferAck {
                transfer_id,
                ok,
                error,
            } => {
                let Some((source, mut state)) = self.incoming.remove(&transfer_id) else {
                    return Vec::new();
                };
                let mut effects = Vec::new();
                if ok {
                    // Publish the handed-over playback at once, above every sequence seen.
                    let at = wall_ms();
                    state.position_ms = position_at(&state, at);
                    state.captured_at_ms = at;
                    state.seq = self
                        .states
                        .values()
                        .map(|known| known.seq)
                        .chain([state.seq])
                        .max()
                        .unwrap_or(0)
                        .saturating_add(1);
                    let before = self.active().map(str::to_owned);
                    effects.extend(self.take_claim());
                    self.states.insert(self.own.id.clone(), state.clone());
                    effects.push(Effect::Broadcast(PeerMessage::State { state }));
                    effects.extend(self.active_changed(before));
                }
                effects.push(Effect::Peer(
                    source,
                    PeerMessage::TransferResult {
                        transfer_id,
                        ok,
                        error,
                    },
                ));
                effects
            }
            ClientMessage::SettingsPut {
                settings,
                base_version,
            } => {
                let serde_json::Value::Object(settings) = settings else {
                    return vec![error("invalid_settings", "Invalid shared settings")];
                };
                if !valid_settings(&settings) {
                    return vec![error("invalid_settings", "Invalid shared settings")];
                }
                if base_version != self.settings.value.version {
                    return vec![Effect::Session(ServerMessage::SettingsConflict {
                        settings: self.settings.value.settings.clone().into(),
                        version: self.settings.value.version,
                    })];
                }
                let next = Settings {
                    value: VersionedSettings {
                        settings,
                        version: base_version.saturating_add(1),
                    },
                    origin: own,
                };
                self.consider_settings(next, None)
            }
            ClientMessage::Reconcile { playback, settings } => {
                let mut effects = Vec::new();
                if let Some(playback) = playback.filter(|playback| playback.device_id == own) {
                    effects.extend(self.publish(playback.state));
                }
                if let Some(value) = settings.filter(|value| valid_settings(&value.settings)) {
                    effects.extend(self.consider_settings(Settings { value, origin: own }, None));
                }
                effects
            }
            ClientMessage::Ping => vec![Effect::Session(ServerMessage::Pong)],
            ClientMessage::Hello(_) => Vec::new(),
        }
    }

    /// Fails outgoing transfers whose target never answered.
    pub fn expire(&mut self, now: Instant) -> Vec<Effect> {
        let expired: Vec<_> = self
            .transfers
            .iter()
            .filter(|(_, transfer)| transfer.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        expired
            .into_iter()
            .map(|id| {
                self.transfers.remove(&id);
                transfer_failed(id, "The other device did not respond")
            })
            .collect()
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.transfers
            .values()
            .map(|transfer| transfer.deadline)
            .min()
    }

    fn publish(&mut self, state: PlaybackState) -> Vec<Effect> {
        if !state.valid() {
            return vec![error("invalid_state", "Invalid playback state")];
        }
        let before = self.active().map(str::to_owned);
        let mut effects = Vec::new();
        match before.as_deref() {
            // Playing here while another device is active takes over from it.
            Some(active) if active != self.own.id && !state.playing => return effects,
            Some(active) if active == self.own.id => {}
            _ => effects.extend(self.take_claim()),
        }
        self.states.insert(self.own.id.clone(), state.clone());
        effects.push(Effect::Broadcast(PeerMessage::State { state }));
        effects.extend(self.active_changed(before));
        effects
    }

    fn transfer(&mut self, target: String, start_playing: bool, now: Instant) -> Vec<Effect> {
        if self.active() != Some(self.own.id.as_str()) {
            return vec![error("not_active", "This device is not playing")];
        }
        if !self.peers.contains_key(&target) {
            return vec![error(
                "device_unavailable",
                "Target device is not available",
            )];
        }
        let Some(mut state) = self.states.get(&self.own.id).cloned() else {
            return vec![error("no_playback", "Nothing to transfer")];
        };
        let at = wall_ms();
        state.position_ms = position_at(&state, at);
        state.captured_at_ms = at;
        self.next_transfer = self.next_transfer.wrapping_add(1);
        let transfer_id = format!("{}-{}", self.own.id, self.next_transfer);
        self.transfers.insert(
            transfer_id.clone(),
            OutgoingTransfer {
                target: target.clone(),
                deadline: now + TRANSFER_TIMEOUT,
            },
        );
        vec![Effect::Peer(
            target,
            PeerMessage::Transfer {
                transfer_id,
                state,
                start_playing,
            },
        )]
    }

    /// Makes this device active with a claim above every claim seen so far.
    fn take_claim(&mut self) -> Vec<Effect> {
        let claim = Claim {
            epoch: self
                .claim
                .as_ref()
                .map_or(1, |claim| claim.epoch.saturating_add(1)),
            device_id: self.own.id.clone(),
        };
        self.claim = Some(claim.clone());
        vec![Effect::Broadcast(PeerMessage::Claim { claim })]
    }

    fn consider_claim(&mut self, claim: Claim, from: Option<&str>) -> Vec<Effect> {
        if self.claim.as_ref().is_none_or(|current| claim > *current) {
            self.claim = Some(claim);
            return Vec::new();
        }
        // Tell a device holding an older claim which one won.
        match (from, &self.claim) {
            (Some(from), Some(current)) if *current != claim => vec![Effect::Peer(
                from.to_owned(),
                PeerMessage::Claim {
                    claim: current.clone(),
                },
            )],
            _ => Vec::new(),
        }
    }

    fn consider_settings(&mut self, settings: Settings, from: Option<&str>) -> Vec<Effect> {
        if !valid_settings(&settings.value.settings) {
            return Vec::new();
        }
        if !settings.newer_than(&self.settings) {
            return match from {
                Some(from) if settings != self.settings => vec![Effect::Peer(
                    from.to_owned(),
                    PeerMessage::Settings {
                        settings: self.settings.clone(),
                    },
                )],
                _ => Vec::new(),
            };
        }
        self.settings = settings.clone();
        let mut effects = vec![Effect::Session(ServerMessage::Settings {
            settings: settings.value.settings.clone().into(),
            version: settings.value.version,
        })];
        if from.is_none() {
            effects.push(Effect::Broadcast(PeerMessage::Settings { settings }));
        }
        effects
    }

    fn active_changed(&self, before: Option<String>) -> Vec<Effect> {
        let after = self.active().map(str::to_owned);
        if before == after {
            return Vec::new();
        }
        let mut effects = vec![Effect::Session(ServerMessage::ActiveDeviceChanged {
            device_id: after.clone(),
        })];
        if let Some(state) = after
            .as_ref()
            .filter(|id| **id != self.own.id)
            .and_then(|id| self.states.get(id))
        {
            effects.push(Effect::Session(ServerMessage::State {
                state: state.clone(),
                from_device_id: after.clone().unwrap_or_default(),
            }));
        }
        effects
    }
}

fn error(code: &str, message: &str) -> Effect {
    Effect::Session(ServerMessage::Error {
        code: code.into(),
        message: message.into(),
    })
}

fn transfer_failed(transfer_id: String, message: &str) -> Effect {
    Effect::Session(ServerMessage::TransferResult {
        transfer_id,
        ok: false,
        error: Some(message.into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            os: "linux".into(),
            online: true,
            last_seen: 0,
        }
    }

    fn state(seq: u64) -> PlaybackState {
        PlaybackState {
            queue: vec!["song".into()],
            index: Some(0),
            playing: true,
            seq,
            ..Default::default()
        }
    }

    /// Links two meshes by exchanging hellos, then delivers peer effects until quiet.
    fn link(left: &mut Mesh, right: &mut Mesh) -> (Vec<ServerMessage>, Vec<ServerMessage>) {
        let (left_id, right_id) = (left.own.id.clone(), right.own.id.clone());
        let from_right = right.hello(1);
        let from_left = left.hello(1);
        let mut pending = vec![
            (left_id.clone(), left.linked(device(&right_id), from_right)),
            (right_id.clone(), right.linked(device(&left_id), from_left)),
        ];
        deliver(left, right, &mut pending)
    }

    fn deliver(
        left: &mut Mesh,
        right: &mut Mesh,
        pending: &mut Vec<(String, Vec<Effect>)>,
    ) -> (Vec<ServerMessage>, Vec<ServerMessage>) {
        let (mut left_session, mut right_session) = (Vec::new(), Vec::new());
        while let Some((at, effects)) = pending.pop() {
            for effect in effects {
                let (sender, receiver, session) = if at == left.own.id {
                    (&left.own.id, &mut *right, &mut left_session)
                } else {
                    (&right.own.id, &mut *left, &mut right_session)
                };
                match effect {
                    Effect::Session(message) => session.push(message),
                    Effect::Peer(_, message) | Effect::Broadcast(message) => {
                        let sender = sender.clone();
                        let target = receiver.own.id.clone();
                        pending.push((target, receiver.peer_message(&sender, message)));
                    }
                }
            }
        }
        (left_session, right_session)
    }

    fn active_change(messages: &[ServerMessage]) -> Option<Option<&str>> {
        messages.iter().rev().find_map(|message| match message {
            ServerMessage::ActiveDeviceChanged { device_id } => Some(device_id.as_deref()),
            _ => None,
        })
    }

    #[test]
    fn the_playing_device_stays_active_when_another_links() {
        let mut mac = Mesh::new(device("mac"));
        let mut linux = Mesh::new(device("linux"));
        mac.session_message(
            ClientMessage::StateUpdate { state: state(1) },
            Instant::now(),
        );
        assert_eq!(mac.active(), Some("mac"));

        let (_, linux_session) = link(&mut mac, &mut linux);
        assert_eq!(linux.active(), Some("mac"));
        assert_eq!(active_change(&linux_session), Some(Some("mac")));
        assert!(linux_session.iter().any(
            |message| matches!(message, ServerMessage::State { from_device_id, state } if from_device_id == "mac" && state.seq == 1)
        ));
        let ServerMessage::Welcome {
            active_device_id,
            state,
            devices,
            ..
        } = linux.welcome()
        else {
            unreachable!()
        };
        assert_eq!(active_device_id.as_deref(), Some("mac"));
        assert_eq!(state.map(|state| state.seq), Some(1));
        assert_eq!(devices.len(), 2);
    }

    #[test]
    fn only_starting_playback_takes_over_from_the_active_device() {
        let mut mac = Mesh::new(device("mac"));
        let mut linux = Mesh::new(device("linux"));
        mac.session_message(
            ClientMessage::StateUpdate { state: state(1) },
            Instant::now(),
        );
        link(&mut mac, &mut linux);
        let paused = PlaybackState {
            playing: false,
            ..state(2)
        };
        let effects =
            linux.session_message(ClientMessage::StateUpdate { state: paused }, Instant::now());
        assert!(
            effects.is_empty(),
            "paused playback elsewhere doesn't take over"
        );
        assert_eq!(linux.active(), Some("mac"));

        let effects = linux.session_message(
            ClientMessage::StateUpdate { state: state(3) },
            Instant::now(),
        );
        let (mac_session, _) = deliver(
            &mut mac,
            &mut linux,
            &mut vec![("linux".to_owned(), effects)],
        );
        assert_eq!(linux.active(), Some("linux"));
        assert_eq!(
            mac.active(),
            Some("linux"),
            "the last device to play is active"
        );
        assert_eq!(active_change(&mac_session), Some(Some("linux")));
    }

    #[test]
    fn simultaneous_claims_settle_on_one_device() {
        let mut mac = Mesh::new(device("mac"));
        let mut linux = Mesh::new(device("linux"));
        mac.session_message(
            ClientMessage::StateUpdate { state: state(1) },
            Instant::now(),
        );
        linux.session_message(
            ClientMessage::StateUpdate { state: state(1) },
            Instant::now(),
        );
        let (mac_session, linux_session) = link(&mut mac, &mut linux);
        assert_eq!(mac.active(), Some("mac"));
        assert_eq!(
            linux.active(),
            Some("mac"),
            "equal epochs go to the higher ID"
        );
        assert_eq!(active_change(&linux_session), Some(Some("mac")));
        assert_eq!(active_change(&mac_session), None);
    }

    #[test]
    fn transfer_moves_the_claim_to_the_target() {
        let mut mac = Mesh::new(device("mac"));
        let mut linux = Mesh::new(device("linux"));
        let now = Instant::now();
        mac.session_message(ClientMessage::StateUpdate { state: state(1) }, now);
        link(&mut mac, &mut linux);

        let request = mac.session_message(
            ClientMessage::Transfer {
                target_device_id: "linux".into(),
                start_playing: true,
            },
            now,
        );
        let (_, linux_session) = deliver(&mut mac, &mut linux, &mut vec![("mac".into(), request)]);
        let Some(ServerMessage::TransferRequest { transfer_id, .. }) = linux_session.first() else {
            panic!("the target is asked to prepare playback");
        };
        let ack = linux.session_message(
            ClientMessage::TransferAck {
                transfer_id: transfer_id.clone(),
                ok: true,
                error: None,
            },
            now,
        );
        let (mac_session, _) = deliver(&mut mac, &mut linux, &mut vec![("linux".into(), ack)]);
        assert_eq!(linux.active(), Some("linux"));
        assert_eq!(mac.active(), Some("linux"));
        assert!(
            mac_session
                .iter()
                .any(|message| matches!(message, ServerMessage::TransferResult { ok: true, .. }))
        );
        assert_eq!(active_change(&mac_session), Some(Some("linux")));
        assert!(
            mac_session.iter().any(|message| matches!(
                message,
                ServerMessage::State { from_device_id, state } if from_device_id == "linux" && state.seq == 2
            )),
            "the source sees the handed-over playback from the target right away"
        );
    }

    #[test]
    fn a_transfer_to_a_silent_device_fails_after_the_timeout() {
        let mut mac = Mesh::new(device("mac"));
        let mut linux = Mesh::new(device("linux"));
        let now = Instant::now();
        mac.session_message(ClientMessage::StateUpdate { state: state(1) }, now);
        link(&mut mac, &mut linux);
        mac.session_message(
            ClientMessage::Transfer {
                target_device_id: "linux".into(),
                start_playing: false,
            },
            now,
        );
        assert!(mac.expire(now).is_empty());
        let failed = mac.expire(now + TRANSFER_TIMEOUT);
        assert!(matches!(
            failed.as_slice(),
            [Effect::Session(ServerMessage::TransferResult {
                ok: false,
                ..
            })]
        ));
        assert_eq!(mac.active(), Some("mac"));
    }

    #[test]
    fn losing_the_active_device_clears_it_and_lets_another_play() {
        let mut mac = Mesh::new(device("mac"));
        let mut linux = Mesh::new(device("linux"));
        mac.session_message(
            ClientMessage::StateUpdate { state: state(1) },
            Instant::now(),
        );
        link(&mut mac, &mut linux);
        let effects = linux.unlinked("mac");
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::Session(ServerMessage::ActiveDeviceChanged { device_id: None })
        )));
        linux.session_message(
            ClientMessage::StateUpdate { state: state(2) },
            Instant::now(),
        );
        assert_eq!(linux.active(), Some("linux"));
    }

    #[test]
    fn commands_reach_only_linked_devices() {
        let mut mac = Mesh::new(device("mac"));
        let mut linux = Mesh::new(device("linux"));
        link(&mut mac, &mut linux);
        let sent = linux.session_message(
            ClientMessage::Command {
                target_device_id: "mac".into(),
                cmd: PlaybackCommand::Next,
            },
            Instant::now(),
        );
        assert!(matches!(
            sent.as_slice(),
            [Effect::Peer(target, PeerMessage::Command { cmd: PlaybackCommand::Next })] if target == "mac"
        ));
        let missing = linux.session_message(
            ClientMessage::Command {
                target_device_id: "phone".into(),
                cmd: PlaybackCommand::Next,
            },
            Instant::now(),
        );
        assert!(matches!(
            missing.as_slice(),
            [Effect::Session(ServerMessage::Error { code, .. })] if code == "device_unavailable"
        ));
    }

    #[test]
    fn shared_settings_converge_on_one_version() {
        let mut mac = Mesh::new(device("mac"));
        let mut linux = Mesh::new(device("linux"));
        let put = |dark| ClientMessage::SettingsPut {
            settings: serde_json::json!({ "dark": dark }),
            base_version: 0,
        };
        mac.session_message(put(true), Instant::now());
        linux.session_message(put(false), Instant::now());
        link(&mut mac, &mut linux);
        assert_eq!(mac.settings, linux.settings);
        assert_eq!(mac.settings.origin, "mac");
        let stale = linux.session_message(put(false), Instant::now());
        assert!(matches!(
            stale.as_slice(),
            [Effect::Session(ServerMessage::SettingsConflict {
                version: 1,
                ..
            })]
        ));
    }
}
