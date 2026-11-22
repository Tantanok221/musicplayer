use super::*;

impl MusicApp {
    /// An empty tab always docks the navigator, since that is where pages are picked.
    pub(super) fn navigator_floats(&self) -> bool {
        self.preferences.layout.show_navigator
            && self.preferences.layout.navigator_auto_hide
            && !self.blank
    }

    /// Hide by pointer position rather than hover, so the navigator's own menus,
    /// which render outside its element tree, don't dismiss it.
    pub(super) fn peek_navigator(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.navigator_peek.is_some() {
            return;
        }
        self.navigator_peek = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let away = this
                    .update_in(cx, |app, window, cx| {
                        let away = !app.navigator_floats()
                            || f32::from(window.mouse_position().x) > width + 12.;
                        if away {
                            app.navigator_peek = None;
                            cx.notify();
                        }
                        away
                    })
                    .unwrap_or(true);
                if away {
                    break;
                }
            }
        }));
        cx.notify();
    }

    pub(super) fn navigation_row(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        icon: Glyph,
        active: bool,
        cx: &App,
    ) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
        // An empty tab shows no page, so nothing in the navigator is current.
        let active = active && !self.blank;
        let colors = ink(cx);
        let label: SharedString = label.into();
        div()
            .id(id)
            .test_support()
            .role(Role::Button)
            .aria_label(label.clone())
            .h(px(25.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .text_size(px(11.))
            .text_color(if active { colors.accent } else { colors.text })
            .when(active, |s| s.bg(colors.selection))
            .hover(|s| s.bg(colors.line))
            .child(Icon::new(icon).size_3())
            .child(div().flex_1().min_w_0().truncate().child(label))
    }

    pub(super) fn navigator(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let mut tree = div().flex().flex_col();
        tree = tree.child(
            self.navigation_row(
                "library-disclosure",
                "Library",
                if self.library_open {
                    Glyph::ChevronDown
                } else {
                    Glyph::ChevronRight
                },
                false,
                cx,
            )
            .on_click(cx.listener(|app, _, _, cx| {
                app.library_open = !app.library_open;
                cx.notify();
            })),
        );
        if self.library_open {
            tree = tree.child(
                div()
                    .pl_3()
                    .child(
                        self.navigation_row(
                            "nav-explorer",
                            "Music Explorer",
                            Glyph::Music,
                            self.workspace == Workspace::Explorer && !self.query.favorites,
                            cx,
                        )
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.switch_workspace(Workspace::Explorer, window, cx);
                            app.query = LibraryQuery::default();
                            app.search_text.clear();
                            app.search.update(cx, |s, cx| s.set_value("", window, cx));
                            app.request_query(cx);
                        })),
                    )
                    .child(
                        self.navigation_row(
                            "nav-favorites",
                            "Favorites",
                            Glyph::Heart,
                            self.workspace == Workspace::Explorer && self.query.favorites,
                            cx,
                        )
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.switch_workspace(Workspace::Explorer, window, cx);
                            app.query.favorites = true;
                            app.request_query(cx);
                        })),
                    )
                    .child(
                        self.navigation_row(
                            "nav-discover",
                            "Discover",
                            Glyph::Compass,
                            self.workspace == Workspace::Discover && self.artist_name.is_none(),
                            cx,
                        )
                        .test_support()
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.switch_workspace(Workspace::Discover, window, cx)
                        })),
                    )
                    .child(
                        self.navigation_row(
                            "nav-queue",
                            "Queue",
                            Glyph::ListMusic,
                            self.workspace == Workspace::Queue,
                            cx,
                        )
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.switch_workspace(Workspace::Queue, window, cx)
                        })),
                    )
                    .child(
                        self.navigation_row(
                            "nav-now-playing",
                            "Now Playing",
                            Glyph::Disc3,
                            self.workspace == Workspace::NowPlaying,
                            cx,
                        )
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.switch_workspace(Workspace::NowPlaying, window, cx)
                        })),
                    ),
            );
        }
        tree = tree.child(
            self.navigation_row(
                "playlist-disclosure",
                "Playlists",
                if self.playlists_open {
                    Glyph::ChevronDown
                } else {
                    Glyph::ChevronRight
                },
                false,
                cx,
            )
            .child(
                short_button("new-playlist", Glyph::Plus, "New playlist").on_click(cx.listener(
                    |app, _, _, cx| {
                        cx.stop_propagation();
                        app.show_playlist_editor = true;
                        app.playlist_editor_rename = false;
                        app.playlist_editor_album = None;
                        app.playlist_editor_tracks = None;
                        cx.notify();
                    },
                )),
            )
            .on_click(cx.listener(|app, _, _, cx| {
                app.playlists_open = !app.playlists_open;
                cx.notify();
            })),
        );
        if self.playlists_open {
            for playlist in self.playlists.summaries().to_vec() {
                let active = self.workspace == Workspace::Playlists
                    && self
                        .playlists
                        .current()
                        .is_some_and(|p| p.id == playlist.id);
                tree = tree.child(
                    div().pl_3().child(
                        self.navigation_row(
                            SharedString::from(format!("nav-playlist-{}", playlist.id)),
                            playlist.name.clone(),
                            Glyph::ListMusic,
                            active,
                            cx,
                        )
                        .when(
                            crate::playlists::is_smart(
                                &playlist,
                                self.account
                                    .as_ref()
                                    .map(|account| account.username.as_str()),
                            ),
                            |row| {
                                row.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "smart-playlist-{}",
                                            playlist.id
                                        )))
                                        .test_support()
                                        .aria_label("Smart playlist")
                                        .text_size(px(10.))
                                        .text_color(ink(cx).muted)
                                        .child("Smart"),
                                )
                            },
                        )
                        .test_support()
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.switch_workspace(Workspace::Playlists, window, cx);
                                let id = app.playlists.open(playlist.clone());
                                app.selected = None;
                                app.service.send(Command::Playlist(id));
                                cx.notify();
                            },
                        )),
                    ),
                );
            }
        }
        let mode = self.preferences.layout.navigator_mode;
        let values = self.view.navigator.values(mode).to_vec();
        let count = values.len();
        let all = match mode {
            NavigatorMode::Artist => self.query.performer.is_empty(),
            NavigatorMode::AlbumArtist => {
                self.query.selection.artists.is_empty() && self.artist_name.is_none()
            }
            NavigatorMode::Year => self.query.year.is_none(),
            NavigatorMode::Genre => {
                self.query.selection.genres.is_empty() && self.query.genre.is_empty()
            }
        };
        let entity = cx.entity();
        let facet_list = uniform_list("navigator-facets", count, move |range, _, cx| {
            entity.update(cx, |app, cx| {
                range
                    .map(|i| {
                        let item = values[i].clone();
                        let active = match mode {
                            NavigatorMode::Artist => app.query.performer == item.id,
                            NavigatorMode::AlbumArtist => {
                                app.query.selection.artists.contains(&item.id)
                                    || app.artist_name.as_ref() == Some(&item.id)
                            }
                            NavigatorMode::Year => {
                                app.query.year.map(|year| year.to_string()).as_ref()
                                    == Some(&item.id)
                            }
                            NavigatorMode::Genre => {
                                app.query.selection.genres.contains(&item.id)
                                    || app.query.genre == item.id
                            }
                        };
                        let art =
                            if matches!(mode, NavigatorMode::Artist | NavigatorMode::AlbumArtist) {
                                app.artist_portrait(
                                    &item.label,
                                    &item.detail,
                                    &item.artwork,
                                    34.,
                                    cx,
                                )
                            } else {
                                app.art(&item.artwork, 34., cx)
                            };
                        let id = item.id.clone();
                        let artist_id = item.detail.clone();
                        div()
                            .id(SharedString::from(format!(
                                "navigator-facet-{}-{}",
                                mode.label(),
                                item.id
                            )))
                            .test_support()
                            .role(Role::Button)
                            .aria_label(format!("Browse {}: {}", mode.label(), item.label))
                            .aria_selected(active)
                            // List rows shrink to their text unless told to fill.
                            .w_full()
                            .h(px(45.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .when(active, |s| s.bg(ink(cx).selection))
                            .hover(|s| s.bg(ink(cx).line))
                            .child(art)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .truncate()
                                            .text_color(if active {
                                                ink(cx).accent
                                            } else {
                                                ink(cx).text
                                            })
                                            .child(item.label),
                                    )
                                    .child(
                                        div().text_size(px(10.)).text_color(ink(cx).muted).child(
                                            format!(
                                                "{} album{}",
                                                item.count,
                                                if item.count == 1 { "" } else { "s" }
                                            ),
                                        ),
                                    ),
                            )
                            .on_click(cx.listener(move |app, event: &ClickEvent, window, cx| {
                                match mode {
                                    NavigatorMode::AlbumArtist => {
                                        if event.modifiers().platform || event.modifiers().control {
                                            if !app.is_library() {
                                                app.switch_workspace(
                                                    Workspace::Explorer,
                                                    window,
                                                    cx,
                                                );
                                            }
                                            app.select_browser(
                                                BrowserColumn::Artist,
                                                Some(id.clone()),
                                                true,
                                                cx,
                                            );
                                        } else {
                                            app.open_artist(
                                                id.clone(),
                                                artist_id.clone(),
                                                window,
                                                cx,
                                            );
                                        }
                                    }
                                    NavigatorMode::Artist => {
                                        app.switch_workspace(Workspace::Explorer, window, cx);
                                        app.query = LibraryQuery {
                                            performer: id.clone(),
                                            ..Default::default()
                                        };
                                        app.search_text.clear();
                                        app.search.update(cx, |input, cx| {
                                            input.set_value("", window, cx)
                                        });
                                        app.request_query(cx);
                                    }
                                    NavigatorMode::Year => {
                                        if let Ok(year) = id.parse() {
                                            app.open_explorer_filter(
                                                ExplorerFilter::Year(year),
                                                window,
                                                cx,
                                            );
                                        }
                                    }
                                    NavigatorMode::Genre => app.open_explorer_filter(
                                        ExplorerFilter::Genre(id.clone()),
                                        window,
                                        cx,
                                    ),
                                }
                            }))
                    })
                    .collect::<Vec<_>>()
            })
        })
        .track_scroll(&self.artist_scroll)
        .flex_1()
        .size_full();
        let weak = cx.weak_entity();
        let selector = Button::new("navigator-mode")
            .ghost()
            .small()
            .w_full()
            .h(px(26.))
            .rounded_none()
            .text_size(px(11.))
            .font_weight(FontWeight::NORMAL)
            .text_color(colors.muted)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(px(11.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(colors.muted)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().truncate().child(mode.label()))
                    .child(
                        div()
                            .flex_shrink_0()
                            .opacity(0.)
                            .group_hover("navigator-heading", |s| s.opacity(1.))
                            .child(Icon::new(Glyph::ChevronDown).size_3()),
                    ),
            )
            .accessibility_label(format!("Browse library by {}", mode.label()))
            .dropdown_menu(move |mut menu, _, _| {
                for choice in NavigatorMode::ALL {
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(choice.label())
                            .checked(choice == mode)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    app.preferences.layout.navigator_mode = choice;
                                    app.artist_scroll.scroll_to_item(0, ScrollStrategy::Top);
                                    app.save_preferences();
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });
        let upper = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.panel)
            .child(panel_title("Navigator", cx))
            .child(
                div()
                    .id("navigation-tree")
                    .overflow_y_scroll()
                    .flex_1()
                    .child(tree),
            );
        let lower = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.panel)
            .child(
                div()
                    .group("navigator-heading")
                    .h(px(27.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(colors.line)
                    .child(selector),
            )
            .child(
                div()
                    .id("all-navigator-values")
                    .role(Role::Button)
                    .aria_label(mode.all_label())
                    .h(px(35.))
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .when(all, |s| s.bg(colors.selection))
                    .child(mode.all_label())
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(colors.muted)
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.switch_workspace(Workspace::Explorer, window, cx);
                        match mode {
                            NavigatorMode::Artist => app.query.performer.clear(),
                            NavigatorMode::AlbumArtist => {
                                app.query
                                    .selection
                                    .select(BrowserColumn::Artist, None, false)
                            }
                            NavigatorMode::Year => app.query.year = None,
                            NavigatorMode::Genre => {
                                app.query.genre.clear();
                                app.query
                                    .selection
                                    .select(BrowserColumn::Genre, None, false);
                            }
                        }
                        app.request_query(cx);
                    })),
            )
            .child(facet_list);
        v_resizable(SharedString::from(format!(
            "left-panels-{}",
            self.layout_generation
        )))
        .on_resize(cx.listener(
            |app, state: &Entity<gpui_kit::component::resizable::ResizableState>, _, cx| {
                if let Some(size) = state.read(cx).sizes().first() {
                    app.preferences.layout.navigator_height = f32::from(*size);
                    app.save_preferences();
                }
            },
        ))
        .child(
            resizable_panel()
                .size(px(self.preferences.layout.navigator_height))
                .size_range(px(100.)..px(600.))
                .child(upper),
        )
        .child(
            resizable_panel()
                .size_range(px(100.)..px(2000.))
                .child(lower),
        )
        .into_any_element()
    }
}
