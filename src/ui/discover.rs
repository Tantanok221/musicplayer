use super::*;
use crate::discover::{Dismissal, Pick, Rediscover, Season};
use rand::seq::SliceRandom;

impl MusicApp {
    pub(super) fn request_discover(&mut self) {
        self.discover_request += 1;
        self.service.send(Command::Discover(self.discover_request));
    }

    fn dismiss_discover(&mut self, id: String, dismissal: Dismissal) {
        self.service.send(Command::HideDiscover(id, dismissal));
        self.request_discover();
    }

    fn shuffle_shelf(&self, picks: &[Pick]) {
        let mut ids = picks
            .iter()
            .map(|pick| pick.album.id.clone())
            .collect::<Vec<_>>();
        ids.shuffle(&mut rand::rng());
        let mut ids = ids.into_iter();
        if let Some(first) = ids.next() {
            self.service.send(Command::PlayAlbum(first));
        }
        for id in ids {
            self.service
                .send(Command::EnqueueAlbum(id, crate::playback::Placement::Last));
        }
    }

    fn see_all(
        &mut self,
        key: String,
        total: usize,
        shown: usize,
        cx: &mut Context<Self>,
    ) -> Option<Button> {
        let expanded = self.discover_visit.expanded(&key);
        (total > shown || expanded).then(|| {
            Button::new(SharedString::from(format!("discover-see-all-{key}")))
                .ghost()
                .small()
                .label(if expanded {
                    "Show less".to_string()
                } else {
                    format!("See all {total}")
                })
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.discover_visit.toggle(&key);
                    cx.notify();
                }))
        })
    }

    fn section_header(
        &self,
        title: &'static str,
        description: &'static str,
        actions: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        let colors = ink(cx);
        div()
            .px_1()
            .pb_2()
            .border_b_1()
            .border_color(colors.line)
            .flex()
            .items_end()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .child(description),
                    ),
            )
            .children(actions)
    }

    fn selected_panel(&mut self, ids: &[&str], cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = ink(cx);
        let album = self
            .selected_album
            .clone()
            .filter(|selected| ids.contains(&selected.id.as_str()))?;
        Some(
            div()
                .w_full()
                .min_w_0()
                .mt_2()
                .border_y_1()
                .border_color(colors.line)
                .child(self.album_panel(album, false, true, cx))
                .into_any_element(),
        )
    }

    pub(super) fn discover_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let Some(shelves) = self.discover.clone() else {
            return div().into_any_element();
        };
        div()
            .id("discover-page")
            .size_full()
            .overflow_y_scroll()
            .px_3()
            .pt_5()
            .pb_8()
            .flex()
            .flex_col()
            .gap_8()
            .child(
                div()
                    .px_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.accent)
                            .child(
                                chrono::Local::now()
                                    .format("%A, %B %-d")
                                    .to_string()
                                    .to_uppercase(),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(FontWeight::BOLD)
                            .child("Rediscover"),
                    )
                    .child(div().text_size(px(12.)).text_color(colors.muted).child(
                        "Albums you once had on repeat, grouped by when you last played them. \
                         New picks every day.",
                    ))
                    .children(self.history_line(&shelves.history, cx)),
            )
            .child(if shelves.lane.is_empty() {
                div()
                    .id("discover-empty")
                    .test_support()
                    .px_1()
                    .text_color(colors.muted)
                    .child(
                        "Nothing to rediscover yet. Albums you used to play a lot but \
                         haven't played in six months will show up here.",
                    )
                    .into_any_element()
            } else {
                self.memory_lane(&shelves, cx)
            })
            .into_any_element()
    }

    fn history_line(
        &self,
        status: &crate::discover::HistoryStatus,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use crate::discover::HistoryStatus;
        let colors = ink(cx);
        let text = match status {
            HistoryStatus::Off => {
                "Add your Last.fm username to reach back through every season you've scrobbled."
                    .to_string()
            }
            HistoryStatus::Loading { done: 0, total: 0 } => {
                "Reading your Last.fm history\u{2026}".to_string()
            }
            HistoryStatus::Loading { done, total } => {
                format!("Reading your Last.fm history\u{2026} {done} of {total} seasons")
            }
            HistoryStatus::Unavailable => {
                "Add a Last.fm API key in Settings to reach back through your history.".to_string()
            }
            HistoryStatus::Failed(message) => format!("Last.fm: {message}"),
            HistoryStatus::Ready => return None,
        };
        let off = matches!(status, HistoryStatus::Off | HistoryStatus::Unavailable);
        Some(
            div()
                .pt_1()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("discover-history")
                        .test_support()
                        .aria_label(text.clone())
                        .text_size(px(11.))
                        .text_color(colors.muted)
                        .child(text),
                )
                .when(off, |line| {
                    line.child(
                        Button::new("discover-add-lastfm")
                            .ghost()
                            .xsmall()
                            .label("Add Last.fm")
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.settings = true;
                                app.settings_page = 0;
                                cx.notify();
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    fn memory_lane(&mut self, shelves: &Rediscover, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let shuffle = shelves.old_rotation.ranked.clone();
        let total = shuffle.len();
        let header = self.section_header(
            "Memory lane",
            "Albums that were once on repeat, grouped by when you last had them on.",
            vec![
                Button::new("discover-shuffle-lane")
                    .ghost()
                    .small()
                    .icon(Glyph::Shuffle)
                    .label("Shuffle")
                    .tooltip(format!("Shuffle all {total} albums in Memory lane"))
                    .on_click(cx.listener(move |app, _, _, _| app.shuffle_shelf(&shuffle)))
                    .into_any_element(),
            ],
            cx,
        );
        let mut lane = div()
            .id("discover-lane")
            .test_support()
            .role(Role::Group)
            .aria_label("Memory lane")
            .flex()
            .flex_col()
            .gap_3()
            .child(header);
        for season in &shelves.lane {
            lane = lane.child(self.discover_season(season, colors, cx));
        }
        lane.into_any_element()
    }

    fn discover_season(
        &mut self,
        season: &Season,
        colors: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (columns, side) = cover_layout(self.library_width);
        let columns = columns.max(1);
        let key = season.key.clone();
        let expanded = self.discover_visit.expanded(&key);
        let picks = if expanded {
            season.shelf.ranked.clone()
        } else {
            season.shelf.today.iter().take(columns).cloned().collect()
        };
        let now = chrono::Utc::now();
        let mut rows = div().flex().flex_col();
        for chunk in picks.chunks(columns) {
            let mut row = div().w_full().min_w_0().flex().gap_4().px_1().pt_2();
            for pick in chunk {
                row = row.child(self.discover_tile(&key, pick, side, now, cx));
            }
            rows = rows.child(row);
            let ids = chunk
                .iter()
                .map(|pick| pick.album.id.as_str())
                .collect::<Vec<_>>();
            rows = rows.children(self.selected_panel(&ids, cx));
        }
        let see_all = self.see_all(key.clone(), season.shelf.ranked.len(), columns, cx);
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_1()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id(SharedString::from(format!("discover-season-{key}")))
                            .test_support()
                            .aria_label(season.label.clone())
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.muted)
                            .child(season.label.clone()),
                    )
                    .children(see_all),
            )
            .child(rows)
            .into_any_element()
    }

    fn discover_tile(
        &mut self,
        key: &str,
        pick: &Pick,
        side: f32,
        now: chrono::DateTime<chrono::Utc>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let album = pick.album.clone();
        let selected = self
            .selected_album
            .as_ref()
            .is_some_and(|selected| selected.id == album.id);
        let reason = pick.reason_text(now);
        let open = album.clone();
        div()
            .id(SharedString::from(format!(
                "discover-album-{key}-{}",
                album.id
            )))
            .test_support()
            .role(Role::Button)
            .aria_label(format!("Select album {} by {}", album.name, album.artist))
            .aria_selected(selected)
            .w(px(side))
            .flex_shrink_0()
            .cursor_pointer()
            .pb_2()
            .border_b_2()
            .border_color(if selected {
                colors.accent
            } else {
                colors.background
            })
            .child(self.art(&album.cover_art, side, cx))
            .child(
                div()
                    .mt_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(album.name.clone()),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .truncate()
                    .child(self.artist_links(
                        album.artist_credits(),
                        format!("discover-artist-{key}-{}", album.id),
                        cx,
                    )),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "discover-reason-{key}-{}",
                        album.id
                    )))
                    .test_support()
                    .aria_label(reason.clone())
                    .mt(px(2.))
                    .text_size(px(10.))
                    .text_color(colors.accent)
                    .truncate()
                    .child(reason),
            )
            .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                app.select_album(open.clone(), cx);
                if event.click_count() == 2 {
                    app.service.send(Command::PlayAlbum(open.id.clone()));
                }
            }))
            .context_menu(self.discover_menu(&album, cx))
            .into_any_element()
    }

    fn discover_menu(
        &self,
        album: &Album,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let app = cx.weak_entity();
        let base = self.album_menu(album, cx);
        let id = album.id.clone();
        move |menu, window, cx| {
            let menu = base(menu, window, cx).separator();
            let (not_now, never) = ((app.clone(), id.clone()), (app.clone(), id.clone()));
            menu.item(menu_row(Glyph::Clock, "Not now").on_click(move |_, _, cx| {
                let (app, id) = not_now.clone();
                let _ = app.update(cx, |app, cx| {
                    app.dismiss_discover(id, Dismissal::NotNow);
                    cx.notify();
                });
            }))
            .item(
                menu_row(Glyph::EyeOff, "Never suggest").on_click(move |_, _, cx| {
                    let (app, id) = never.clone();
                    let _ = app.update(cx, |app, cx| {
                        app.dismiss_discover(id, Dismissal::Never);
                        cx.notify();
                    });
                }),
            )
        }
    }
}
