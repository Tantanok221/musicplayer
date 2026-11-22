use super::*;

#[derive(Clone)]
pub(super) struct PlaylistEntry {
    id: String,
    index: usize,
    entries: Arc<Vec<Arc<Track>>>,
}

impl PlaylistEntry {
    fn new(playlist: &Playlist, index: usize) -> Self {
        Self {
            id: playlist.id.clone(),
            index,
            entries: playlist.entry.clone(),
        }
    }

    pub(super) fn mutation(&self, current: Option<&Playlist>) -> Option<Mutation> {
        let current = current?;
        self.matches(current)
            .then(|| Mutation::RemoveFromPlaylist(self.id.clone(), self.index))
    }

    fn matches(&self, current: &Playlist) -> bool {
        current.id == self.id
            && self.index < current.entry.len()
            && current
                .track_ids()
                .eq(self.entries.iter().map(|track| track.id.as_str()))
    }

    fn move_to(&self, current: &Playlist, destination: usize) -> Option<Mutation> {
        if destination == self.index
            || !current.can_reorder()
            || !self.matches(current)
            || destination >= current.entry.len()
        {
            return None;
        }
        let original: Vec<_> = current.track_ids().map(str::to_owned).collect();
        let mut reordered = original.clone();
        let song = reordered.remove(self.index);
        reordered.insert(destination, song);
        (original != reordered).then(|| Mutation::ReorderPlaylist {
            id: self.id.clone(),
            original,
            reordered,
        })
    }
}

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
        if !self.playlist.as_ref().is_some_and(Playlist::can_reorder) {
            return row;
        }
        let colors = ink(cx);
        let target_id = entry.id.clone();
        let drop_id = target_id.clone();
        let index = entry.index;
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
                if drag.entry.id != target_id || drag.entry.index == index {
                    style
                } else if drag.entry.index < index {
                    style.border_b_2().border_color(colors.accent)
                } else {
                    style.border_t_2().border_color(colors.accent)
                }
            })
            .on_drop(cx.listener(move |app, drag: &PlaylistDrag, _, cx| {
                if drag.entry.id == drop_id
                    && let Some(current) = app.playlist.as_ref()
                    && let Some(mutation) = drag.entry.move_to(current, index)
                {
                    app.mutate(mutation, cx);
                }
            }))
    }

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
            let editable = playlist.owner.is_empty()
                || self
                    .account
                    .as_ref()
                    .is_some_and(|account| account.username == playlist.owner);
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
                                                let ids = app
                                                    .playback
                                                    .queue
                                                    .iter()
                                                    .map(|t| t.id.clone())
                                                    .collect();
                                                app.mutate(
                                                    Mutation::AddToPlaylist(save_id.clone(), ids),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(ids: &[&str]) -> Playlist {
        Playlist {
            id: "playlist".into(),
            song_count: ids.len() as u32,
            entry: Arc::new(
                ids.iter()
                    .map(|id| {
                        Arc::new(Track {
                            id: (*id).into(),
                            ..Default::default()
                        })
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }

    #[::core::prelude::v1::test]
    fn removal_keeps_duplicate_entry_indices_and_rejects_stale_or_empty_playlists() {
        let original = playlist(&["a", "b", "a"]);
        let removal = PlaylistEntry::new(&original, 2);
        assert_eq!(
            removal.mutation(Some(&original)),
            Some(Mutation::RemoveFromPlaylist("playlist".into(), 2))
        );
        assert!(removal.mutation(None).is_none());
        assert!(removal.mutation(Some(&playlist(&[]))).is_none());
        assert!(
            removal
                .mutation(Some(&playlist(&["a", "a", "b"])))
                .is_none()
        );
        let mut other = original.clone();
        other.id = "other".into();
        assert!(removal.mutation(Some(&other)).is_none());
        assert!(
            PlaylistEntry::new(&original, 3)
                .mutation(Some(&original))
                .is_none()
        );
    }

    #[::core::prelude::v1::test]
    fn reorder_uses_full_indices_and_rejects_changed_incomplete_or_unchanged_order() {
        let original = playlist(&["a", "b", "a", "c"]);
        let source = PlaylistEntry::new(&original, 2);
        let expected = |ids: [&str; 4]| Mutation::ReorderPlaylist {
            id: "playlist".into(),
            original: ["a", "b", "a", "c"].map(String::from).into(),
            reordered: ids.map(String::from).into(),
        };
        assert_eq!(
            source.move_to(&original, 3),
            Some(expected(["a", "b", "c", "a"]))
        );
        assert_eq!(
            source.move_to(&original, 1),
            Some(expected(["a", "a", "b", "c"]))
        );
        assert!(source.move_to(&original, 2).is_none());
        assert!(source.move_to(&original, 4).is_none());
        assert!(
            source
                .move_to(&playlist(&["a", "c", "a", "b"]), 3)
                .is_none()
        );
        let mut incomplete = original;
        incomplete.song_count += 1;
        assert!(source.move_to(&incomplete, 3).is_none());
    }
}
