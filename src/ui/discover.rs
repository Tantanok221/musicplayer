use super::*;
use crate::discover::{Dismissal, Pick, Shelf};
use rand::seq::SliceRandom;

struct ShelfCopy {
    key: &'static str,
    title: &'static str,
    description: &'static str,
}

const OLD_ROTATION: ShelfCopy = ShelfCopy {
    key: "old-rotation",
    title: "Old rotation",
    description: "Albums you couldn't stop playing, then quietly set aside.",
};

const FORGOTTEN_FAVORITES: ShelfCopy = ShelfCopy {
    key: "forgotten-favorites",
    title: "Forgotten favorites",
    description: "Albums you loved, or loved songs on, that haven't come up in a while.",
};

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

    pub(super) fn discover_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let Some(shelves) = self.discover.clone() else {
            return div().into_any_element();
        };
        let mut page = div()
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
                                 New picks every day.",
                    )),
            );
        for (copy, shelf) in [
            (OLD_ROTATION, &shelves.old_rotation),
            (FORGOTTEN_FAVORITES, &shelves.forgotten_favorites),
        ] {
            if !shelf.is_empty() {
                page = page.child(self.discover_shelf(copy, shelf, cx));
            }
        }
        if shelves.old_rotation.is_empty() && shelves.forgotten_favorites.is_empty() {
            page = page.child(
                div()
                    .id("discover-empty")
                    .test_support()
                    .px_1()
                    .text_color(colors.muted)
                    .child(
                        "Nothing to rediscover yet. Albums you used to play a lot and \
                         favorites you haven't played in six months will show up here.",
                    ),
            );
        }
        page.into_any_element()
    }

    fn discover_shelf(
        &mut self,
        copy: ShelfCopy,
        shelf: &Shelf,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let (columns, side) = cover_layout(self.library_width);
        let columns = columns.max(1);
        let key = copy.key;
        let expanded = self.discover_expanded.contains(key);
        let picks = if expanded {
            shelf.ranked.clone()
        } else {
            shelf.today.iter().take(columns).cloned().collect()
        };
        let shuffle = shelf.ranked.clone();
        let now = chrono::Utc::now();
        let mut rows = div().flex().flex_col();
        for chunk in picks.chunks(columns) {
            let mut row = div().w_full().min_w_0().flex().gap_4().px_1().pt_3();
            for pick in chunk {
                row = row.child(self.discover_tile(key, pick, side, now, cx));
            }
            rows = rows.child(row);
            if let Some(album) = self
                .selected_album
                .clone()
                .filter(|selected| chunk.iter().any(|pick| pick.album.id == selected.id))
            {
                rows = rows.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .mt_2()
                        .border_y_1()
                        .border_color(colors.line)
                        .child(self.album_panel(album, false, true, cx)),
                );
            }
        }
        let total = shelf.ranked.len();
        div()
            .id(SharedString::from(format!("discover-shelf-{key}")))
            .test_support()
            .role(Role::Group)
            .aria_label(copy.title)
            .flex()
            .flex_col()
            .child(
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
                                    .child(copy.title),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(colors.muted)
                                    .child(copy.description),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("discover-shuffle-{key}")))
                            .ghost()
                            .small()
                            .icon(Glyph::Shuffle)
                            .label("Shuffle")
                            .tooltip(format!("Shuffle all {total} albums in {}", copy.title))
                            .on_click(cx.listener(move |app, _, _, _| app.shuffle_shelf(&shuffle))),
                    )
                    .when(total > columns || expanded, |header| {
                        header.child(
                            Button::new(SharedString::from(format!("discover-see-all-{key}")))
                                .ghost()
                                .small()
                                .label(if expanded {
                                    "Show less".to_string()
                                } else {
                                    format!("See all {total}")
                                })
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    if !app.discover_expanded.remove(key) {
                                        app.discover_expanded.insert(key);
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(rows)
            .into_any_element()
    }

    fn discover_tile(
        &mut self,
        key: &'static str,
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
