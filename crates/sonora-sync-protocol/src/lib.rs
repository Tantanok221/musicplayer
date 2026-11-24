use serde::{Deserialize, Serialize};
use std::time::Duration;

pub type TrackId = String;
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedSettings {
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub version: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReconciledPlayback {
    pub device_id: String,
    pub state: PlaybackState,
}

pub fn valid_settings(settings: &serde_json::Map<String, serde_json::Value>) -> bool {
    settings.iter().all(|(name, value)| match name.as_str() {
        "dark" => value.is_null() || value.is_boolean(),
        "gain" => matches!(value.as_str(), Some("Off" | "Track" | "Album" | "Smart")),
        "sorting" | "playlist_columns" | "favorites" => value.is_object(),
        "all_missing_releases" => value.is_boolean(),
        _ => false,
    })
}

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
    /// Edit the active device's queue. `queue` is the [`queue_fingerprint`] of the
    /// queue the edit was made against; the device ignores edits to a queue that
    /// has changed since. Insertions do not depend on it.
    EditQueue {
        queue: String,
        change: QueueChange,
    },
}

/// Where inserted tracks go in the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    Next,
    AfterAlbum,
    Last,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum QueueChange {
    Insert {
        track_ids: Vec<TrackId>,
        placement: Placement,
    },
    /// Remove one entry at any position.
    RemoveAt {
        index: usize,
    },
    /// Move one entry from one position to another.
    MoveTo {
        from: usize,
        to: usize,
    },
    /// Remove upcoming entries.
    Remove {
        indices: Vec<usize>,
    },
    /// Move upcoming entries before a position.
    Move {
        indices: Vec<usize>,
        before: usize,
    },
    ClearUpcoming,
}

impl QueueChange {
    fn valid(&self) -> bool {
        match self {
            Self::Insert { track_ids, .. } => !track_ids.is_empty() && queue_valid(track_ids, None),
            Self::Remove { indices } | Self::Move { indices, .. } => {
                !indices.is_empty() && indices.len() <= 10_000
            }
            Self::RemoveAt { .. } | Self::MoveTo { .. } | Self::ClearUpcoming => true,
        }
    }
}

/// A short, stable identifier for a queue's exact track order.
pub fn queue_fingerprint(queue: &[TrackId]) -> String {
    // FNV-1a over each ID and a separator, so the result is the same on every platform.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for id in queue {
        for byte in id.bytes().chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{:016x}-{}", hash, queue.len())
}

impl PlaybackCommand {
    pub fn valid(&self) -> bool {
        match self {
            Self::SetVolume { volume } => *volume <= 100,
            Self::SetQueue {
                track_ids, index, ..
            } => queue_valid(track_ids, *index),
            Self::EditQueue { queue, change } => queue.len() <= 64 && change.valid(),
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
    Reconcile {
        playback: Option<ReconciledPlayback>,
        settings: Option<VersionedSettings>,
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
