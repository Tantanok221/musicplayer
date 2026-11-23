use crate::model::{RepeatMode, Track};
use sonora_sync_protocol::{Device, PlaybackState, Repeat};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[doc(hidden)]
pub mod lan;
#[doc(hidden)]
pub mod lan_security;
pub(crate) mod relay;
pub(crate) mod settings;
mod transport;
pub use relay::{InvalidRelayUrl, RelayEvent};
pub use settings::SharedPreferences;

#[derive(Clone, Debug, Default)]
pub struct DeviceStatus {
    pub enabled: bool,
    pub connected: bool,
    pub local_device_id: String,
    pub active_device_id: Option<String>,
    pub devices: Vec<Device>,
    pub playback: Option<PlaybackState>,
    pub track: Option<Arc<Track>>,
}

impl DeviceStatus {
    pub fn remote_id(&self) -> Option<&str> {
        self.active_device_id
            .as_deref()
            .filter(|id| self.connected && *id != self.local_device_id)
    }

    pub fn track_id(&self) -> Option<&str> {
        let state = self.playback.as_ref()?;
        Some(state.queue.get(state.index?)?)
    }

    pub fn remote_position(&self) -> f64 {
        let Some(state) = self.playback.as_ref() else {
            return 0.;
        };
        position_at(state, wall_ms()) as f64 / 1000.
    }

    pub(crate) fn accept(&mut self, state: PlaybackState) -> bool {
        if self
            .playback
            .as_ref()
            .is_some_and(|last| state.seq <= last.seq)
        {
            return false;
        }
        self.playback = Some(state);
        true
    }
}

pub(crate) fn wall_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|time| i64::try_from(time.as_millis()).ok())
        .unwrap_or(0)
}

pub(crate) fn position_at(state: &PlaybackState, now_ms: i64) -> u64 {
    let elapsed = if state.playing && state.captured_at_ms > 0 {
        now_ms.saturating_sub(state.captured_at_ms).max(0) as u64
    } else {
        0
    };
    state.position_ms.saturating_add(elapsed)
}

#[derive(Clone, Debug)]
pub struct PlaybackObservation {
    pub namespace: String,
    pub revision: u64,
    pub volume: u8,
    pub shuffle: bool,
    pub repeat: Repeat,
}

pub(crate) fn repeat(value: RepeatMode) -> Repeat {
    match value {
        RepeatMode::Off => Repeat::Off,
        RepeatMode::One => Repeat::One,
        RepeatMode::All => Repeat::All,
    }
}

pub(crate) fn milliseconds(seconds: f64) -> u64 {
    (seconds * 1000.).round() as u64
}

pub(crate) fn volume(level: f32) -> u8 {
    (level.clamp(0., 1.) * 100.).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_equal_or_older_remote_frames() {
        let mut status = DeviceStatus::default();
        assert!(status.accept(PlaybackState {
            seq: 5,
            ..Default::default()
        }));
        assert!(!status.accept(PlaybackState {
            seq: 5,
            ..Default::default()
        }));
        assert!(!status.accept(PlaybackState {
            seq: 4,
            ..Default::default()
        }));
        assert!(status.accept(PlaybackState {
            seq: 6,
            ..Default::default()
        }));
    }
}
