use super::*;

impl MusicApp {
    pub(super) fn playlists_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        if let Some(playlist) = self.playlist.clone() {
            let tracks = playlist.entry.clone();
            let values = tracks
                .iter()
                .enumerate()
                .filter(|(_, track)| {
                    normalize(&format!(
                        "{} {} {}",
                        track.title,
                        track.artist_label(),
                        track.album
                    ))
                    .contains(&normalize(&self.playlist_search))
                })
                .map(|(i, t)| (i, t.clone()))
                .collect::<Vec<_>>();
            let count = values.len();
            let entity = cx.entity();
            let play = tracks;
            let weak = cx.weak_entity();
            let rename = playlist.name.clone();
            let rename_id = playlist.id.clone();
            let delete_id = playlist.id.clone();
            let save_id = playlist.id.clone();
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
                                    let save_id = save_id.clone();
                                    menu.item(PopupMenuItem::new("Rename…").on_click(
                                        move |_, window, cx| {
                                            let _ = weak.update(cx, |app, cx| {
                                                if app
                                                    .playlist
                                                    .as_ref()
                                                    .is_some_and(|p| p.id == rename_id)
                                                {
                                                    app.show_playlist_editor = true;
                                                    app.playlist_editor_rename = true;
                                                    app.playlist_name.update(cx, |input, cx| {
                                                        input.set_value(name.clone(), window, cx)
                                                    });
                                                    cx.notify();
                                                }
                                            });
                                        },
                                    ))
                                    .item(PopupMenuItem::new("Append Queue").on_click(
                                        move |_, _, cx| {
                                            let _ = weak2.update(cx, |app, cx| {
                                                let mut ids =
                                                    app.playlist.as_ref().map_or(vec![], |p| {
                                                        p.entry
                                                            .iter()
                                                            .map(|t| t.id.clone())
                                                            .collect::<Vec<_>>()
                                                    });
                                                ids.extend(
                                                    app.playback.queue.iter().map(|t| t.id.clone()),
                                                );
                                                app.mutate(
                                                    Mutation::ReplacePlaylist(save_id.clone(), ids),
                                                    cx,
                                                );
                                            });
                                        },
                                    ))
                                    .separator()
                                    .item(
                                        PopupMenuItem::new("Delete playlist…").on_click(
                                            move |_, _, cx| {
                                                let _ = weak3.update(cx, |app, cx| {
                                                    app.pending_delete = Some(delete_id.clone());
                                                    cx.notify();
                                                });
                                            },
                                        ),
                                    )
                                }),
                        ),
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
                                        None,
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
        for playlist in self.playlists.clone() {
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
                        app.playlist = Some(playlist.clone());
                        app.service.send(Command::Playlist(playlist.id.clone()));
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
