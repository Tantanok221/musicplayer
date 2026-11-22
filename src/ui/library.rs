use super::*;

/// How often a cover that is still on screen re-asks to move to the front of the download queue.
const ART_REPRIORITIZE: Duration = Duration::from_millis(750);

/// Rows of covers above and below the viewport downloaded ahead of scrolling.
const PREFETCH_ROWS: usize = 2;

/// Albums above and below the top of the album-and-tracks list downloaded ahead.
const PREFETCH_ALBUMS: std::ops::Range<isize> = -3..8;

pub(super) struct ArtSettle {
    first: Instant,
    last: Instant,
}

impl ArtSettle {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            first: now,
            last: now,
        }
    }

    fn observe(&mut self, now: Instant) -> bool {
        // Allow the scheduled wake a couple of frames of event-loop jitter.
        if now.duration_since(self.last) > ART_SETTLE * 3 {
            self.first = now;
        }
        self.last = now;
        now.duration_since(self.first) >= ART_SETTLE
    }
}

impl MusicApp {
    pub(super) fn request_query(&mut self, cx: &mut Context<Self>) {
        self.pending_explorer_album = None;
        self.query.favorite_filter = self.preferences.favorites.clone();
        if self.is_library() {
            self.query.ordering = Some(self.preferences.sorting.get(self.sort_scope()));
            self.explorer_query = self.query.clone();
        }
        self.request = self.service.query(self.query.clone());
        self.group_requested.clear();
        self.group_tracks.clear();
        self.selected_album = None;
        self.loading_more = false;
        cx.notify();
    }

    pub(super) fn is_library(&self) -> bool {
        self.artist_name.is_none() && matches!(self.workspace, Workspace::Explorer)
    }

    pub(super) fn main_view(&self) -> MainView {
        self.preferences.layout.explorer_view
    }

    pub(super) fn browser_visible(&self) -> bool {
        self.is_library() && self.preferences.layout.explorer_browser
    }

    pub(super) fn toggle_browser(&mut self, cx: &mut Context<Self>) {
        self.preferences.layout.explorer_browser = !self.preferences.layout.explorer_browser;
        self.save_preferences();
        cx.notify();
    }

    pub(super) fn cover_rows(&self) -> CoverRows {
        CoverRows::grouped(
            self.view.albums.len(),
            self.cover_columns,
            self.alphabet_jump,
            &self.cover_sections,
        )
    }

    pub(super) fn album_row_count(&self) -> usize {
        self.view.albums.len() + usize::from(self.alphabet_jump.is_some())
    }

    pub(super) fn jump_to_album(&mut self, index: usize) {
        self.alphabet_jump = Some(index);
        if self.main_view() == MainView::AlbumCovers {
            let rows = self.cover_rows();
            self.cover_list.reset(rows.row_count() + 1);
            self.cover_list.scroll_to(ListOffset {
                item_ix: rows.row_for_album(index),
                offset_in_item: px(0.),
            });
        } else {
            self.album_list.reset(self.album_row_count());
            self.album_list.scroll_to(ListOffset {
                item_ix: index,
                offset_in_item: px(0.),
            });
        }
    }

    pub(super) fn set_view(&mut self, view: MainView, cx: &mut Context<Self>) {
        self.alphabet_jump = None;
        self.preferences.layout.explorer_view = view;
        self.query.ordering = Some(self.preferences.sorting.get(self.sort_scope()));
        self.explorer_query.ordering = self.query.ordering.clone();
        self.request_query(cx);
        self.album_list.reset(self.album_row_count());
        self.cover_list
            .reset(self.cover_rows().row_count() + usize::from(self.alphabet_jump.is_some()));
        self.save_preferences();
        cx.notify();
    }

    pub(super) fn select_browser(
        &mut self,
        column: BrowserColumn,
        value: Option<String>,
        multiple: bool,
        cx: &mut Context<Self>,
    ) {
        self.query.selection.select(column, value, multiple);
        self.request_query(cx);
    }

    pub(super) fn select_track(&mut self, track: Arc<Track>, cx: &mut Context<Self>) {
        self.selected = Some(track);
        cx.notify();
    }

    pub(super) fn select_album(&mut self, album: Arc<Album>, cx: &mut Context<Self>) {
        let old = self
            .selected_album
            .as_ref()
            .and_then(|a| self.view.albums.iter().position(|v| v.id == a.id));
        let current = self.view.albums.iter().position(|a| a.id == album.id);
        self.selected_album = Some(album.clone());
        self.selected_missing = None;
        self.soulseek = None;
        self.request_group(&album.id);
        self.request_popularity(&album.artist_credits());
        for index in [old, current].into_iter().flatten() {
            let row = self.cover_rows().row_for_album(index);
            self.cover_list.splice(row..row + 1, 1);
        }
        cx.notify();
    }

    pub(super) fn request_group(&mut self, id: &str) {
        if !self.group_tracks.contains_key(id) && self.group_requested.insert(id.into()) {
            self.service.send(Command::GroupTracks(
                self.request,
                if self.artist_name.is_some() || self.workspace == Workspace::Discover {
                    LibraryQuery::default()
                } else {
                    self.query.clone()
                },
                id.into(),
            ));
        }
    }

    pub(super) fn active_filter_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let mut bar = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(colors.line)
            .text_size(px(10.))
            .text_color(colors.muted);
        if !self.query.performer.is_empty() {
            bar = bar.child(
                Button::new("clear-performer-filter")
                    .ghost()
                    .compact()
                    .label(format!("Artist: {} ×", self.query.performer))
                    .accessibility_label("Clear artist filter")
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.query.performer.clear();
                        app.request_query(cx);
                    })),
            );
        }
        if let Some(year) = self.query.year {
            bar = bar.child(
                Button::new("clear-year-filter")
                    .ghost()
                    .compact()
                    .label(format!("Year: {year} ×"))
                    .accessibility_label(format!("Clear year {year} filter"))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.query.year = None;
                        app.request_query(cx);
                    })),
            );
        }
        if !self.query.record_label.is_empty() {
            bar = bar.child(
                Button::new("clear-record-label-filter")
                    .ghost()
                    .compact()
                    .label(format!("Label: {} ×", self.query.record_label))
                    .accessibility_label(format!(
                        "Clear record label {} filter",
                        self.query.record_label
                    ))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.query.record_label.clear();
                        app.request_query(cx);
                    })),
            );
        }
        for (index, genre) in self.query.selection.genres.iter().enumerate() {
            let genre = genre.clone();
            bar = bar.child(
                Button::new(("clear-genre-filter", index))
                    .ghost()
                    .compact()
                    .label(format!("Genre: {genre} ×"))
                    .accessibility_label(format!("Clear genre {genre} filter"))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.query.selection.genres.retain(|value| value != &genre);
                        app.query.selection.artists.clear();
                        app.query.selection.albums.clear();
                        app.request_query(cx);
                    })),
            );
        }
        bar.into_any_element()
    }

    /// Queues covers just outside the viewport once scrolling pauses, so they are
    /// already downloaded when they scroll in. Covers on screen still go first.
    pub(super) fn prefetch_art(&mut self, list: &'static str, top: usize, covers: Vec<String>) {
        let now = Instant::now();
        match self.prefetch_anchor {
            Some((anchor, row, since)) if anchor == list && row == top => {
                if now.duration_since(since) < ART_SETTLE {
                    self.art_wake = true;
                    return;
                }
            }
            _ => {
                self.prefetch_anchor = Some((list, top, now));
                self.art_wake = true;
                return;
            }
        }
        for id in covers {
            if id.is_empty()
                || self.account_cache.artwork.contains_key(&id)
                || self.art_failures.contains(&id)
            {
                continue;
            }
            if self
                .art_prefetched
                .get(&id)
                .is_none_or(|sent| now.duration_since(*sent) >= ART_REPRIORITIZE)
            {
                self.art_prefetched.insert(id.clone(), now);
                self.service
                    .send(Command::Artwork(id, Some(Priority::Nearby)));
            }
        }
    }

    pub(super) fn art(&mut self, id: &str, side: f32, cx: &App) -> AnyElement {
        if let Some(path) = self.account_cache.artwork.get(id) {
            let path = match thumbnail::size_for(side).filter(|_| self.art_thumbs.contains(id)) {
                Some(size) => thumbnail::thumb_path(path, size),
                None => path.clone(),
            };
            return div()
                .id(SharedString::from(format!("cover-image-{id}")))
                .test_support()
                .role(Role::Image)
                .size(px(side))
                .flex_shrink_0()
                .overflow_hidden()
                .child(
                    img(path)
                        .image_cache(&self.image_cache)
                        .size_full()
                        .aspect_square()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element();
        }
        if !id.is_empty() && !self.art_failures.contains(id) {
            let now = cx.background_executor().now();
            // Covers already on disk show at once; only downloads wait for the tile to settle.
            let fresh = !self.art_seen.contains_key(id);
            let settled = self
                .art_seen
                .entry(id.to_string())
                .or_insert_with(|| ArtSettle::new(now))
                .observe(now);
            if fresh {
                self.service.send(Command::Artwork(id.into(), None));
            }
            if !settled {
                self.art_wake = true;
            } else if self
                .art_requested
                .get(id)
                .is_none_or(|sent| now.duration_since(*sent) >= ART_REPRIORITIZE)
            {
                self.art_requested.insert(id.into(), now);
                self.service
                    .send(Command::Artwork(id.into(), Some(Priority::Visible)));
            }
        }
        div()
            .size(px(side))
            .flex_shrink_0()
            .bg(ink(cx).panel)
            .border_1()
            .border_color(ink(cx).line)
            .flex()
            .items_center()
            .justify_center()
            .child(
                Icon::new(Glyph::Disc3)
                    .size(px((side * 0.32).max(16.)))
                    .text_color(ink(cx).muted),
            )
            .into_any_element()
    }

    /// What the Favorites page lists: hearts, a minimum rating, or both. Kept in
    /// preferences so it survives restarts and new builds.
    pub(super) fn favorites_filter_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.preferences.favorites.clone();
        let weak = cx.weak_entity();
        let update =
            move |weak: &WeakEntity<Self>, cx: &mut App, change: &dyn Fn(&mut FavoritesFilter)| {
                let _ = weak.update(cx, |app, cx| {
                    change(&mut app.preferences.favorites);
                    app.save_preferences();
                    app.request_query(cx);
                });
            };
        let update = std::rc::Rc::new(update);
        Button::new("favorites-filter")
            .ghost()
            .small()
            .icon(Glyph::SlidersHorizontal)
            .label(filter.summary())
            .text_color(ink(cx).muted)
            .tooltip("Choose what counts as a favorite")
            .dropdown_menu(move |menu, _, _| {
                let item =
                    |label: &str, checked: bool, change: Box<dyn Fn(&mut FavoritesFilter)>| {
                        let (weak, update) = (weak.clone(), update.clone());
                        PopupMenuItem::new(label.to_string())
                            .checked(checked)
                            .on_click(move |_, _, cx| update(&weak, cx, &*change))
                    };
                let mut menu = menu
                    .label("Include")
                    .item(item(
                        "Hearted",
                        filter.hearted || filter.min_rating == 0,
                        Box::new(|filter| filter.hearted = !filter.hearted),
                    ))
                    .separator()
                    .label("Minimum rating")
                    .item(item(
                        "Any rating",
                        filter.min_rating == 0,
                        Box::new(|filter| {
                            filter.min_rating = 0;
                            filter.hearted = true;
                        }),
                    ));
                for stars in 1..=5u8 {
                    let label = format!(
                        "{}{}",
                        "★".repeat(stars as usize),
                        if stars < 5 { " and up" } else { "" }
                    );
                    menu = menu.item(item(
                        &label,
                        filter.min_rating == stars,
                        Box::new(move |filter| filter.min_rating = stars),
                    ));
                }
                menu.separator()
                    .label("When both are on")
                    .item(item(
                        "Match either",
                        !filter.match_all,
                        Box::new(|filter| filter.match_all = false),
                    ))
                    .item(item(
                        "Match both",
                        filter.match_all,
                        Box::new(|filter| filter.match_all = true),
                    ))
            })
            .into_any_element()
    }

    pub(super) fn browser_column(
        &self,
        column: BrowserColumn,
        label: &'static str,
        values: Vec<Facet>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let selected = match column {
            BrowserColumn::Genre => &self.query.selection.genres,
            BrowserColumn::Artist => &self.query.selection.artists,
            BrowserColumn::Album => &self.query.selection.albums,
        };
        let empty = selected.is_empty();
        let selected = selected.clone();
        let entity = cx.entity();
        let count = values.len();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors.line)
            .child(panel_title(label, cx))
            .child(
                div()
                    .id(SharedString::from(format!("all-{label}")))
                    .test_support()
                    .role(Role::Button)
                    .aria_label(format!("All {label}"))
                    .aria_selected(empty)
                    .h(px(23.))
                    .px_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .when(empty, |s| s.bg(colors.selection).text_color(colors.accent))
                    .child(format!("All ({count})"))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.select_browser(column, None, false, cx)
                    })),
            )
            .child(
                uniform_list(
                    SharedString::from(format!("browser-{label}")),
                    count,
                    move |range, _, cx| {
                        entity.update(cx, |_, cx| {
                            range
                                .map(|i| {
                                    let item = values[i].clone();
                                    let active = selected.contains(&item.id);
                                    let id = item.id;
                                    div()
                                        .id(SharedString::from(format!(
                                            "browser-value-{label}-{id}"
                                        )))
                                        .test_support()
                                        .role(Role::Button)
                                        .aria_label(format!("Filter {label}: {}", item.label))
                                        .aria_selected(active)
                                        // List rows shrink to their text unless told to fill.
                                        .w_full()
                                        .h(px(22.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .cursor_pointer()
                                        .when(active, |s| {
                                            s.bg(colors.selection).text_color(colors.accent)
                                        })
                                        .hover(|s| s.bg(colors.line))
                                        .child(
                                            div().flex_1().min_w_0().truncate().child(item.label),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(colors.muted)
                                                .child(item.count.to_string()),
                                        )
                                        .on_click(cx.listener(
                                            move |app, event: &ClickEvent, _, cx| {
                                                app.select_browser(
                                                    column,
                                                    Some(id.clone()),
                                                    event.modifiers().platform
                                                        || event.modifiers().control,
                                                    cx,
                                                );
                                            },
                                        ))
                                })
                                .collect::<Vec<_>>()
                        })
                    },
                )
                .flex_1()
                .size_full(),
            )
            .into_any_element()
    }

    pub(super) fn column_browser(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .flex()
            .bg(ink(cx).panel)
            .text_size(px(11.))
            .child(self.browser_column(
                BrowserColumn::Genre,
                "Genre",
                self.view.browser.genres.clone(),
                cx,
            ))
            .child(self.browser_column(
                BrowserColumn::Artist,
                "Album Artist",
                self.view.browser.artists.clone(),
                cx,
            ))
            .child(self.browser_column(
                BrowserColumn::Album,
                "Album",
                self.view.browser.albums.clone(),
                cx,
            ))
            .into_any_element()
    }

    pub(super) fn grouped_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let top = self.album_list.logical_scroll_top().item_ix;
        let nearby = PREFETCH_ALBUMS
            .filter_map(|delta| top.checked_add_signed(delta))
            .filter_map(|index| self.view.albums.get(index))
            .map(|album| album.cover_art.clone())
            .collect();
        self.prefetch_art("albums", top, nearby);
        let entity = cx.entity();
        list(self.album_list.clone(), move |index, _, cx| {
            entity.update(cx, |app, cx| {
                if index == app.view.albums.len() {
                    // Leave room below the last album so a letter can align at the top.
                    div().h(px(app.library_height)).into_any_element()
                } else {
                    app.album_group(index, cx)
                }
            })
        })
        .size_full()
        .into_any_element()
    }

    pub(super) fn covers_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (columns, side) = cover_layout(self.library_width);
        if columns != self.cover_columns {
            self.cover_columns = columns;
            self.cover_list
                .reset(self.cover_rows().row_count() + usize::from(self.alphabet_jump.is_some()));
            if let Some(index) = self.alphabet_jump.or_else(|| {
                self.selected_album.as_ref().and_then(|selected| {
                    self.view
                        .albums
                        .iter()
                        .position(|album| album.id == selected.id)
                })
            }) {
                self.cover_list.scroll_to(ListOffset {
                    item_ix: self.cover_rows().row_for_album(index),
                    offset_in_item: px(0.),
                });
            }
        }
        let rows = self.cover_rows().row_count();
        let top = self.cover_list.logical_scroll_top().item_ix.min(rows);
        // Card height is the square cover plus its two text lines and padding.
        let visible = (self.library_height / (side + 56.)).ceil() as usize + 1;
        let below = top + visible;
        let nearby = (top.saturating_sub(PREFETCH_ROWS)..top)
            .chain(below..(below + PREFETCH_ROWS).min(rows))
            .flat_map(|row| self.cover_rows().albums_in_row(row))
            .filter_map(|index| self.view.albums.get(index))
            .map(|album| album.cover_art.clone())
            .collect();
        self.prefetch_art("covers", top, nearby);
        let entity = cx.entity();
        list(self.cover_list.clone(), move |row, _, cx| {
            entity.update(cx, |app, cx| {
                if row == app.cover_rows().row_count() {
                    return div().h(px(app.library_height)).into_any_element();
                }
                let mut cards = div().w_full().min_w_0().flex().gap_4().px_3().pt_3();
                for index in app.cover_rows().albums_in_row(row) {
                    let album = app.view.albums[index].clone();
                    let selected = app
                        .selected_album
                        .as_ref()
                        .is_some_and(|a| a.id == album.id);
                    let art = app.art(&album.cover_art, side, cx);
                    let open = album.clone();
                    cards = cards.child(
                        div()
                            .id(SharedString::from(format!("explorer-album-{}", album.id)))
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
                                    .child(album.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(ink(cx).muted)
                                    .truncate()
                                    .child(app.artist_links(
                                        album.artist_credits(),
                                        format!("card-artist-{}", album.id),
                                        cx,
                                    )),
                            )
                            .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                                app.select_album(open.clone(), cx);
                                if event.click_count() == 2 {
                                    // Favorites lists an album for its favorite songs; play those.
                                    app.service.send(if app.query.favorites {
                                        Command::PlayAlbumMatching(
                                            app.query.clone(),
                                            open.id.clone(),
                                        )
                                    } else {
                                        Command::PlayAlbum(open.id.clone())
                                    });
                                }
                            }))
                            .context_menu(app.album_menu(&album, cx)),
                    );
                }
                let first = app.cover_rows().albums_in_row(row).start;
                let mut item = div().w_full().min_w_0().flex().flex_col();
                if app.cover_sections.contains(&first)
                    && let Some(spec) = &app.query.ordering
                {
                    item = item.child(
                        div()
                            .mx_3()
                            .pt_3()
                            .pb_1()
                            .border_b_1()
                            .border_color(ink(cx).line)
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(spec.group_label(&app.view.albums[first])),
                    );
                }
                item = item.child(cards);
                if let Some(index) = app
                    .selected_album
                    .as_ref()
                    .and_then(|selected| {
                        app.view
                            .albums
                            .iter()
                            .position(|album| album.id == selected.id)
                    })
                    .filter(|index| app.cover_rows().row_for_album(*index) == row)
                {
                    item = item.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .mt_2()
                            .border_y_1()
                            .border_color(ink(cx).line)
                            .child(app.album_group(index, cx)),
                    );
                }
                item.into_any_element()
            })
        })
        .size_full()
        .into_any_element()
    }

    pub(super) fn flat_tracks(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let total = self.tracks.len() + usize::from(self.tracks.len() < self.track_total);
        uniform_list("all-tracks", total, move |range, _, cx| {
            entity.update(cx, |app, cx| {
                if range.end + 20 >= app.tracks.len()
                    && app.tracks.len() < app.track_total
                    && !app.loading_more
                {
                    app.loading_more = true;
                    app.service.send(Command::Page(
                        app.request,
                        app.view.revision,
                        app.query.clone(),
                        app.tracks.len(),
                    ));
                }
                range
                    .map(|index| {
                        if let Some(track) = app.tracks.get(index).cloned() {
                            let key = format!("flat-{}", track.id);
                            app.track_row(track, index + 1, key, None, None, cx)
                        } else {
                            div()
                                .h(px(27.))
                                .px_3()
                                .text_color(ink(cx).muted)
                                .child("Loading more tracks…")
                                .into_any_element()
                        }
                    })
                    .collect::<Vec<_>>()
            })
        })
        .track_scroll(&self.track_scroll)
        .size_full()
        .px_2()
        .into_any_element()
    }

    pub(super) fn empty(&self, message: &str, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("library-empty")
            .test_support()
            .role(Role::Status)
            .aria_label(message.to_string())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .text_color(ink(cx).muted)
            .child(Icon::new(Glyph::Music).size_7())
            .child(message.to_string())
            .child(
                Button::new("empty-clear")
                    .ghost()
                    .small()
                    .label(if self.account.is_none() {
                        "Connect to Navidrome"
                    } else {
                        "Clear search and filters"
                    })
                    .on_click(cx.listener(|app, _, window, cx| {
                        if app.account.is_none() {
                            app.settings = true;
                        } else {
                            app.query = LibraryQuery::default();
                            app.search_text.clear();
                            app.search
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            app.request_query(cx);
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub(super) fn main_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.settings {
            return self.connection_settings(cx);
        }
        if self.artist_name.is_some() {
            return self.artist_page(cx);
        }
        if self.workspace == Workspace::Queue {
            return self.queue_view(cx);
        }
        if self.workspace == Workspace::Playlists {
            return self.playlists_view(cx);
        }
        if self.workspace == Workspace::Discover {
            return self.discover_view(cx);
        }
        if self.view.track_count == 0 {
            return self.empty(
                if self.account.is_none() {
                    "Connect your Navidrome server to browse your collection."
                } else {
                    "No tracks match the current search or filters."
                },
                cx,
            );
        }
        match self.main_view() {
            MainView::AlbumsAndTracks => self.grouped_view(cx),
            MainView::AlbumCovers => self.covers_view(cx),
            MainView::Tracks => self.flat_tracks(cx),
        }
    }

    pub(super) fn center_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        if self.blank && !self.settings {
            return self.empty_tab_page(cx);
        }
        let mut center = div().size_full().min_h_0().min_w_0().flex().flex_col();
        if !self.settings {
            center = center.child(self.header(cx));
            if self.is_library()
                && (self.query.year.is_some()
                    || !self.query.selection.genres.is_empty()
                    || !self.query.record_label.is_empty()
                    || !self.query.performer.is_empty())
            {
                center = center.child(self.active_filter_bar(cx));
            }
        }
        if let Some(error) = self.error.clone().or_else(|| self.playback.error.clone()) {
            center =
                center.child(
                    div()
                        .w_full()
                        .px_2()
                        .py_2()
                        .flex_shrink_0()
                        .bg(if cx.theme().is_dark() {
                            rgb(0x423227)
                        } else {
                            rgb(0xf8e9d7)
                        })
                        .id("error-banner")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .flex()
                        .items_start()
                        .gap_2()
                        .text_size(px(11.))
                        .child(div().flex_1().min_w_0().child(error))
                        .child(Button::new("retry-error").small().label("Retry").on_click(
                            cx.listener(|app, _, _, cx| {
                                if let Some(mutation) = app.feedback.retryable().cloned() {
                                    app.mutate(mutation, cx);
                                } else if app.playback.error.is_some() {
                                    app.service.player.send(PlayerCommand::Retry);
                                } else {
                                    app.service.send(Command::Refresh);
                                }
                                app.error = None;
                                cx.notify();
                            }),
                        ))
                        .child(short_button("dismiss-error", Glyph::X, "Dismiss").on_click(
                            cx.listener(|app, _, _, cx| {
                                app.error = None;
                                app.playback.error = None;
                                app.feedback.dismiss();
                                app.service.player.send(PlayerCommand::ClearError);
                                cx.notify();
                            }),
                        )),
                );
        }
        if let Some(id) = self.pending_delete.clone() {
            center = center.child(
                div()
                    .p_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child("Delete this playlist from Navidrome?")
                    .child(
                        Button::new("confirm-delete")
                            .small()
                            .label("Delete")
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.pending_delete = None;
                                app.mutate(Mutation::DeletePlaylist(id.clone()), cx);
                            })),
                    )
                    .child(
                        Button::new("cancel-delete")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.pending_delete = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        if self.show_playlist_editor {
            center = center.child(
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
                            .label(if self.playlist_editor_rename {
                                "Rename"
                            } else {
                                "Create playlist"
                            })
                            .on_click(cx.listener(|app, _, _, cx| {
                                let name = app.playlist_name.read(cx).value().to_string();
                                if name.trim().is_empty() {
                                    return;
                                }
                                if app.playlist_editor_rename {
                                    if let Some(playlist) = app.playlists.current() {
                                        app.mutate(
                                            Mutation::RenamePlaylist(playlist.id.clone(), name),
                                            cx,
                                        );
                                    }
                                } else if let Some(album) = app.playlist_editor_album.clone() {
                                    app.service.send(Command::AlbumToPlaylist(
                                        album,
                                        crate::service::PlaylistTarget::New(name),
                                    ));
                                } else {
                                    let ids =
                                        app.playlist_editor_tracks.clone().unwrap_or_default();
                                    app.mutate(Mutation::CreatePlaylist(name, ids), cx);
                                }
                            })),
                    )
                    .child(
                        short_button("cancel-playlist-editor", Glyph::X, "Cancel").on_click(
                            cx.listener(|app, _, _, cx| {
                                app.show_playlist_editor = false;
                                app.playlist_editor_tracks = None;
                                app.playlist_editor_album = None;
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }
        let content = self.main_content(cx);
        let previous_width = self.library_width;
        let previous_height = self.library_height;
        let weak = cx.weak_entity();
        let content = div()
            .id("library-content")
            .test_support()
            .relative()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, window, _| {
                        let width = f32::from(bounds.size.width);
                        let height = f32::from(bounds.size.height);
                        if width > 0.
                            && height > 0.
                            && ((width - previous_width).abs() > 0.5
                                || (height - previous_height).abs() > 0.5)
                        {
                            window.on_next_frame(move |_, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    if (app.library_width - width).abs() > 0.5
                                        || (app.library_height - height).abs() > 0.5
                                    {
                                        app.library_width = width;
                                        app.library_height = height;
                                        app.album_list.remeasure();
                                        app.cover_list.remeasure();
                                        cx.notify();
                                    }
                                });
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(content);
        if self.browser_visible() && !self.settings {
            center = center.child(
                v_resizable(SharedString::from(format!(
                    "browser-and-library-{}",
                    self.layout_generation
                )))
                .on_resize(cx.listener(
                    |app, state: &Entity<gpui_kit::component::resizable::ResizableState>, _, cx| {
                        if let Some(size) = state.read(cx).sizes().first() {
                            app.preferences.layout.browser_height = f32::from(*size);
                            app.save_preferences();
                        }
                    },
                ))
                .child(
                    resizable_panel()
                        .size(px(self.preferences.layout.browser_height))
                        .size_range(px(90.)..px(400.))
                        .child(self.column_browser(cx)),
                )
                .child(
                    resizable_panel()
                        .size_range(px(160.)..px(2000.))
                        .child(content),
                ),
            );
        } else {
            center = center.child(div().flex_1().min_h_0().min_w_0().child(content));
        }
        center.bg(colors.background).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{ART_SETTLE, ArtSettle};
    use std::time::{Duration, Instant};

    #[test]
    fn art_waits_for_settle_even_with_frequent_frames() {
        let start = Instant::now();
        let mut settle = ArtSettle::new(start);
        assert!(!settle.observe(start));
        assert!(!settle.observe(start + Duration::from_millis(100)));
        assert!(settle.observe(start + ART_SETTLE));
    }

    #[test]
    fn art_accepts_its_own_late_wake_and_repeated_idle_frames() {
        let start = Instant::now();
        let mut settle = ArtSettle::new(start);
        for milliseconds in [130, 260, 390] {
            assert!(settle.observe(start + Duration::from_millis(milliseconds)));
        }
    }

    #[test]
    fn art_restarts_wait_after_leaving_the_screen() {
        let start = Instant::now();
        let mut settle = ArtSettle::new(start);
        assert!(settle.observe(start + ART_SETTLE));
        let returned = start + Duration::from_secs(2);
        assert!(!settle.observe(returned));
        assert!(!settle.observe(returned + Duration::from_millis(119)));
        assert!(settle.observe(returned + ART_SETTLE));
    }
}
