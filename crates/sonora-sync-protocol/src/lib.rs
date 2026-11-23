use serde::{Deserialize, Serialize};
use std::time::Duration;

pub type TrackId = String;
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Serialize, Deserialize)]
pub struct Hello {
    pub username: String,
    pub subsonic_token: String,
    pub subsonic_salt: String,
    pub device_id: String,
    pub device_name: String,
    pub os: String,
    pub client_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub os: String,
    pub online: bool,
    pub last_seen: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    One,
    All,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackState {
    pub queue: Vec<TrackId>,
    pub index: Option<usize>,
    pub position_ms: u64,
    pub playing: bool,
    pub volume: u8,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub captured_at_ms: i64,
    pub seq: u64,
}

impl PlaybackState {
    pub fn valid(&self) -> bool {
        queue_valid(&self.queue, self.index) && self.volume <= 100
    }
}

fn queue_valid(queue: &[TrackId], index: Option<usize>) -> bool {
    index.is_none_or(|i| i < queue.len())
        && queue.len() <= 10_000
        && queue.iter().all(|id| !id.is_empty() && id.len() <= 256)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PlaybackCommand {
    Play,
    Pause,
    Seek {
        ms: u64,
    },
    Next,
    Prev,
    SetVolume {
        volume: u8,
    },
    PlayIndex {
        i: usize,
    },
    SetQueue {
        track_ids: Vec<TrackId>,
        index: Option<usize>,
        position_ms: u64,
    },
}

impl PlaybackCommand {
    pub fn valid(&self) -> bool {
        match self {
            Self::SetVolume { volume } => *volume <= 100,
            Self::SetQueue {
                track_ids, index, ..
            } => queue_valid(track_ids, *index),
            _ => true,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    Hello(Hello),
    StateUpdate {
        state: PlaybackState,
    },
    Command {
        target_device_id: String,
        cmd: PlaybackCommand,
    },
    Transfer {
        target_device_id: String,
        start_playing: bool,
    },
    TransferAck {
        transfer_id: String,
        ok: bool,
        error: Option<String>,
    },
    SettingsPut {
        settings: serde_json::Value,
        base_version: u64,
    },
    Ping,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    Welcome {
        devices: Vec<Device>,
        active_device_id: Option<String>,
        state: Option<PlaybackState>,
        settings: serde_json::Value,
        settings_version: u64,
    },
    DevicesChanged {
        devices: Vec<Device>,
    },
    State {
        state: PlaybackState,
        from_device_id: String,
    },
    Command {
        cmd: PlaybackCommand,
        from_device_id: String,
    },
    TransferRequest {
        transfer_id: String,
        state: PlaybackState,
        start_playing: bool,
    },
    TransferResult {
        transfer_id: String,
        ok: bool,
        error: Option<String>,
    },
    ActiveDeviceChanged {
        device_id: Option<String>,
    },
    Settings {
        settings: serde_json::Value,
        version: u64,
    },
    SettingsConflict {
        settings: serde_json::Value,
        version: u64,
    },
    Error {
        code: String,
        message: String,
    },
    Pong,
}
