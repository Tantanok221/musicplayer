use super::*;
use crate::sync::DeviceStatus;
use sonora_sync_protocol::PlaybackCommand;

impl MusicApp {
    pub(super) fn devices_button(&self) -> impl IntoElement {
        let status = self.device_status.clone();
        let service = self.service.clone();
        let transferable = status.track_id().is_some() || self.playback.track.is_some();
        let start_playing = self.display_playing() || status.active_device_id.is_none();
        Button::new("devices")
            .ghost()
            .small()
            .icon(Glyph::Monitor)
            .label("Devices")
            .dropdown_menu(move |mut menu, _, _| {
                if !status.connected {
                    return menu.item(PopupMenuItem::new("Sync server unavailable").disabled(true));
                }
                for choice in device_menu(&status, transferable, start_playing) {
                    let icon = match choice.os.as_str() {
                        "macos" | "ios" => Glyph::Apple,
                        "linux" => Glyph::Terminal,
                        "android" => Glyph::Smartphone,
                        _ => Glyph::Monitor,
                    };
                    let mut item = PopupMenuItem::new(choice.label).icon(icon);
                    if let Some(command) = choice.command {
                        let service = service.clone();
                        item = item.on_click(move |_, _, _| service.send(command.clone()));
                    } else {
                        item = item.disabled(true);
                    }
                    menu = menu.item(item);
                }
                if let Some(state) = status.playback.as_ref()
                    && let Some(track) = status
                        .track
                        .as_ref()
                        .map(|track| track.title.as_str())
                        .or(status.track_id())
                {
                    let verb = if state.playing { "Playing" } else { "Paused" };
                    menu = menu
                        .separator()
                        .item(PopupMenuItem::new(format!("{verb} · {track}")).disabled(true));
                }
                menu
            })
    }

    /// The queue the queue views show: the active remote device's, or this device's.
    pub(super) fn queue_playback(&self) -> &PlaybackSnapshot {
        self.remote_queue.as_ref().unwrap_or(&self.playback)
    }

    pub(super) fn playback_intent(&self, command: PlayerCommand) {
        if self.device_status.remote_id().is_none() {
            self.service.player.send(command);
            return;
        }
        if let Some(change) = crate::sync::queue_change(&command) {
            let queue = self
                .device_status
                .playback
                .as_ref()
                .map_or(&[][..], |state| state.queue.as_slice());
            self.service
                .send(Command::RemotePlayback(PlaybackCommand::EditQueue {
                    queue: sonora_sync_protocol::queue_fingerprint(queue),
                    change,
                }));
            return;
        }
        let command = match command {
            PlayerCommand::Toggle if self.display_playing() => PlaybackCommand::Pause,
            PlayerCommand::Toggle | PlayerCommand::Play => PlaybackCommand::Play,
            PlayerCommand::Pause => PlaybackCommand::Pause,
            PlayerCommand::Next => PlaybackCommand::Next,
            PlayerCommand::Previous => PlaybackCommand::Prev,
            PlayerCommand::Seek(seconds) => PlaybackCommand::Seek {
                ms: crate::sync::milliseconds(seconds),
            },
            PlayerCommand::JumpTo(i)
                if self.device_status.playback.as_ref().is_some_and(|state| {
                    state.queue.iter().map(String::as_str).eq(self
                        .queue_playback()
                        .queue
                        .iter()
                        .map(|track| track.id.as_str()))
                }) =>
            {
                PlaybackCommand::PlayIndex { i }
            }
            PlayerCommand::Replace(tracks, index) => {
                let track_ids: Vec<_> = tracks.iter().map(|track| track.id.clone()).collect();
                let has_tracks = !track_ids.is_empty();
                self.service
                    .send(Command::RemotePlayback(PlaybackCommand::SetQueue {
                        track_ids,
                        index: has_tracks.then_some(index),
                        position_ms: 0,
                    }));
                if !has_tracks {
                    return;
                }
                PlaybackCommand::Play
            }
            _ => return,
        };
        self.service.send(Command::RemotePlayback(command));
    }

    pub(super) fn display_playing(&self) -> bool {
        if self.device_status.remote_id().is_some() {
            self.device_status
                .playback
                .as_ref()
                .is_some_and(|state| state.playing)
        } else {
            self.playback.playing
        }
    }

    pub(super) fn display_duration(&self) -> f64 {
        if self.device_status.remote_id().is_some() {
            self.device_status
                .track
                .as_ref()
                .map_or(0., |track| track.duration)
        } else {
            self.playback.duration
        }
    }

    pub(super) fn display_volume(&self) -> f32 {
        if self.device_status.remote_id().is_some() {
            self.device_status
                .playback
                .as_ref()
                .map_or(0., |state| f32::from(state.volume) / 100.)
        } else {
            self.preferences.volume
        }
    }

    pub(super) fn control_volume(&mut self, volume: f32) {
        if self.device_status.remote_id().is_some() {
            self.service
                .send(Command::RemotePlayback(PlaybackCommand::SetVolume {
                    volume: crate::sync::volume(volume),
                }));
        } else {
            self.preferences.volume = volume;
            self.save_preferences();
        }
    }

    pub(super) fn device_status_changed(
        &mut self,
        status: DeviceStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remote_changed = self.device_status.remote_id() != status.remote_id();
        let shows_remote = self.remote_queue.is_some();
        self.remote_queue = status.queue_snapshot();
        if shows_remote || self.remote_queue.is_some() {
            self.notify_part(Part::SidebarQueue, cx);
            self.notify_part(Part::Center, cx);
        }
        self.device_status = status;
        let shown = self.queue_playback().queue_ids.clone();
        self.queue_ui.retain_selection(&shown);
        if remote_changed {
            self.volume_before_mute = None;
            self.scrub = None;
        }
        if self.scrub.is_none() {
            let progress = if self.display_duration() > 0. {
                (self.display_position() / self.display_duration() * 1000.).clamp(0., 1000.) as f32
            } else {
                0.
            };
            self.seek
                .update(cx, |slider, cx| slider.set_value(progress, window, cx));
        }
        let volume = self.display_volume();
        self.volume
            .update(cx, |slider, cx| slider.set_value(volume, window, cx));
        self.notify_part(Part::Player, cx);
    }

    pub(super) fn display_position(&self) -> f64 {
        if self.device_status.remote_id().is_none() {
            return self.playback.position;
        }
        let position = self.device_status.remote_position();
        let duration = self.display_duration();
        if duration > 0. {
            position.min(duration)
        } else {
            position
        }
    }
}

/// One device in the Devices menu, and what choosing it does.
struct DeviceChoice {
    label: String,
    os: String,
    command: Option<Command>,
}

/// Choosing the playing device shows and controls it, choosing this device goes
/// back to its own player, and choosing any other device moves playback there.
fn device_menu(
    status: &DeviceStatus,
    transferable: bool,
    start_playing: bool,
) -> Vec<DeviceChoice> {
    let controlled = status.remote_id();
    status
        .devices
        .iter()
        .map(|device| {
            let active = status.active_device_id.as_deref() == Some(&device.id);
            let local = device.id == status.local_device_id;
            let mut label = device.name.clone();
            if active && !local {
                let playing = status.playback.as_ref().is_some_and(|state| state.playing);
                label.push_str(if playing { " · Playing" } else { " · Paused" });
            } else if active {
                label.push_str(" · Active");
            }
            if controlled == Some(device.id.as_str()) {
                label.push_str(" · Controlling");
            }
            if local {
                label.push_str(" · This device");
            }
            if !device.online {
                label.push_str(" · Offline");
            }
            let command = if local {
                controlled.map(|_| Command::ControlDevice(None))
            } else if !device.online || controlled == Some(device.id.as_str()) {
                None
            } else if active {
                Some(Command::ControlDevice(Some(device.id.clone())))
            } else if transferable && status.other_active().is_none() {
                Some(Command::TransferTo(device.id.clone(), start_playing))
            } else {
                None
            };
            DeviceChoice {
                label,
                os: device.os.clone(),
                command,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{DeviceStatus, device_menu};
    use crate::service::Command;
    use sonora_sync_protocol::{Device, PlaybackState};

    fn status(active: Option<&str>, controlled: Option<&str>) -> DeviceStatus {
        let device = |id: &str, online| Device {
            id: id.into(),
            name: id.into(),
            os: "linux".into(),
            online,
            last_seen: 1,
        };
        DeviceStatus {
            connected: true,
            local_device_id: "mac".into(),
            active_device_id: active.map(str::to_owned),
            controlled: controlled.map(str::to_owned),
            devices: vec![
                device("mac", true),
                device("linux", true),
                device("laptop", true),
                device("old", false),
            ],
            playback: Some(PlaybackState {
                playing: true,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn choices(status: &DeviceStatus) -> Vec<(String, &'static str)> {
        device_menu(status, true, true)
            .into_iter()
            .map(|choice| (choice.label, action(choice.command)))
            .collect()
    }

    fn action(command: Option<Command>) -> &'static str {
        match command {
            None => "none",
            Some(Command::ControlDevice(None)) => "this device",
            Some(Command::ControlDevice(Some(id))) if id == "linux" => "control linux",
            Some(Command::TransferTo(id, true)) if id == "linux" => "transfer to linux",
            Some(_) => "other",
        }
    }

    fn pair(label: &str, action: &'static str) -> (String, &'static str) {
        (label.into(), action)
    }

    #[test]
    fn the_playing_device_is_offered_for_control() {
        assert_eq!(
            choices(&status(Some("linux"), None)),
            [
                pair("mac · This device", "none"),
                pair("linux · Playing", "control linux"),
                // Playback here can't be moved while another device plays.
                pair("laptop", "none"),
                pair("old · Offline", "none"),
            ]
        );
    }

    #[test]
    fn this_device_ends_control() {
        assert_eq!(
            &choices(&status(Some("linux"), Some("linux")))[..2],
            [
                pair("mac · This device", "this device"),
                pair("linux · Playing · Controlling", "none"),
            ]
        );
    }

    #[test]
    fn idle_devices_receive_this_devices_playback() {
        assert_eq!(
            &choices(&status(Some("mac"), None))[..2],
            [
                pair("mac · Active · This device", "none"),
                pair("linux", "transfer to linux"),
            ]
        );
        let nothing = device_menu(&status(None, None), false, true);
        assert!(nothing[1].command.is_none(), "nothing to move");
    }
}
