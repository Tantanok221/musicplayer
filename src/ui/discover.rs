use super::*;
use crate::discover::{Dismissal, Pick, Rediscover, Season};
use rand::seq::SliceRandom;

const LOVED_ROWS: usize = 4;
const SPOTLIGHT_SIDE: f32 = 200.;
const LOVED_SIDE: f32 = 64.;

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

    fn play_pick(&self, pick: &Pick) {
        if pick.loved.is_empty() || matches!(pick.reason, crate::discover::Reason::Rotation { .. })
        {
            self.service.send(Command::PlayAlbum(pick.album.id.clone()));
        } else {
            let ids = pick.loved.iter().map(|song| song.id.clone()).collect();
            self.service.send(Command::PlayIds(ids, 0));
        }
    }

    pub(super) fn discover_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let Some(shelves) = self.discover.clone() else {
            return div().into_any_element();
        };
        let empty = shelves.spotlight.is_empty()
            && shelves.lane.is_empty()
            && shelves.forgotten_favorites.is_empty();
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
                        "Music you already own and haven't heard in a while. \
                         Different memories every day.",
                    )),
            )
            .children(self.discover_spotlight(&shelves, cx))
            .when(!shelves.lane.is_empty(), |page| {
                page.child(self.memory_lane(&shelves, cx))
            })
            .when(!shelves.forgotten_favorites.is_empty(), |page| {
                page.child(self.loved_letters(&shelves, cx))
            })
            .child(if empty {
                div()
                    .id("discover-empty")
                    .test_support()
                    .px_1()
                    .text_color(colors.muted)
                    .child(
                        "Nothing to rediscover yet. Albums you used to play a lot and \
                         favorites you haven't played in six months will show up here.",
                    )
                    .into_any_element()
            } else {
                div()
                    .id("discover-closing")
                    .test_support()
                    .pt_4()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("That's all for today."),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .child("Tomorrow brings different memories."),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }

    fn discover_spotlight(
        &mut self,
        shelves: &Rediscover,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let colors = ink(cx);
        let pick = self.discover_visit.spotlight(&shelves.spotlight)?.clone();
        let album = pick.album.clone();
        let memory = pick.memory();
        let detail = pick.detail();
        let byline = match album.year {
            Some(year) => format!("{} \u{b7} {year}", album.artist),
            None => album.artist.clone(),
        };
        let play = pick.clone();
        let dismiss = album.id.clone();
        let loved = !pick.loved.is_empty()
            && !matches!(pick.reason, crate::discover::Reason::Rotation { .. });
        Some(
            div()
                .id("discover-spotlight")
                .test_support()
                .role(Role::Group)
                .aria_label("Today's rediscovery")
                .px_1()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_6()
                .child(
                    div()
                        .id("discover-spotlight-album")
                        .test_support()
                        .aria_label(format!("{} by {}", album.name, album.artist))
                        .flex_shrink_0()
                        .child(self.art(&album.cover_art, SPOTLIGHT_SIDE, cx)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(260.))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.muted)
                                .child("TODAY'S REDISCOVERY"),
                        )
                        .child(
                            div()
                                .text_size(px(20.))
                                .font_weight(FontWeight::BOLD)
                                .child(album.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.muted)
                                .child(byline),
                        )
                        .child(
                            div()
                                .id("discover-spotlight-memory")
                                .test_support()
                                .aria_label(memory.clone())
                                .pt_1()
                                .text_size(px(14.))
                                .child(memory),
                        )
                        .child(
                            div()
                                .id("discover-spotlight-detail")
                                .test_support()
                                .aria_label(detail.clone())
                                .text_size(px(12.))
                                .text_color(colors.muted)
                                .child(detail),
                        )
                        .child(
                            div()
                                .pt_2()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    Button::new("discover-spotlight-play")
                                        .primary()
                                        .icon(Glyph::Play)
                                        .label(if loved {
                                            "Play the songs you loved"
                                        } else {
                                            "Play it again"
                                        })
                                        .on_click(
                                            cx.listener(move |app, _, _, _| app.play_pick(&play)),
                                        ),
                                )
                                .when(shelves.spotlight.len() > 1, |row| {
                                    row.child(
                                        Button::new("discover-spotlight-next")
                                            .ghost()
                                            .label("Another memory")
                                            .on_click(cx.listener(|app, _, _, cx| {
                                                app.discover_visit.another();
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(
                                    Button::new("discover-spotlight-dismiss")
                                        .ghost()
                                        .label("Not today")
                                        .tooltip("Hide this album from Discover for 30 days")
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.dismiss_discover(
                                                dismiss.clone(),
                                                Dismissal::NotNow,
                                            );
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
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

    fn loved_letters(&mut self, shelves: &Rediscover, cx: &mut Context<Self>) -> AnyElement {
        let shelf = &shelves.forgotten_favorites;
        let key = "loved".to_string();
        let expanded = self.discover_visit.expanded(&key);
        let picks = if expanded {
            shelf.ranked.clone()
        } else {
            shelf.today.iter().take(LOVED_ROWS).cloned().collect()
        };
        let shuffle = shelf.ranked.clone();
        let total = shuffle.len();
        let mut actions = vec![
            Button::new("discover-shuffle-loved")
                .ghost()
                .small()
                .icon(Glyph::Shuffle)
                .label("Shuffle")
                .tooltip(format!(
                    "Shuffle all {total} albums in Songs you once loved"
                ))
                .on_click(cx.listener(move |app, _, _, _| app.shuffle_shelf(&shuffle)))
                .into_any_element(),
        ];
        if let Some(button) = self.see_all(key, total, LOVED_ROWS, cx) {
            actions.push(button.into_any_element());
        }
        let header = self.section_header(
            "Songs you once loved",
            "Albums you favorited, or loved songs on, that have been resting.",
            actions,
            cx,
        );
        let mut list = div()
            .id("discover-loved")
            .test_support()
            .role(Role::Group)
            .aria_label("Songs you once loved")
            .flex()
            .flex_col()
            .child(header);
        for pick in &picks {
            list = list.child(self.loved_letter(pick, cx));
            list = list.children(self.selected_panel(&[pick.album.id.as_str()], cx));
        }
        list.into_any_element()
    }

    fn loved_letter(&mut self, pick: &Pick, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let album = pick.album.clone();
        let id = album.id.clone();
        let selected = self
            .selected_album
            .as_ref()
            .is_some_and(|selected| selected.id == id);
        let memory = pick.memory();
        let detail = pick.detail();
        let open = album.clone();
        let play = pick.clone();
        let loved = !pick.loved.is_empty();
        div()
            .id(SharedString::from(format!("discover-loved-{id}")))
            .test_support()
            .role(Role::Button)
            .aria_label(format!("Select album {} by {}", album.name, album.artist))
            .aria_selected(selected)
            .px_1()
            .py_3()
            .flex()
            .items_center()
            .gap_4()
            .cursor_pointer()
            .border_b_1()
            .border_color(if selected { colors.accent } else { colors.line })
            .child(
                div()
                    .flex_shrink_0()
                    .child(self.art(&album.cover_art, LOVED_SIDE, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .text_size(px(12.))
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(album.name.clone()),
                            )
                            .child(div().text_color(colors.muted).child("\u{b7}"))
                            .child(div().text_color(colors.muted).truncate().child(
                                self.artist_links(
                                    album.artist_credits(),
                                    format!("discover-artist-loved-{id}"),
                                    cx,
                                ),
                            )),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("discover-loved-memory-{id}")))
                            .test_support()
                            .aria_label(memory.clone())
                            .text_size(px(12.))
                            .child(memory),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("discover-loved-detail-{id}")))
                            .test_support()
                            .aria_label(detail.clone())
                            .text_size(px(11.))
                            .text_color(colors.accent)
                            .truncate()
                            .child(detail),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!("discover-play-loved-{id}")))
                    .ghost()
                    .small()
                    .icon(Glyph::Play)
                    .label(if loved {
                        "Play the songs you loved"
                    } else {
                        "Play album"
                    })
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.play_pick(&play)
                    })),
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
