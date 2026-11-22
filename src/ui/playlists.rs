use super::*;
use crate::playlists::{PlaylistEntry, PlaylistView};

#[derive(Clone)]
struct PlaylistDrag {
    entry: PlaylistEntry,
    title: String,
}

impl MusicApp {
    pub(super) fn playlist_menu(
        &self,
        playlist: &Playlist,
        cx: &Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let app = cx.weak_entity();
        let id = playlist.id.clone();
        let namespace = self.account.as_ref().map(Account::namespace);
        let delete = self.playlist_delete_action(id.clone(), cx);
        move |menu, _, cx| {
            let Some(view) = app.upgrade() else {
                return menu;
            };
            let view = view.read(cx);
            let Some(playlist) = view.playlists.by_id(&id) else {
                return menu;
            };
            let owned = crate::playlists::is_owner(
                playlist,
                view.account
                    .as_ref()
                    .map(|account| account.username.as_str()),
            );
            let (app, id, namespace) = (app.clone(), id.clone(), namespace.clone());
            context_menu::queue_options(
                menu.min_w(px(220.)),
                &view.queue_places(),
                move |placement, cx| {
                    let _ = app.update(cx, |app, _| {
                        if app.account.as_ref().map(Account::namespace) == namespace
                            && app.playlists.by_id(&id).is_some()
                        {
                            app.service
                                .send(Command::EnqueuePlaylist(id.clone(), placement));
                        }
                    });
                },
            )
            .when(owned, |menu| {
                menu.separator()
                    .item(PopupMenuItem::new("Delete playlist…").on_click(delete.clone()))
            })
        }
    }

    fn playlist_delete_action(
        &self,
        id: String,
        cx: &Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static {
        let app = cx.weak_entity();
        let namespace = self.account.as_ref().map(Account::namespace);
        move |_, _, cx| {
            let _ = app.update(cx, |app, cx| {
                if app.account.as_ref().map(Account::namespace) == namespace
                    && app.playlists.request_delete(
                        &id,
                        app.account
                            .as_ref()
                            .map(|account| account.username.as_str()),
                    )
                {
                    cx.notify();
                }
            });
        }
    }

    pub(super) fn playlist_delete_confirmation(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let playlist = self.playlists.pending_deletion(
            self.account
                .as_ref()
                .map(|account| account.username.as_str()),
        )?;
        let prompt = format!("Delete “{}” from Navidrome?", playlist.name);
        Some(
            div()
                .p_2()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .bg(ink(cx).panel)
                .child(
                    div()
                        .id("playlist-delete-prompt")
                        .min_w_0()
                        .test_support()
                        .aria_label(prompt.clone())
                        .child(prompt),
                )
                .child(
                    Button::new("confirm-delete")
                        .small()
                        .danger()
                        .label("Delete")
                        .on_click(cx.listener(|app, _, _, cx| {
                            let user = app
                                .account
                                .as_ref()
                                .map(|account| account.username.as_str());
                            if let Some(mutation) = app.playlists.confirm_delete(user) {
                                app.mutate(mutation, cx);
                            } else {
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Button::new("cancel-delete")
                        .small()
                        .label("Cancel")
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.playlists.cancel_delete();
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    fn playlist_track_row(
        &mut self,
        playlist: &Playlist,
        index: usize,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.track_row(
            playlist.entry[index].clone(),
            index + 1,
            format!("playlist-{index}"),
            Some(tracks::TrackPosition::playlist(playlist, index, editable)),
            None,
            cx,
        )
    }

    fn playlist_albums(
        &mut self,
        playlist: Playlist,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let albums = self.playlists.visible_albums();
        if albums.is_empty() {
            return div()
                .id("playlist-empty")
                .test_support()
                .aria_label("No tracks match this playlist view")
                .p_4()
                .text_size(px(11.))
                .text_color(ink(cx).muted)
                .child(if playlist.entry.is_empty() {
                    "This playlist is empty."
                } else {
                    "No tracks match your search."
                })
                .into_any_element();
        }
        let (columns, side) = cover_layout(self.library_width);
        let rows = albums.len().div_ceil(columns);
        if self.playlist_album_list.item_count() != rows {
            self.playlist_album_list.reset(rows);
        }
        // Expanding a card, a refresh, and resizing can all change row heights.
        self.playlist_album_list.remeasure();
        let entity = cx.entity();
        list(self.playlist_album_list.clone(), move |row, _, cx| {
            entity.update(cx, |app, cx| {
                let mut cards = div().w_full().min_w_0().flex().gap_4().px_3().pt_3();
                let range = row * columns..((row + 1) * columns).min(albums.len());
                for index in range.clone() {
                    let album = &albums[index];
                    let selected = app.playlists.expanded_album() == Some(album.key.as_str());
                    let key = album.key.clone();
                    let play: Vec<_> = album
                        .entries
                        .iter()
                        .map(|&i| playlist.entry[i].clone())
                        .collect();
                    let art = app.art(&album.track.cover_art, side, cx);
                    cards = cards.child(
                        div()
                            .id(SharedString::from(format!("playlist-album-{index}")))
                            .test_support()
                            .role(Role::Button)
                            .aria_label(format!(
                                "{} by {} · {} playlist tracks",
                                album.track.album,
                                album.track.album_artist_label(),
                                album.entries.len()
                            ))
                            .aria_selected(selected)
                            .w(px(side))
                            .flex_shrink_0()
                            .cursor_pointer()
                            .pb_2()
                            .border_b_2()
                            .border_color(if selected {
                                ink(cx).accent
                            } else {
                                ink(cx).background
                            })
                            .child(art)
                            .child(
                                div()
                                    .mt_1()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(if album.track.album.is_empty() {
                                        "Unknown album".to_string()
                                    } else {
                                        album.track.album.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(ink(cx).muted)
                                    .truncate()
                                    .child(album.track.album_artist_label().to_string()),
                            )
                            .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                                if event.click_count() == 2 {
                                    app.service
                                        .player
                                        .send(PlayerCommand::Replace(play.clone(), 0));
                                } else if let Some(id) = app.playlists.toggle_album(key.clone()) {
                                    app.service.send(Command::AlbumDetails(id.to_owned()));
                                }
                                cx.notify();
                            })),
                    );
                }
                let mut item = div().w_full().min_w_0().flex().flex_col().child(cards);
                if let Some(album) = albums[range]
                    .iter()
                    .find(|album| app.playlists.expanded_album() == Some(album.key.as_str()))
                {
                    item = item.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .mt_2()
                            .border_y_1()
                            .border_color(ink(cx).line)
                            .child(app.playlist_album_panel(album, &playlist, editable, cx)),
                    );
                }
                item.into_any_element()
            })
        })
        .size_full()
        .into_any_element()
    }

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

    pub(super) fn playlist_rename_view(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let id = self.playlist_rename.clone()?;
        Some(
            div()
                .p_2()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .child(Input::new(&self.playlist_name).id("playlist-name").small()),
                )
                .child(
                    Button::new("save-playlist-editor")
                        .small()
                        .label("Rename")
                        .on_click(cx.listener(move |app, _, _, cx| {
                            let name = app.playlist_name.read(cx).value().to_string();
                            if !name.trim().is_empty() {
                                app.mutate(Mutation::RenamePlaylist(id.clone(), name), cx);
                            }
                        })),
                )
                .child(
                    short_button("cancel-playlist-editor", Glyph::X, "Cancel").on_click(
                        cx.listener(|app, _, _, cx| {
                            app.playlist_rename = None;
                            cx.notify();
                        }),
                    ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn playlists_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        if let Some(playlist) = self.playlists.current().cloned() {
            let tracks = playlist.entry.clone();
            let play = tracks;
            let weak = cx.weak_entity();
            let rename = playlist.name.clone();
            let rename_id = playlist.id.clone();
            let delete = self.playlist_delete_action(playlist.id.clone(), cx);
            let append = self.playlists.clone();
            let editable = self
                .playlists
                .editable(self.account.as_ref().map(|a| a.username.as_str()));
            let owned = crate::playlists::is_owner(
                &playlist,
                self.account.as_ref().map(|a| a.username.as_str()),
            );
            let edit_rules = self.smart.available
                && crate::playlists::is_smart(
                    &playlist,
                    self.account.as_ref().map(|a| a.username.as_str()),
                );
            let body = if self.playlists.view() == PlaylistView::Albums {
                self.playlist_albums(playlist.clone(), editable, cx)
            } else {
                let values = self.playlists.visible_entries();
                let entity = cx.entity();
                let playlist = playlist.clone();
                uniform_list("playlist-tracks", values.len(), move |range, _, cx| {
                    entity.update(cx, |app, cx| {
                        range
                            .map(|row| {
                                app.playlist_track_row(&playlist, values[row].0, editable, cx)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .flex_1()
                .size_full()
                .px_2()
                .into_any_element()
            };
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
                                .disabled(play.is_empty())
                                .on_click(cx.listener(move |app, _, _, _| {
                                    if !play.is_empty() {
                                        app.service
                                            .player
                                            .send(PlayerCommand::Replace(play.as_ref().clone(), 0));
                                    }
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
                        .children(
                            [
                                ("playlist-view-tracks", "Tracks", PlaylistView::Tracks),
                                ("playlist-view-albums", "Albums", PlaylistView::Albums),
                            ]
                            .map(|(id, label, view)| {
                                Button::new(id)
                                    .ghost()
                                    .small()
                                    .label(label)
                                    .selected(self.playlists.view() == view)
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.playlists.set_view(view);
                                        cx.notify();
                                    }))
                            }),
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
                                        let weak_rules = weak.clone();
                                        let name = rename.clone();
                                        let rename_id = rename_id.clone();
                                        let append = append.clone();
                                        menu.item(PopupMenuItem::new("Rename…").on_click(
                                            move |_, window, cx| {
                                                let _ = weak.update(cx, |app, cx| {
                                                    if app
                                                        .playlists
                                                        .current()
                                                        .is_some_and(|p| p.id == rename_id)
                                                    {
                                                        app.playlist_rename =
                                                            Some(rename_id.clone());
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
                                        .when(edit_rules, |menu| {
                                            menu.item(PopupMenuItem::new("Edit Rules…").on_click(
                                                move |_, window, cx| {
                                                    let _ = weak_rules.update(cx, |app, cx| {
                                                        app.edit_smart_playlist(window, cx)
                                                    });
                                                },
                                            ))
                                        })
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
                                            PopupMenuItem::new("Delete playlist…")
                                                .on_click(delete.clone()),
                                        )
                                    }),
                            )
                        }),
                )
                .child(body)
                .into_any_element();
        }
        let mut items = div().flex().flex_col().p_2().gap_1();
        for playlist in self.playlists.summaries().to_vec() {
            let name = playlist.name.clone();
            let menu = self.playlist_menu(&playlist, cx);
            items = items.child(
                div()
                    .id(SharedString::from(format!("playlist-list-{}", playlist.id)))
                    .test_support()
                    .role(Role::Button)
                    .aria_label(name.clone())
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
                    }))
                    .context_menu(menu),
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
