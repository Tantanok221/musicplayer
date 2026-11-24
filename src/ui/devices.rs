use super::*;
use crate::sync::DeviceStatus;
use sonora_sync_protocol::PlaybackCommand;

impl MusicApp {
    pub(super) fn resume_queue_button(&self, from: &str) -> Button {
        let service = self.service.clone();
        Button::new("resume-server-queue")
            .small()
            .label(format!("Resume from {from}"))
            .on_click(move |_, _, _| service.send(Command::ResumeServerQueue))
    }

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
                for device in &status.devices {
                    let active = status.active_device_id.as_deref() == Some(&device.id);
                    let mut label = device.name.clone();
                    if active {
                        label.push_str(" · Active");
                    }
                    if device.id == status.local_device_id {
                        label.push_str(" · This device");
                    }
                    if !device.online {
                        label.push_str(" · Offline");
                    }
                    let icon = match device.os.as_str() {
                        "macos" | "ios" => Glyph::Apple,
                        "linux" => Glyph::Terminal,
                        "android" => Glyph::Smartphone,
                        _ => Glyph::Monitor,
                    };
                    let mut item = PopupMenuItem::new(label).icon(icon);
                    if device.online && !active && transferable {
                        let id = device.id.clone();
                        let service = service.clone();
                        item = item.on_click(move |_, _, _| {
                            service.send(Command::TransferTo(id.clone(), start_playing))
                        });
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

    pub(super) fn playback_intent(&self, command: PlayerCommand) {
        if self.device_status.remote_id().is_none() {
            self.service.player.send(command);
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
                        .playback
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
        self.device_status = status;
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
