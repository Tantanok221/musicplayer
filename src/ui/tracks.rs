use super::*;

#[derive(Clone)]
pub(super) enum TrackPlayback {
    Queue(usize),
    Playlist(Arc<Vec<Arc<Track>>>, usize),
}

impl TrackPlayback {
    fn command(&self) -> PlayerCommand {
        match self {
            Self::Queue(index) => PlayerCommand::JumpTo(*index),
            Self::Playlist(tracks, index) => {
                PlayerCommand::Replace(tracks.as_ref().clone(), *index)
            }
        }
    }
}

#[derive(Clone)]
pub(super) enum TrackPosition {
    Queue(usize),
    Playlist {
        playback: TrackPlayback,
        editable_entry: Option<crate::playlists::PlaylistEntry>,
    },
}

impl TrackPosition {
    pub(super) fn playlist(playlist: &Playlist, index: usize, editable: bool) -> Self {
        Self::Playlist {
            playback: TrackPlayback::Playlist(playlist.entry.clone(), index),
            editable_entry: editable.then(|| crate::playlists::PlaylistEntry::new(playlist, index)),
        }
    }

    fn playback(&self) -> TrackPlayback {
        match self {
            Self::Queue(index) => TrackPlayback::Queue(*index),
            Self::Playlist { playback, .. } => playback.clone(),
        }
    }
}

pub(super) struct TrackDragPreview(pub(super) String);

impl Render for TrackDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .border_1()
            .border_color(cx.theme().border)
            .child(self.0.clone())
    }
}

impl MusicApp {
    pub(super) fn queue_places(&self) -> Rc<QueuePlaces> {
        let current = self.playback.track.as_ref();
        Rc::new(QueuePlaces {
            current_track: current.map(|track| track.title.clone()),
            current_album: current
                .filter(|track| !track.album_id.is_empty())
                .map(|track| track.album.clone()),
            last_album: self.playback.queue.last().map(|track| {
                if track.album.is_empty() {
                    track.title.clone()
                } else {
                    track.album.clone()
                }
            }),
        })
    }

    /// Right-click menu for an album: play it, or queue it in one of three places.
    pub(super) fn track_row(
        &mut self,
        track: Arc<Track>,
        number: usize,
        key: String,
        position: Option<TrackPosition>,
        popular: Option<bool>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let selected = self.selected.as_ref().is_some_and(|t| t.id == track.id);
        let active = self
            .playback
            .track
            .as_ref()
            .is_some_and(|t| t.id == track.id);
        let feedback = self.track_feedback(track.clone(), key.clone(), cx);
        let now = track.clone();
        let playback = position.as_ref().map(TrackPosition::playback);
        let row = div()
            .id(SharedString::from(format!("row-{key}")))
            .cursor_default();
        let row = match &position {
            Some(TrackPosition::Playlist {
                editable_entry: Some(entry),
                ..
            }) => self.playlist_reorder_row(row, entry, track.title.clone(), cx),
            _ => row,
        };
        let menu = self.track_menu(track.clone(), position, cx);
        row.test_support()
            .role(Role::Group)
            .aria_label(format!(
                "{} · {} · {}",
                track.title,
                track.artist_label(),
                track.album
            ))
            .w_full()
            .min_w_0()
            .h(px(27.))
            .px_1()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(11.))
            .when(selected, |s| s.bg(colors.selection))
            .when(!selected, |s| s.hover(|s| s.bg(colors.panel)))
            .when_some(popular, |row, popular| {
                row.child(
                    div()
                        .id(SharedString::from(format!("popular-{key}")))
                        .w(px(12.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .when(popular, |slot| {
                            slot.child(
                                Icon::new(Glyph::Flame)
                                    .size(px(11.))
                                    .text_color(colors.flame),
                            )
                            .tooltip(|window, cx| {
                                tooltip::Tooltip::new("Popular on Last.fm").build(window, cx)
                            })
                        }),
                )
            })
            .child(
                div()
                    .w(px(23.))
                    .flex_shrink_0()
                    .text_color(if active { colors.accent } else { colors.muted })
                    .child(number.to_string()),
            )
            .child(div().w(px(13.)).flex_shrink_0().when(active, |s| {
                s.child(
                    Icon::new(if self.playback.playing {
                        Glyph::Volume2
                    } else {
                        Glyph::Pause
                    })
                    .size(px(11.))
                    .text_color(colors.accent),
                )
            }))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_color(if active { colors.accent } else { colors.text })
                    .child(track.title.clone()),
            )
            .child(
                div()
                    .w(px(38.))
                    .flex_shrink_0()
                    .text_color(colors.muted)
                    .text_right()
                    .child(duration_label(track.duration)),
            )
            .child(feedback)
            .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                app.select_track(now.clone(), cx);
                if event.click_count() == 2 {
                    if let Some(playback) = &playback {
                        app.service.player.send(playback.command());
                    } else {
                        app.play_track(&now);
                    }
                }
            }))
            .context_menu(menu)
            .into_any_element()
    }

    pub(super) fn playing_tracks(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let colors = ink(cx);
        let count = self.playback.queue.len();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.panel)
            .child(
                div()
                    .h(px(28.))
                    .px_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.line)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .child(format!("Queue ({count})")),
                    )
                    .child(
                        short_button("save-playing-tracks", Glyph::Save, "Save Queue as playlist")
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.show_playlist_editor = true;
                                app.playlist_editor_rename = false;
                                app.playlist_editor_album = None;
                                app.playlist_editor_tracks =
                                    Some(app.playback.queue.iter().map(|t| t.id.clone()).collect());
                                cx.notify();
                            })),
                    ),
            )
            .child(
                uniform_list("playing-tracks-sidebar", count, move |range, _, cx| {
                    entity.update(cx, |app, cx| {
                        range
                            .map(|index| {
                                let track = app.playback.queue[index].clone();
                                let active = app.playback.index == Some(index);
                                let key = format!("queue-side-{index}");
                                let favorite = app.favorite_button(&track, &key, cx);
                                let rating = if app.information_width < 300. {
                                    app.rating_menu(&track, &key, cx).into_any_element()
                                } else {
                                    app.rating_stars(&track, &key, cx).into_any_element()
                                };
                                let select = track.clone();
                                let service = app.service.clone();
                                let service2 = service.clone();
                                let service3 = service.clone();
                                let menu_state = app.menu_state(cx);
                                let menu_track = track.clone();
                                div()
                                    .id(("queued", index))
                                    .w_full()
                                    .min_w_0()
                                    .role(Role::Group)
                                    .aria_label(format!("Queue {}: {}", index + 1, track.title))
                                    .h(px(37.))
                                    .overflow_hidden()
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .when(active, |s| s.bg(colors.selection))
                                    .hover(|s| s.bg(colors.line))
                                    .child(div().w(px(12.)).text_color(colors.accent).when(
                                        active,
                                        |s| {
                                            s.child(
                                                Icon::new(if app.playback.playing {
                                                    Glyph::Volume2
                                                } else {
                                                    Glyph::Pause
                                                })
                                                .size_3(),
                                            )
                                        },
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .truncate()
                                                    .text_color(if active {
                                                        colors.accent
                                                    } else {
                                                        colors.text
                                                    })
                                                    .child(track.title.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(colors.muted)
                                                    .truncate()
                                                    .child(app.artist_links(
                                                        track.artist_credits(),
                                                        format!("queue-artist-{index}"),
                                                        cx,
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .flex_shrink_0()
                                            .child(favorite)
                                            .child(rating),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(10.))
                                            .text_color(colors.muted)
                                            .child(duration_label(track.duration)),
                                    )
                                    .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                                        app.select_track(select.clone(), cx);
                                        if event.click_count() == 2 {
                                            app.service.player.send(PlayerCommand::JumpTo(index));
                                        }
                                    }))
                                    .context_menu(move |menu, window, cx| {
                                        let service = service.clone();
                                        let service2 = service2.clone();
                                        let service3 = service3.clone();
                                        let menu = context_menu::track_rating(
                                            menu.min_w(px(220.)),
                                            &menu_state,
                                            &menu_track,
                                        )
                                        .item(menu_row(Glyph::Play, "Play Now").on_click(
                                            move |_, _, _| {
                                                service.player.send(PlayerCommand::JumpTo(index))
                                            },
                                        ))
                                        .item(
                                            menu_row(Glyph::ArrowUp, "Move Earlier").on_click(
                                                move |_, _, _| {
                                                    service2.player.send(PlayerCommand::Move(
                                                        index,
                                                        index.saturating_sub(1),
                                                    ))
                                                },
                                            ),
                                        );
                                        context_menu::track_extras(
                                            menu,
                                            &menu_state,
                                            &menu_track,
                                            window,
                                            cx,
                                        )
                                        .separator()
                                        .item(
                                            menu_row(Glyph::ListX, "Remove from Queue").on_click(
                                                move |_, _, _| {
                                                    service3
                                                        .player
                                                        .send(PlayerCommand::Remove(index))
                                                },
                                            ),
                                        )
                                    })
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .flex_1()
                .size_full(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Arc, PlayerCommand, Track, TrackPlayback};

    #[test]
    fn playlist_playback_preserves_order_and_occurrence() {
        let track = Arc::new(Track {
            id: "duplicate".into(),
            ..Default::default()
        });
        let tracks = Arc::new(vec![track.clone(), track]);
        let PlayerCommand::Replace(queue, index) =
            TrackPlayback::Playlist(tracks.clone(), 1).command()
        else {
            panic!("playlist playback must replace the queue")
        };
        assert_eq!(index, 1);
        assert_eq!(queue.len(), 2);
        assert!(Arc::ptr_eq(&queue[0], &tracks[0]));
        assert!(Arc::ptr_eq(&queue[1], &tracks[1]));
    }

    #[test]
    fn queue_playback_keeps_the_existing_queue() {
        assert!(matches!(
            TrackPlayback::Queue(3).command(),
            PlayerCommand::JumpTo(3)
        ));
    }
}
