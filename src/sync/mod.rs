use crate::model::{RepeatMode, Track};
use sonora_sync_protocol::{Device, Placement, PlaybackState, QueueChange, Repeat};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[doc(hidden)]
pub mod lan;
#[doc(hidden)]
pub mod lan_registry;
#[doc(hidden)]
pub mod lan_security;
pub(crate) mod mesh;
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
    /// The other device this one chose to show and control in Devices.
    pub controlled: Option<String>,
    pub devices: Vec<Device>,
    pub playback: Option<PlaybackState>,
    pub track: Option<Arc<Track>>,
    /// The active remote device's queue, resolved from the catalog in queue order.
    pub queue: Arc<Vec<Arc<Track>>>,
}

impl DeviceStatus {
    /// The device this one controls: another active device that was chosen in Devices.
    pub fn remote_id(&self) -> Option<&str> {
        self.other_active()
            .filter(|id| self.controlled.as_deref() == Some(*id))
    }

    /// Another device that is playing, whether or not this one controls it.
    pub fn other_active(&self) -> Option<&str> {
        self.active_device_id
            .as_deref()
            .filter(|id| self.connected && *id != self.local_device_id)
    }

    /// Stop controlling a device that left, or when another device took over.
    /// A reconnect keeps the choice; `devices_changed` marks a live device-list update.
    pub(crate) fn settle_control(&mut self, was_remote: bool, devices_changed: bool) {
        let Some(controlled) = self.controlled.as_deref() else {
            return;
        };
        let gone = devices_changed && !self.devices.iter().any(|device| device.id == controlled);
        let replaced = self
            .other_active()
            .is_some_and(|active| active != controlled);
        // This device took over playback while it was showing another device's.
        let local = was_remote && self.active_device_id.as_deref() == Some(&self.local_device_id);
        if gone || replaced || local {
            self.controlled = None;
        }
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

    /// The active remote device's queue in the shape the queue views render.
    /// Entry IDs are queue positions, and there is no local undo history.
    pub fn queue_snapshot(&self) -> Option<crate::model::PlaybackSnapshot> {
        self.remote_id()?;
        let state = self.playback.as_ref()?;
        Some(crate::model::PlaybackSnapshot {
            track: self.track.clone(),
            playing: state.playing,
            position: self.remote_position(),
            duration: self.track.as_ref().map_or(0., |track| track.duration),
            index: state.index,
            queue: self.queue.clone(),
            queue_ids: Arc::new((0..self.queue.len() as u64).collect()),
            queue_revision: state.seq,
            ..Default::default()
        })
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

/// The remote form of a local queue edit, or `None` for commands that are not queue edits.
pub fn queue_change(command: &crate::playback::PlayerCommand) -> Option<QueueChange> {
    use crate::playback::{PlayerCommand, QueueEdit};
    Some(match command {
        PlayerCommand::Insert(tracks, placement) => QueueChange::Insert {
            track_ids: tracks.iter().map(|track| track.id.clone()).collect(),
            placement: remote_placement(*placement),
        },
        PlayerCommand::Remove(index) => QueueChange::RemoveAt { index: *index },
        PlayerCommand::Move(from, to) => QueueChange::MoveTo {
            from: *from,
            to: *to,
        },
        PlayerCommand::EditQueue(_, QueueEdit::Remove(indices)) => QueueChange::Remove {
            indices: indices.clone(),
        },
        PlayerCommand::EditQueue(_, QueueEdit::Move { indices, before }) => QueueChange::Move {
            indices: indices.clone(),
            before: *before,
        },
        PlayerCommand::EditQueue(_, QueueEdit::ClearUpcoming) => QueueChange::ClearUpcoming,
        _ => return None,
    })
}

fn remote_placement(placement: crate::playback::Placement) -> Placement {
    match placement {
        crate::playback::Placement::Next => Placement::Next,
        crate::playback::Placement::AfterAlbum => Placement::AfterAlbum,
        crate::playback::Placement::Last => Placement::Last,
    }
}

pub(crate) fn local_placement(placement: Placement) -> crate::playback::Placement {
    match placement {
        Placement::Next => crate::playback::Placement::Next,
        Placement::AfterAlbum => crate::playback::Placement::AfterAlbum,
        Placement::Last => crate::playback::Placement::Last,
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
    fn queue_snapshot_shows_only_an_active_remote_queue() {
        let track = |id: &str| {
            Arc::new(Track {
                id: id.into(),
                ..Default::default()
            })
        };
        let mut status = DeviceStatus {
            connected: true,
            local_device_id: "local".into(),
            active_device_id: Some("remote".into()),
            controlled: Some("remote".into()),
            playback: Some(PlaybackState {
                queue: vec!["a".into(), "b".into()],
                index: Some(1),
                seq: 9,
                ..Default::default()
            }),
            queue: Arc::new(vec![track("a"), track("b")]),
            ..Default::default()
        };
        let snapshot = status.queue_snapshot().expect("remote queue");
        assert_eq!(*snapshot.queue_ids, [0, 1]);
        assert_eq!(snapshot.index, Some(1));
        assert_eq!(snapshot.queue_revision, 9);
        assert!(!snapshot.can_undo_queue);

        status.active_device_id = Some("local".into());
        assert!(
            status.queue_snapshot().is_none(),
            "this device shows its own queue"
        );
        status.active_device_id = Some("remote".into());
        status.connected = false;
        assert!(status.queue_snapshot().is_none());
    }

    #[test]
    fn another_device_is_controlled_only_once_chosen() {
        let device = |id: &str| Device {
            id: id.into(),
            name: id.into(),
            os: "linux".into(),
            online: true,
            last_seen: 1,
        };
        let mut status = DeviceStatus {
            connected: true,
            local_device_id: "local".into(),
            active_device_id: Some("linux".into()),
            devices: vec![device("local"), device("linux"), device("laptop")],
            ..Default::default()
        };
        assert_eq!(status.other_active(), Some("linux"));
        assert_eq!(
            status.remote_id(),
            None,
            "playing elsewhere is not followed"
        );

        status.controlled = Some("linux".into());
        assert_eq!(status.remote_id(), Some("linux"));

        // Another device starting playback ends control instead of following it.
        status.active_device_id = Some("laptop".into());
        status.settle_control(true, false);
        assert_eq!(status.controlled, None);
        assert_eq!(status.remote_id(), None);

        // Choosing a device before its transfer lands keeps the choice.
        status.active_device_id = Some("local".into());
        status.controlled = Some("linux".into());
        status.settle_control(false, false);
        assert_eq!(status.controlled.as_deref(), Some("linux"));
        // Playing here while showing another device's playback ends control.
        status.settle_control(true, false);
        assert_eq!(status.controlled, None);

        // A device that leaves is no longer controlled.
        status.controlled = Some("linux".into());
        status.devices.retain(|device| device.id != "linux");
        status.settle_control(false, false);
        assert_eq!(
            status.controlled.as_deref(),
            Some("linux"),
            "a reconnect that hasn't listed the device yet keeps the choice"
        );
        status.settle_control(false, true);
        assert_eq!(status.controlled, None);
    }

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
