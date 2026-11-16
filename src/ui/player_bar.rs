use super::*;

impl MusicApp {
    pub(super) fn player_button(
        &self,
        id: &'static str,
        icon: Glyph,
        label: &'static str,
        command: PlayerCommand,
    ) -> Button {
        let player = self.service.player.clone();
        short_button(id, icon, label).on_click(move |_, _, _| player.send(command.clone()))
    }

    pub(super) fn player_bar(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let width = f32::from(window.viewport_size().width);
        let compact = width < 1000.;
        let track = self.playback.track.clone();
        let title = track
            .as_ref()
            .map_or("No track playing".into(), |track| track.title.clone());
        let artist_caption = if self.playback.buffering {
            div().child("Buffering…").into_any_element()
        } else if let Some(track) = &track {
            self.artist_links(track.artist_credits(), "footer-artists".into(), cx)
        } else {
            div().into_any_element()
        };
        let cover = track.as_ref().map(|track| {
            let side = if compact { 32. } else { 40. };
            div()
                .id("footer-cover")
                .group("footer-cover")
                .relative()
                .flex_shrink_0()
                .size(px(side))
                .cursor_pointer()
                .role(Role::Button)
                .aria_label("Open Now Playing")
                .tooltip(|window, cx| tooltip::Tooltip::new("Open Now Playing").build(window, cx))
                .child(self.art(&track.cover_art, side, cx))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui_kit::black().opacity(0.5))
                        .text_color(gpui_kit::white())
                        .opacity(0.)
                        .group_hover("footer-cover", |overlay| overlay.opacity(1.))
                        .child(Icon::new(Glyph::Maximize).size(px(side * 0.45))),
                )
                .on_click(cx.listener(|app, _, window, cx| {
                    app.switch_workspace(Workspace::NowPlaying, window, cx);
                    cx.notify();
                }))
        });
        let favorite = track
            .as_ref()
            .map(|track| self.favorite_button(track, "footer", cx));
        let rating = track.as_ref().map(|track| {
            if compact {
                self.rating_menu(track, "footer", cx).into_any_element()
            } else {
                self.rating_stars(track, "footer", cx).into_any_element()
            }
        });
        let time = |id, seconds| {
            div()
                .id(id)
                .test_support()
                .role(Role::Status)
                .aria_label(duration_label(seconds))
                .w(px(34.))
                .flex_shrink_0()
                .text_size(px(10.))
                .text_color(colors.muted)
                .child(duration_label(seconds))
        };
        let muted = self.preferences.volume <= 0.;
        div()
            .id("player-bar")
            .w_full()
            .min_w_0()
            .h(px(56.))
            .px_2()
            .flex_shrink_0()
            .border_t_1()
            .border_color(colors.line)
            .bg(colors.panel)
            .flex()
            .items_center()
            .gap(px(if compact { 8. } else { 16. }))
            .child(
                div()
                    .w(px((width * 0.2).clamp(120., 260.)))
                    .flex_shrink_0()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when_some(cover, |row, cover| row.child(cover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .min_w_0()
                                    .child(match track {
                                        Some(track) if !track.album_id.is_empty() => div()
                                            .id("footer-title")
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .truncate()
                                            .cursor_pointer()
                                            .role(Role::Link)
                                            .aria_label(format!(
                                                "Show {} in the library",
                                                track.album
                                            ))
                                            .hover(|title| title.text_color(colors.accent))
                                            .tooltip({
                                                let tip = format!(
                                                    "Show “{}” in the library",
                                                    track.album
                                                );
                                                move |window, cx| {
                                                    tooltip::Tooltip::new(tip.clone())
                                                        .build(window, cx)
                                                }
                                            })
                                            .child(title)
                                            .on_click(cx.listener(move |app, _, window, cx| {
                                                app.open_queue_album(&track, window, cx);
                                                cx.notify();
                                            }))
                                            .into_any_element(),
                                        _ => div()
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .truncate()
                                            .child(title)
                                            .into_any_element(),
                                    })
                                    .when_some(favorite, |row, favorite| {
                                        row.child(favorite.h(px(18.)))
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(colors.muted)
                                    .min_w_0()
                                    .child(artist_caption),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .gap(px(2.))
                            .child(
                                short_button(
                                    "shuffle",
                                    Glyph::Shuffle,
                                    if self.preferences.shuffle {
                                        "Shuffle: on"
                                    } else {
                                        "Shuffle: off"
                                    },
                                )
                                .selected(self.preferences.shuffle)
                                .on_click(cx.listener(
                                    |app, _, _, cx| {
                                        app.preferences.shuffle = !app.preferences.shuffle;
                                        app.save_preferences();
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(self.player_button(
                                "previous",
                                Glyph::SkipBack,
                                "Previous",
                                PlayerCommand::Previous,
                            ))
                            .child(self.player_button(
                                "play-pause",
                                if self.playback.playing {
                                    Glyph::Pause
                                } else {
                                    Glyph::Play
                                },
                                if self.playback.playing {
                                    "Pause"
                                } else {
                                    "Play"
                                },
                                PlayerCommand::Toggle,
                            ))
                            .child(self.player_button(
                                "next",
                                Glyph::SkipForward,
                                "Next",
                                PlayerCommand::Next,
                            ))
                            .child(
                                short_button(
                                    "repeat",
                                    if self.preferences.repeat == RepeatMode::One {
                                        Glyph::Repeat1
                                    } else {
                                        Glyph::Repeat
                                    },
                                    match self.preferences.repeat {
                                        RepeatMode::Off => "Repeat: off",
                                        RepeatMode::All => "Repeat: all",
                                        RepeatMode::One => "Repeat: one",
                                    },
                                )
                                .selected(self.preferences.repeat != RepeatMode::Off)
                                .on_click(cx.listener(
                                    |app, _, _, cx| {
                                        app.preferences.repeat = match app.preferences.repeat {
                                            RepeatMode::Off => RepeatMode::All,
                                            RepeatMode::All => RepeatMode::One,
                                            RepeatMode::One => RepeatMode::Off,
                                        };
                                        app.save_preferences();
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(
                        time(
                            "playback-elapsed",
                            self.scrub.map_or(self.playback.position, |value| {
                                value as f64 / 1000. * self.playback.duration
                            }),
                        )
                        .text_right()
                        .when(self.scrub.is_some(), |label| {
                            label.text_color(colors.accent)
                        }),
                    )
                    .child(
                        Slider::new(&self.seek)
                            .flex_1()
                            .min_w(px(40.))
                            .accessibility_label("Playback position"),
                    )
                    .child(time("playback-duration", self.playback.duration)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(if compact { 4. } else { 10. }))
                    .when_some(rating, |row, rating| row.child(rating))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                short_button(
                                    "mute",
                                    if muted {
                                        Glyph::VolumeX
                                    } else {
                                        Glyph::Volume2
                                    },
                                    if muted { "Unmute" } else { "Mute" },
                                )
                                .on_click(cx.listener(
                                    |app, _, window, cx| {
                                        let volume = match app.volume_before_mute.take() {
                                            Some(previous) => previous,
                                            None if app.preferences.volume > 0. => {
                                                app.volume_before_mute =
                                                    Some(app.preferences.volume);
                                                0.
                                            }
                                            None => Preferences::default().volume,
                                        };
                                        app.preferences.volume = volume;
                                        app.volume.update(cx, |slider, cx| {
                                            slider.set_value(volume, window, cx)
                                        });
                                        app.save_preferences();
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                Slider::new(&self.volume)
                                    .w(px(if compact { 44. } else { 76. }))
                                    .accessibility_label("Volume"),
                            ),
                    ),
            )
            .into_any_element()
    }
}
