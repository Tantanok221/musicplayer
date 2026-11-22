use super::*;
use crate::playlists::PlaylistEntry;

#[derive(Clone)]
struct PlaylistDrag {
    entry: PlaylistEntry,
    title: String,
}

impl MusicApp {
    pub(super) fn playlist_reorder_row(
        &self,
        row: Stateful<Div>,
        entry: &PlaylistEntry,
        title: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        if !self.playlists.current().is_some_and(Playlist::can_reorder) {
            return row;
        }
        let colors = ink(cx);
        let target = entry.clone();
        let drop_target = target.clone();
        row.cursor_grab()
            .on_drag(
                PlaylistDrag {
                    entry: entry.clone(),
                    title,
                },
                |drag, _, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| tracks::TrackDragPreview(drag.title.clone()))
                },
            )
            .drag_over::<PlaylistDrag>(move |style, drag, _, _| {
                match drag.entry.drop_target(&target) {
                    Some(std::cmp::Ordering::Less) => {
                        style.border_b_2().border_color(colors.accent)
                    }
                    Some(std::cmp::Ordering::Greater) => {
                        style.border_t_2().border_color(colors.accent)
                    }
                    _ => style,
                }
            })
            .on_drop(cx.listener(move |app, drag: &PlaylistDrag, _, cx| {
                if let Some(current) = app.playlists.current()
                    && let Some(mutation) = drag.entry.drop_on(current, &drop_target)
                {
                    app.mutate(mutation, cx);
                }
            }))
    }

    pub(super) fn playlists_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        if let Some(playlist) = self.playlists.current().cloned() {
            let tracks = playlist.entry.clone();
            let values = self.playlists.visible_entries();
            let count = values.len();
            let entity = cx.entity();
            let play = tracks;
            let weak = cx.weak_entity();
            let rename = playlist.name.clone();
            let rename_id = playlist.id.clone();
            let delete_id = playlist.id.clone();
            let append = self.playlists.clone();
            let editable = self
                .playlists
                .editable(self.account.as_ref().map(|a| a.username.as_str()));
            let owned = crate::playlists::is_owner(
                &playlist,
                self.account.as_ref().map(|a| a.username.as_str()),
            );
            return div()
                .size_full()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(34.))
                        .px_2()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .border_b_1()
                        .border_color(colors.line)
                        .child(
                            Button::new("play-playlist")
                                .ghost()
                                .small()
                                .icon(Glyph::Play)
                                .label("Play")
                                .on_click(cx.listener(move |app, _, _, _| {
                                    app.service
                                        .player
                                        .send(PlayerCommand::Replace(play.as_ref().clone(), 0))
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(10.))
                                .text_color(colors.muted)
                                .child(format!(
                                    "{} tracks · {}",
                                    playlist.song_count,
                                    duration_label(playlist.duration)
                                )),
                        )
                        .child(
                            div().when(
                                crate::playlists::is_smart(
                                    &playlist,
                                    self.account
                                        .as_ref()
                                        .map(|account| account.username.as_str()),
                                ),
                                |header| {
                                    header.child(
                                        div()
                                            .id("smart-playlist-header")
                                            .test_support()
                                            .aria_label("Smart playlist · updates automatically")
                                            .text_size(px(10.))
                                            .text_color(colors.muted)
                                            .child("Smart playlist · updates automatically"),
                                    )
                                },
                            ),
                        )
                        .when(owned, |header| {
                            header.child(
                                Button::new("playlist-options")
                                    .ghost()
                                    .compact()
                                    .icon(Glyph::Ellipsis)
                                    .tooltip("Playlist options")
                                    .dropdown_menu(move |menu, _, _| {
                                        let weak = weak.clone();
                                        let weak2 = weak.clone();
                                        let weak3 = weak.clone();
                                        let name = rename.clone();
                                        let rename_id = rename_id.clone();
                                        let delete_id = delete_id.clone();
                                        let append = append.clone();
                                        menu.item(PopupMenuItem::new("Rename…").on_click(
                                            move |_, window, cx| {
                                                let _ = weak.update(cx, |app, cx| {
                                                    if app
                                                        .playlists
                                                        .current()
                                                        .is_some_and(|p| p.id == rename_id)
                                                    {
                                                        app.show_playlist_editor = true;
                                                        app.playlist_editor_rename = true;
                                                        app.playlist_name.update(
                                                            cx,
                                                            |input, cx| {
                                                                input.set_value(
                                                                    name.clone(),
                                                                    window,
                                                                    cx,
                                                                )
                                                            },
                                                        );
                                                        cx.notify();
                                                    }
                                                });
                                            },
                                        ))
                                        .when(editable, |menu| {
                                            menu.item(PopupMenuItem::new("Append Queue").on_click(
                                                move |_, _, cx| {
                                                    let _ = weak2.update(cx, |app, cx| {
                                                        let ids = app
                                                            .playback
                                                            .queue
                                                            .iter()
                                                            .map(|t| t.id.clone())
                                                            .collect();
                                                        if let Some(mutation) =
                                                            append.append_queue(ids)
                                                        {
                                                            app.mutate(mutation, cx);
                                                        }
                                                    });
                                                },
                                            ))
                                        })
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("Delete playlist…").on_click(
                                                move |_, _, cx| {
                                                    let _ = weak3.update(cx, |app, cx| {
                                                        app.pending_delete =
                                                            Some(delete_id.clone());
                                                        cx.notify();
                                                    });
                                                },
                                            ),
                                        )
                                    }),
                            )
                        }),
                )
                .child(
                    uniform_list("playlist-tracks", count, move |range, _, cx| {
                        entity.update(cx, |app, cx| {
                            range
                                .map(|row| {
                                    let (index, track) = values[row].clone();
                                    app.track_row(
                                        track,
                                        index + 1,
                                        format!("playlist-{index}"),
                                        Some(tracks::TrackPosition::Playlist {
                                            playback: tracks::TrackPlayback::Playlist(
                                                playlist.entry.clone(),
                                                index,
                                            ),
                                            editable_entry: editable
                                                .then(|| PlaylistEntry::new(&playlist, index)),
                                        }),
                                        None,
                                        cx,
                                    )
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .flex_1()
                    .size_full()
                    .px_2(),
                )
                .into_any_element();
        }
        let mut items = div().flex().flex_col().p_2().gap_1();
        for playlist in self.playlists.summaries().to_vec() {
            let name = playlist.name.clone();
            items = items.child(
                div()
                    .id(SharedString::from(format!("playlist-list-{}", playlist.id)))
                    .h(px(35.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.panel))
                    .child(
                        Icon::new(Glyph::ListMusic)
                            .size_4()
                            .text_color(colors.muted),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(
                        div()
                            .text_color(colors.muted)
                            .text_size(px(10.))
                            .child(format!("{} tracks", playlist.song_count)),
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        let id = app.playlists.open(playlist.clone());
                        app.service.send(Command::Playlist(id));
                        cx.notify();
                    })),
            );
        }
        div()
            .id("playlist-manager")
            .size_full()
            .overflow_y_scroll()
            .child(items)
            .into_any_element()
    }
}
