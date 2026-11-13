use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
enum QueueRow {
    Earlier(usize),
    Heading(&'static str),
    Album(usize, bool),
    Track(usize),
    Empty,
    End,
}

fn next_index(playback: &PlaybackSnapshot) -> usize {
    playback
        .index
        .map_or(0, |i| (i + 1).min(playback.queue.len()))
}

fn queue_rows(playback: &PlaybackSnapshot, earlier: bool, search: &str) -> Vec<QueueRow> {
    let current = playback.index.filter(|i| *i < playback.queue.len());
    let search = normalize(search.trim());
    let matches = |track: &Track| {
        search.is_empty()
            || normalize(&format!(
                "{} {} {}",
                track.title,
                track.artist_label(),
                track.album
            ))
            .contains(&search)
    };
    let mut rows = Vec::new();
    if let Some(current) = current {
        if current > 0 {
            rows.push(QueueRow::Earlier(current));
            if earlier || !search.is_empty() {
                rows.extend(
                    (0..current)
                        .filter(|i| matches(&playback.queue[*i]))
                        .map(QueueRow::Track),
                );
            }
        }
        rows.push(QueueRow::Heading("Now playing"));
        rows.push(QueueRow::Track(current));
    }
    rows.push(QueueRow::Heading("Up next"));
    let mut found = false;
    for index in next_index(playback)..playback.queue.len() {
        let track = &playback.queue[index];
        if !matches(track) {
            continue;
        }
        // Mark only consecutive album runs, preserving the actual playback order.
        if !track.album_id.is_empty()
            && (index == next_index(playback)
                || playback.queue[index - 1].album_id != track.album_id)
        {
            let continuing = current.is_some_and(|i| playback.queue[i].album_id == track.album_id)
                && index == next_index(playback);
            rows.push(QueueRow::Album(index, continuing));
        }
        rows.push(QueueRow::Track(index));
        found = true;
    }
    if !found {
        rows.push(QueueRow::Empty);
    } else {
        rows.push(QueueRow::End);
    }
    rows
}

pub(super) struct QueueViewState {
    pub focus: FocusHandle,
    list: ListState,
    selected: HashSet<u64>,
    anchor: Option<u64>,
    cursor: Option<u64>,
    earlier: bool,
    rows: Vec<QueueRow>,
    signature: Option<(u64, Option<usize>, bool, String)>,
}
impl QueueViewState {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle(),
            list: ListState::new(0, ListAlignment::Top, px(200.)),
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            earlier: false,
            rows: vec![],
            signature: None,
        }
    }
    pub fn retain_selection(&mut self, ids: &[u64]) {
        if !self.selected.is_empty() {
            let present: HashSet<_> = ids.iter().copied().collect();
            self.selected.retain(|id| present.contains(id));
        }
        if self.anchor.is_some_and(|id| !ids.contains(&id)) {
            self.anchor = None;
        }
        if self.cursor.is_some_and(|id| !ids.contains(&id)) {
            self.cursor = None;
        }
    }
    fn prepare(&mut self, playback: &PlaybackSnapshot, search: &str) {
        let signature = (
            playback.queue_revision,
            playback.index,
            self.earlier,
            search.to_string(),
        );
        if self.signature.as_ref() == Some(&signature) {
            return;
        }
        let offset = self.list.logical_scroll_top();
        let anchor = self.rows.get(offset.item_ix).cloned();
        self.rows = queue_rows(playback, self.earlier, search);
        self.list.reset(self.rows.len());
        if offset.item_ix > 0
            && let Some(at) = anchor.and_then(|row| self.rows.iter().position(|r| *r == row))
        {
            self.list.scroll_to(ListOffset {
                item_ix: at,
                offset_in_item: offset.offset_in_item,
            });
        }
        self.signature = Some(signature);
    }
}

#[derive(Clone)]
struct QueueDrag {
    revision: u64,
    ids: Vec<u64>,
    first_index: usize,
    title: String,
}
struct QueueDragPreview(String);
impl Render for QueueDragPreview {
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
    /// Shows the track's album, selected and expanded, in Music Explorer.
    pub(super) fn open_queue_album(
        &mut self,
        track: &Track,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if track.album_id.is_empty() {
            return;
        }
        // Switching workspaces records history itself, but not within Explorer.
        if self.workspace == Workspace::Explorer && self.artist_name.is_none() && !self.blank {
            self.remember_location();
        }
        self.switch_workspace(Workspace::Explorer, window, cx);
        self.query = LibraryQuery {
            selection: BrowserSelection {
                albums: vec![track.album_id.clone()],
                ..Default::default()
            },
            ..Default::default()
        };
        self.search_text.clear();
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.request_query(cx);
        self.pending_explorer_album = Some(track.album_id.clone());
        self.focus.focus(window, cx);
    }
    fn queue_album_link(
        &self,
        track: Arc<Track>,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Button::new(SharedString::from(key))
            .ghost()
            .xsmall()
            .compact()
            .px_0()
            .min_w_0()
            .max_w_full()
            .cursor_pointer()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(ink(cx).text)
                    .truncate()
                    .child(track.album.clone()),
            )
            .accessibility_label(format!("Open album {}", track.album))
            .disabled(track.album_id.is_empty())
            .on_click(cx.listener(move |app, _, window, cx| {
                cx.stop_propagation();
                app.open_queue_album(&track, window, cx);
            }))
            .into_any_element()
    }
    pub(super) fn queue_summary(&self) -> String {
        let start = next_index(&self.playback);
        let upcoming = &self.playback.queue[start..];
        let seconds = upcoming.iter().map(|t| t.duration.max(0.)).sum::<f64>()
            + (self.playback.duration - self.playback.position).max(0.);
        format!(
            "{} upcoming · {} {}",
            upcoming.len(),
            duration_label(seconds),
            if self.preferences.repeat == RepeatMode::Off {
                "remaining"
            } else {
                "this pass"
            }
        )
    }
    fn queue_edit(&self, edit: QueueEdit) {
        self.service
            .player
            .send(PlayerCommand::EditQueue(self.playback.queue_revision, edit));
    }
    fn queue_edit_at(&self, revision: u64, edit: QueueEdit) {
        self.service
            .player
            .send(PlayerCommand::EditQueue(revision, edit));
    }
    fn selected_queue_indices(&self) -> Vec<usize> {
        self.playback
            .queue_ids
            .iter()
            .enumerate()
            .filter(|(index, id)| {
                *index >= next_index(&self.playback) && self.queue_ui.selected.contains(id)
            })
            .map(|(index, _)| index)
            .collect()
    }
    fn queue_select(
        &mut self,
        index: usize,
        multiple: bool,
        range: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.playback.queue_ids.get(index).copied() else {
            return;
        };
        if range
            && let Some(anchor) = self.queue_ui.anchor.and_then(|id| {
                self.playback
                    .queue_ids
                    .iter()
                    .position(|value| *value == id)
            })
        {
            if !multiple {
                self.queue_ui.selected.clear();
            }
            self.queue_ui
                .selected
                .extend(self.queue_ui.rows.iter().filter_map(|row| match row {
                    QueueRow::Track(i) if (anchor.min(index)..=anchor.max(index)).contains(i) => {
                        self.playback.queue_ids.get(*i).copied()
                    }
                    _ => None,
                }));
        } else if multiple {
            if !self.queue_ui.selected.remove(&id) {
                self.queue_ui.selected.insert(id);
            }
            self.queue_ui.anchor = Some(id);
        } else {
            self.queue_ui.selected.clear();
            self.queue_ui.selected.insert(id);
            self.queue_ui.anchor = Some(id);
        }
        self.queue_ui.cursor = Some(id);
        if let Some(track) = self.playback.queue.get(index).cloned() {
            self.select_track(track, cx);
        }
        self.queue_ui.focus.focus(window, cx);
        cx.notify();
    }
    fn queue_step_selection(
        &mut self,
        forward: bool,
        range: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let visible = self
            .queue_ui
            .rows
            .iter()
            .filter_map(|row| {
                if let QueueRow::Track(index) = row {
                    Some(*index)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let anchor = self
            .queue_ui
            .cursor
            .or(self.queue_ui.anchor)
            .and_then(|id| {
                self.playback
                    .queue_ids
                    .iter()
                    .position(|value| *value == id)
            });
        let current = anchor.and_then(|index| visible.iter().position(|i| *i == index));
        let at = current.map_or(0, |i| {
            if forward {
                (i + 1).min(visible.len().saturating_sub(1))
            } else {
                i.saturating_sub(1)
            }
        });
        if let Some(index) = visible.get(at).copied() {
            self.queue_select(index, false, range, window, cx);
            // Move the keyboard cursor independently of the range's selection anchor.
            if !range {
                self.queue_ui.anchor = self.playback.queue_ids.get(index).copied();
            }
            if let Some(row) = self
                .queue_ui
                .rows
                .iter()
                .position(|r| *r == QueueRow::Track(index))
            {
                self.queue_ui.list.scroll_to_reveal_item(row);
            }
        }
    }
    fn queue_remove_selection(&self) {
        self.queue_edit(QueueEdit::Remove(self.selected_queue_indices()));
    }
    fn queue_move_selection(&self, later: bool) {
        let indices = self.selected_queue_indices();
        if let (Some(first), Some(last)) = (indices.first(), indices.last()) {
            let before = if later {
                (last + 2).min(self.playback.queue.len())
            } else {
                first.saturating_sub(1).max(next_index(&self.playback))
            };
            self.queue_edit(QueueEdit::Move { indices, before });
        }
    }
    fn save_queue_playlist(&mut self, upcoming: bool, cx: &mut Context<Self>) {
        let start = if upcoming {
            next_index(&self.playback)
        } else {
            0
        };
        self.playlist_editor_tracks = Some(
            self.playback.queue[start..]
                .iter()
                .map(|t| t.id.clone())
                .collect(),
        );
        self.show_playlist_editor = true;
        self.playlist_editor_rename = false;
        self.playlist_editor_album = None;
        cx.notify();
    }
    pub(super) fn queue_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let empty = self.playback.queue.is_empty();
        let no_upcoming = next_index(&self.playback) == self.playback.queue.len();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                short_button(
                    "save-queue-playlist",
                    Glyph::ListPlus,
                    "Save queue as playlist…",
                )
                .disabled(empty)
                .dropdown_menu(move |menu, _, _| {
                    let all = weak.clone();
                    let upcoming = weak.clone();
                    menu.item(
                        PopupMenuItem::new("Whole queue…").on_click(move |_, _, cx| {
                            let _ = all.update(cx, |app, cx| app.save_queue_playlist(false, cx));
                        }),
                    )
                    .item(
                        PopupMenuItem::new("Upcoming tracks…")
                            .disabled(no_upcoming)
                            .on_click(move |_, _, cx| {
                                let _ = upcoming
                                    .update(cx, |app, cx| app.save_queue_playlist(true, cx));
                            }),
                    )
                }),
            )
            .child(
                short_button("clear-upcoming", Glyph::ListX, "Clear upcoming tracks")
                    .disabled(no_upcoming)
                    .on_click(cx.listener(|app, _, _, _| app.queue_edit(QueueEdit::ClearUpcoming))),
            )
            .child(
                short_button(
                    "jump-to-playing",
                    Glyph::LocateFixed,
                    "Scroll to the playing track",
                )
                .disabled(self.playback.index.is_none())
                .on_click(cx.listener(|app, _, _, cx| {
                    if let Some(index) = app.playback.index
                        && let Some(row) = app
                            .queue_ui
                            .rows
                            .iter()
                            .position(|r| *r == QueueRow::Track(index))
                    {
                        app.queue_ui.list.scroll_to(ListOffset {
                            item_ix: row.saturating_sub(1),
                            offset_in_item: px(0.),
                        });
                        cx.notify();
                    }
                })),
            )
            .child(
                short_button("undo-queue-edit", Glyph::Undo2, "Undo queue edit")
                    .disabled(!self.playback.can_undo_queue)
                    .on_click(cx.listener(|app, _, _, _| {
                        app.service.player.send(PlayerCommand::UndoQueue)
                    })),
            )
            .into_any_element()
    }
    pub(super) fn queue_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.queue_ui.prepare(&self.playback, &self.queue_search);
        if self.playback.queue.is_empty() {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(ink(cx).muted)
                .child(Icon::new(Glyph::ListMusic).size_7())
                .child("Your queue is empty")
                .child(
                    Button::new("browse-empty-queue")
                        .ghost()
                        .small()
                        .label("Browse library")
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.switch_workspace(Workspace::Explorer, window, cx)
                        })),
                )
                .into_any_element();
        }
        let entity = cx.entity();
        div()
            .size_full()
            .min_h_0()
            .key_context("Queue")
            .track_focus(&self.queue_ui.focus)
            .on_action(cx.listener(|app, _: &UndoQueueEdit, _, _| {
                app.service.player.send(PlayerCommand::UndoQueue)
            }))
            .on_action(
                cx.listener(|app, _: &RemoveQueueSelection, _, _| app.queue_remove_selection()),
            )
            .on_action(
                cx.listener(|app, _: &MoveQueueEarlier, _, _| app.queue_move_selection(false)),
            )
            .on_action(cx.listener(|app, _: &MoveQueueLater, _, _| app.queue_move_selection(true)))
            .on_action(cx.listener(|app, _: &SelectAllQueued, _, cx| {
                app.queue_ui.selected = app
                    .queue_ui
                    .rows
                    .iter()
                    .filter_map(|row| {
                        if let QueueRow::Track(i) = row {
                            app.playback.queue_ids.get(*i).copied()
                        } else {
                            None
                        }
                    })
                    .collect();
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &NextQueueEntry, window, cx| {
                app.queue_step_selection(true, false, window, cx)
            }))
            .on_action(cx.listener(|app, _: &PreviousQueueEntry, window, cx| {
                app.queue_step_selection(false, false, window, cx)
            }))
            .on_action(cx.listener(|app, _: &ExtendQueueNext, window, cx| {
                app.queue_step_selection(true, true, window, cx)
            }))
            .on_action(cx.listener(|app, _: &ExtendQueuePrevious, window, cx| {
                app.queue_step_selection(false, true, window, cx)
            }))
            .on_action(cx.listener(|app, _: &PlayQueueSelection, _, _| {
                if let Some(index) = app
                    .queue_ui
                    .anchor
                    .and_then(|id| app.playback.queue_ids.iter().position(|value| *value == id))
                {
                    app.service.player.send(PlayerCommand::JumpTo(index));
                }
            }))
            .child(
                list(self.queue_ui.list.clone(), move |row, _, cx| {
                    entity.update(cx, |app, cx| app.queue_list_row(row, cx))
                })
                .size_full(),
            )
            .into_any_element()
    }
    fn queue_list_row(&mut self, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        match self.queue_ui.rows.get(row).cloned() {
            Some(QueueRow::Earlier(count)) => Button::new("earlier-queue")
                .ghost()
                .xsmall()
                .disabled(!self.queue_search.trim().is_empty())
                .icon(
                    if self.queue_ui.earlier || !self.queue_search.trim().is_empty() {
                        Glyph::ChevronDown
                    } else {
                        Glyph::ChevronRight
                    },
                )
                .label(format!("Earlier in queue · {count}"))
                .on_click(cx.listener(|app, _, _, cx| {
                    app.queue_ui.earlier = !app.queue_ui.earlier;
                    cx.notify();
                }))
                .into_any_element(),
            Some(QueueRow::Heading(label)) => div()
                .w_full()
                .min_w_0()
                .px_3()
                .pt_4()
                .pb_2()
                .flex()
                .flex_wrap()
                .gap_x_4()
                .gap_y_1()
                .items_center()
                .justify_between()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .child(div().flex_shrink_0().child(label))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(colors.muted)
                        .flex_shrink_0()
                        .child(if label == "Now playing" {
                            format!(
                                "Track {} of {}",
                                self.playback.index.unwrap_or(0) + 1,
                                self.playback.queue.len()
                            )
                        } else {
                            let upcoming = &self.playback.queue[next_index(&self.playback)..];
                            format!(
                                "{} tracks · {}",
                                upcoming.len(),
                                duration_label(upcoming.iter().map(|t| t.duration.max(0.)).sum())
                            )
                        }),
                )
                .into_any_element(),
            Some(QueueRow::Album(index, _)) => {
                let track = self.playback.queue[index].clone();
                div()
                    .w_full()
                    .min_w_0()
                    .px_3()
                    .pt_2()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(
                        div()
                            .w_5()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(Glyph::Disc3).size_3().text_color(colors.text)),
                    )
                    .child(div().flex_1().min_w_0().flex().items_center().child(
                        self.queue_album_link(
                            track.clone(),
                            format!("queue-album-heading-{index}"),
                            cx,
                        ),
                    ))
                    .into_any_element()
            }
            Some(QueueRow::Track(index)) => self.queue_track_row(index, cx),
            Some(QueueRow::Empty) => {
                let searching = !self.queue_search.trim().is_empty();
                div()
                    .px_3()
                    .py_4()
                    .text_sm()
                    .text_color(colors.muted)
                    .child(if searching {
                        "No upcoming tracks match your search."
                    } else {
                        "Nothing else queued. Browse your library to add more music."
                    })
                    .when(searching, |s| {
                        s.child(
                            Button::new("clear-queue-search")
                                .ghost()
                                .small()
                                .label("Clear search")
                                .on_click(cx.listener(|app, _, window, cx| {
                                    app.queue_search.clear();
                                    app.search
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                    cx.notify();
                                })),
                        )
                    })
                    .on_drop(cx.listener(|app, drag: &QueueDrag, _, _| {
                        app.drop_queue_tracks(drag, app.playback.queue.len())
                    }))
                    .into_any_element()
            }
            Some(QueueRow::End) => div()
                .id("queue-drop-end")
                .w_full()
                .px_3()
                .h_3()
                .role(Role::Group)
                .aria_label("Drop tracks at end of queue")
                .drag_over::<QueueDrag>(move |s, _, _, _| {
                    s.border_t_2().border_color(colors.accent)
                })
                .on_drop(cx.listener(|app, drag: &QueueDrag, _, _| {
                    app.drop_queue_tracks(drag, app.playback.queue.len())
                }))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }
    fn drop_queue_tracks(&self, drag: &QueueDrag, before: usize) {
        if drag.revision != self.playback.queue_revision {
            return;
        }
        let indices = drag
            .ids
            .iter()
            .filter_map(|id| self.playback.queue_ids.iter().position(|value| value == id))
            .collect();
        self.queue_edit(QueueEdit::Move { indices, before });
    }
    fn queue_track_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let track = self.playback.queue[index].clone();
        let Some(id) = self.playback.queue_ids.get(index).copied() else {
            return div().into_any_element();
        };
        let colors = ink(cx);
        let revision = self.playback.queue_revision;
        let active = self.playback.index == Some(index);
        let upcoming = index >= next_index(&self.playback);
        let selected = self.queue_ui.selected.contains(&id);
        let mut indices = self.selected_queue_indices();
        if !indices.contains(&index) {
            indices = vec![index];
        }
        let selected_ids = indices
            .iter()
            .filter_map(|i| self.playback.queue_ids.get(*i).copied())
            .collect::<Vec<_>>();
        let drag = QueueDrag {
            revision: self.playback.queue_revision,
            ids: selected_ids,
            first_index: indices[0],
            title: if indices.len() == 1 {
                track.title.clone()
            } else {
                format!("{} tracks", indices.len())
            },
        };
        let remove = indices.clone();
        let move_next = indices.clone();
        let weak = cx.weak_entity();
        let play_weak = weak.clone();
        let next_weak = weak.clone();
        let remove_weak = weak.clone();
        let earlier_weak = weak.clone();
        let later_weak = weak.clone();
        let menu_track = track.clone();
        let menu_state = self.menu_state(cx);
        let mut row = div()
            .id(SharedString::from(format!("queue-entry-{id}")))
            .role(Role::Group)
            .aria_label(format!(
                "Queue {}: {} · {}",
                index + 1,
                track.title,
                track.artist_label()
            ))
            .w_full()
            .min_w_0()
            .min_h_12()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .border_l_2()
            .border_color(if active {
                colors.accent
            } else {
                colors.background
            })
            .when(active, |s| s.bg(colors.panel))
            .when(selected, |s| s.bg(colors.selection))
            .when(!active && !selected, |s| s.hover(|s| s.bg(colors.panel)))
            .child(
                div()
                    .w_5()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(if active { colors.accent } else { colors.muted })
                    .child(if active {
                        Icon::new(if self.playback.playing {
                            Glyph::Volume2
                        } else {
                            Glyph::Pause
                        })
                        .size_4()
                        .into_any_element()
                    } else {
                        div()
                            .child(if upcoming {
                                (index - next_index(&self.playback) + 1).to_string()
                            } else {
                                (index + 1).to_string()
                            })
                            .into_any_element()
                    }),
            )
            .when(active, |s| s.child(self.art(&track.cover_art, 40., cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(if active {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(if active { colors.accent } else { colors.text })
                            .truncate()
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted)
                            .min_w_0()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_x_1()
                            .gap_y_1()
                            .child(self.artist_links(
                                track.artist_credits(),
                                format!("queue-artist-entry-{id}"),
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .flex_shrink_0()
                    .child(if active {
                        format!(
                            "{} / {}",
                            duration_label(self.playback.position),
                            duration_label(track.duration)
                        )
                    } else {
                        duration_label(track.duration)
                    }),
            )
            .on_click(cx.listener(move |app, event: &ClickEvent, window, cx| {
                app.queue_select(
                    index,
                    event.modifiers().platform || event.modifiers().control,
                    event.modifiers().shift,
                    window,
                    cx,
                );
                if event.click_count() == 2 {
                    app.service.player.send(PlayerCommand::JumpTo(index));
                }
            }));
        let context_menu = move |menu: PopupMenu,
                                 window: &mut Window,
                                 cx: &mut Context<PopupMenu>| {
            let play = play_weak.clone();
            let next = next_weak.clone();
            let next_indices = move_next.clone();
            let remove = remove_weak.clone();
            let earlier = earlier_weak.clone();
            let later = later_weak.clone();
            let menu =
                super::context_menu::track_rating(menu.min_w(px(220.)), &menu_state, &menu_track)
                    .item(
                        menu_row(Glyph::Play, "Play from Here").on_click(move |_, _, cx| {
                            let _ = play.update(cx, |app, _| {
                                app.service.player.send(PlayerCommand::JumpTo(index))
                            });
                        }),
                    )
                    .item(
                        menu_row(Glyph::ListStart, "Move to Next")
                            .disabled(!upcoming)
                            .on_click(move |_, _, cx| {
                                let _ = next.update(cx, |app, _| {
                                    app.queue_edit_at(
                                        revision,
                                        QueueEdit::Move {
                                            indices: next_indices.clone(),
                                            before: next_index(&app.playback),
                                        },
                                    )
                                });
                            }),
                    )
                    .item(
                        menu_row(Glyph::ArrowUp, "Move Earlier")
                            .disabled(!upcoming)
                            .on_click(move |_, _, cx| {
                                let _ = earlier.update(cx, |app, _| {
                                    app.queue_edit_at(
                                        revision,
                                        QueueEdit::Move {
                                            indices: vec![index],
                                            before: index
                                                .saturating_sub(1)
                                                .max(next_index(&app.playback)),
                                        },
                                    )
                                });
                            }),
                    )
                    .item(
                        menu_row(Glyph::ArrowDown, "Move Later")
                            .disabled(!upcoming)
                            .on_click(move |_, _, cx| {
                                let _ = later.update(cx, |app, _| {
                                    app.queue_edit_at(
                                        revision,
                                        QueueEdit::Move {
                                            indices: vec![index],
                                            before: (index + 2).min(app.playback.queue.len()),
                                        },
                                    )
                                });
                            }),
                    );
            super::context_menu::track_extras(menu, &menu_state, &menu_track, window, cx)
                .separator()
                .item(
                    menu_row(Glyph::ListX, "Remove from Queue")
                        .disabled(!upcoming)
                        .on_click(move |_, _, cx| {
                            let _ = remove.update(cx, |app, _| {
                                app.queue_edit_at(revision, QueueEdit::Remove(vec![index]))
                            });
                        }),
                )
        };
        if upcoming {
            let revision = self.playback.queue_revision;
            let drag_weak = cx.weak_entity();
            row = row
                .cursor_grab()
                .on_drag(drag, move |drag, _, window, cx| {
                    cx.stop_propagation();
                    let _ = drag_weak.update(cx, |app, cx| {
                        app.queue_ui.selected = drag.ids.iter().copied().collect();
                        app.queue_ui.focus.focus(window, cx);
                        cx.notify();
                    });
                    cx.new(|_| QueueDragPreview(drag.title.clone()))
                })
                .child(
                    short_button(
                        SharedString::from(format!("queue-remove-{id}")),
                        Glyph::X,
                        "Remove from queue",
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.queue_edit(QueueEdit::Remove(remove.clone()));
                    })),
                )
                .drag_over::<QueueDrag>(move |s, drag, _, _| {
                    if drag.first_index < index {
                        s.border_b_2().border_color(colors.accent)
                    } else {
                        s.border_t_2().border_color(colors.accent)
                    }
                })
                .on_drop(cx.listener(move |app, drag: &QueueDrag, _, _| {
                    if drag.revision == revision {
                        app.drop_queue_tracks(drag, index + usize::from(drag.first_index < index));
                    }
                }));
        }
        row.context_menu(context_menu).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{QueueRow, queue_rows};
    use sonora::model::{PlaybackSnapshot, Track};
    use std::sync::Arc;
    fn playback() -> PlaybackSnapshot {
        PlaybackSnapshot {
            queue: Arc::new(
                [("same", "A"), ("same", "A"), ("b", "B"), ("a-again", "A")]
                    .iter()
                    .map(|(id, album)| {
                        Arc::new(Track {
                            id: (*id).into(),
                            title: (*id).into(),
                            album_id: (*album).into(),
                            ..Default::default()
                        })
                    })
                    .collect(),
            ),
            index: Some(0),
            ..Default::default()
        }
    }
    #[test]
    fn queue_sections_preserve_duplicate_occurrences_and_nonconsecutive_albums() {
        let rows = queue_rows(&playback(), false, "");
        assert_eq!(
            rows.iter()
                .filter_map(|row| if let QueueRow::Track(index) = row {
                    Some(*index)
                } else {
                    None
                })
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert!(rows.contains(&QueueRow::Album(1, true)));
        assert!(rows.contains(&QueueRow::Album(3, false)));
    }
    #[test]
    fn search_keeps_current_visible_and_reveals_matching_earlier_entries() {
        let mut snapshot = playback();
        snapshot.index = Some(2);
        let rows = queue_rows(&snapshot, false, "same");
        assert!(rows.contains(&QueueRow::Earlier(2)));
        assert!(rows.contains(&QueueRow::Track(0)));
        assert!(rows.contains(&QueueRow::Track(1)));
        assert!(rows.contains(&QueueRow::Track(2)));
        assert!(!rows.contains(&QueueRow::Track(3)));
        assert!(rows.contains(&QueueRow::Empty));
    }
}
