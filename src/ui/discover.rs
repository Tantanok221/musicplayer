use super::*;
use rand::seq::SliceRandom;

const TILE: f32 = 128.;

impl MusicApp {
    pub(super) fn request_discover(&mut self) {
        self.discover_request += 1;
        self.service.send(Command::Discover(self.discover_request));
    }

    fn shuffle_shelf(&self, albums: &[Arc<Album>]) {
        let mut ids = albums
            .iter()
            .map(|album| album.id.clone())
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
            .p_4()
            .flex()
            .flex_col()
            .gap_6();
        for (key, title, description, albums) in [
            (
                "old-rotation",
                "Old rotation",
                "Albums you played a lot, but not in the last six months.",
                &shelves.old_rotation,
            ),
            (
                "forgotten-favorites",
                "Forgotten favorites",
                "Favorite albums and albums with loved songs you haven't played in six months.",
                &shelves.forgotten_favorites,
            ),
        ] {
            if !albums.is_empty() {
                page = page.child(self.discover_shelf(key, title, description, albums, cx));
            }
        }
        if shelves.old_rotation.is_empty() && shelves.forgotten_favorites.is_empty() {
            page = page.child(
                div()
                    .id("discover-empty")
                    .test_support()
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
        key: &'static str,
        title: &'static str,
        description: &'static str,
        albums: &[Arc<Album>],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let shuffle = albums.to_vec();
        let mut tiles = div().flex().flex_wrap().gap_4();
        for album in albums {
            tiles = tiles.child(self.discover_tile(key, album, cx));
        }
        div()
            .id(SharedString::from(format!("discover-shelf-{key}")))
            .test_support()
            .role(Role::Group)
            .aria_label(title)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(15.))
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
                    .child(
                        Button::new(SharedString::from(format!("discover-shuffle-{key}")))
                            .ghost()
                            .small()
                            .icon(Glyph::Shuffle)
                            .label("Shuffle")
                            .tooltip(format!("Shuffle {title}"))
                            .on_click(cx.listener(move |app, _, _, _| app.shuffle_shelf(&shuffle))),
                    ),
            )
            .child(tiles)
            .into_any_element()
    }

    fn discover_tile(
        &mut self,
        key: &'static str,
        album: &Arc<Album>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let art = self.art(&album.cover_art, TILE, cx);
        let open = album.id.clone();
        let play = album.id.clone();
        div()
            .id(SharedString::from(format!(
                "discover-album-{key}-{}",
                album.id
            )))
            .test_support()
            .role(Role::Button)
            .aria_label(format!("Open album {} by {}", album.name, album.artist))
            .w(px(TILE))
            .flex_shrink_0()
            .cursor_pointer()
            .child(art)
            .child(
                div()
                    .mt_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(album.name.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "discover-play-{key}-{}",
                                album.id
                            )))
                            .test_support()
                            .role(Role::Button)
                            .aria_label(format!("Play {}", album.name))
                            .size(px(18.))
                            .flex_shrink_0()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.bg(colors.line))
                            .child(Icon::new(Glyph::Play).size(px(10.)))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.service.send(Command::PlayAlbum(play.clone()));
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .truncate()
                    .child(album.artist.clone()),
            )
            .on_click(cx.listener(move |app, _, window, cx| {
                app.open_album_in_explorer(open.clone(), window, cx)
            }))
            .context_menu(self.album_menu(album, cx))
            .into_any_element()
    }
}
