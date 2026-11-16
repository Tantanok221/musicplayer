use super::*;

impl MusicApp {
    pub(super) fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let label = self.artist_name.as_deref().unwrap_or(match self.workspace {
            Workspace::Queue => "Queue",
            Workspace::NowPlaying => "Now Playing",
            Workspace::Playlists => self
                .playlist
                .as_ref()
                .map_or("Playlists", |p| p.name.as_str()),
            _ => self.main_view().label(),
        });
        let weak = cx.weak_entity();
        let mut left = div().flex().items_center().gap_1();
        if self.is_library() {
            left = left.child(
                Button::new("main-view-menu")
                    .ghost()
                    .small()
                    .label(label.to_string())
                    .icon(Glyph::ChevronDown)
                    .dropdown_menu(move |mut menu, _, _| {
                        for view in [
                            MainView::AlbumsAndTracks,
                            MainView::AlbumCovers,
                            MainView::Tracks,
                        ] {
                            let weak = weak.clone();
                            menu = menu.item(PopupMenuItem::new(view.label()).on_click(
                                move |_, _, cx| {
                                    let _ = weak.update(cx, |app, cx| app.set_view(view, cx));
                                },
                            ));
                        }
                        menu
                    }),
            );
        } else {
            left = left.child(
                div()
                    .px_2()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label.to_string()),
            );
        }
        if self.is_library() && self.query.favorites {
            left = left.child(self.favorites_filter_menu(cx));
        }
        if self.workspace == Workspace::Queue
            && self.artist_name.is_none()
            && self.library_width >= 620.
        {
            left = left.child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(self.queue_summary()),
            );
        }
        let mut alphabet = div().flex().items_center().gap(px(1.));
        if self.is_library()
            && self.main_view() != MainView::Tracks
            && self.library_width >= 900.
            && self
                .query
                .ordering
                .as_ref()
                .is_some_and(|spec| spec.alphabetical_field().is_some())
        {
            for letter in "#ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars() {
                alphabet = alphabet.child(
                    div()
                        .id(SharedString::from(format!("jump-{letter}")))
                        .w(px(13.))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(9.))
                        .text_color(colors.muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(colors.accent).bg(colors.line))
                        .child(letter.to_string())
                        .on_click(cx.listener(move |app, _, _, cx| {
                            let found = app.view.albums.iter().position(|album| {
                                app.query
                                    .ordering
                                    .as_ref()
                                    .is_some_and(|spec| spec.album_letter(album) == letter)
                            });
                            if let Some(index) = found {
                                app.jump_to_album(index);
                                cx.notify();
                            }
                        })),
                );
            }
        }
        div()
            .h(px(30.))
            .px_1()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(colors.line)
            .child(left)
            .child(alphabet)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(self.is_library(), |s| {
                        s.child(
                            short_button(
                                "toggle-browser",
                                Glyph::Columns3,
                                "Toggle column browser",
                            )
                            .selected(self.browser_visible())
                            .on_click(cx.listener(|app, _, _, cx| app.toggle_browser(cx))),
                        )
                        .child(
                            short_button("clear-filter", Glyph::ListFilter, "Clear filters")
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.query.selection = Default::default();
                                    app.query.favorites = false;
                                    app.query.genre.clear();
                                    app.query.artist.clear();
                                    app.query.performer.clear();
                                    app.query.year = None;
                                    app.query.record_label.clear();
                                    app.request_query(cx);
                                })),
                        )
                        .child(self.sort_button(cx))
                    })
                    .when(
                        self.workspace == Workspace::Queue && self.artist_name.is_none(),
                        |s| s.child(self.queue_toolbar(cx)),
                    )
                    .children(self.downloads_button(cx))
                    .child(
                        short_button("refresh-library", Glyph::RefreshCw, "Refresh library")
                            .on_click(
                                cx.listener(|app, _, _, _| app.service.send(Command::Refresh)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Server reachability at the start of the status bar, in the same quiet dot style
    /// for every state; offline adds amber text and retries on click.
    pub(super) fn connection_indicator(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dot = |color: u32| {
            div()
                .size(px(7.))
                .rounded_full()
                .flex_shrink_0()
                .bg(rgb(color))
        };
        let quiet = |id: &'static str, color: u32, label: &'static str, hint: &'static str| {
            div()
                .id(id)
                .test_support()
                .role(Role::Status)
                .aria_label(label)
                .mr_2()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .child(dot(color))
                .child(label)
                .tooltip(move |window, cx| tooltip::Tooltip::new(hint).build(window, cx))
                .into_any_element()
        };
        match &self.connection {
            Connection::None => None,
            Connection::Online => Some(quiet(
                "server-online",
                0x3fa45b,
                "Online",
                "Connected to your Navidrome server",
            )),
            Connection::Connecting => Some(quiet(
                "server-connecting",
                0xd99a2b,
                "Connecting…",
                "Contacting your Navidrome server",
            )),
            Connection::Offline(reason) => {
                let reason: SharedString =
                    format!("{reason}\n\nRetrying automatically. Click to retry now.").into();
                Some(
                    div()
                        .id("server-offline")
                        .test_support()
                        .mr_2()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .text_color(ink(cx).accent)
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .hover(|label| label.underline())
                        .role(Role::Alert)
                        .aria_label("Server offline. Click to retry now.")
                        .child(dot(0xd99a2b))
                        .child("Server offline · Retry")
                        .tooltip(move |window, cx| {
                            tooltip::Tooltip::new(reason.clone()).build(window, cx)
                        })
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.connection = Connection::Connecting;
                            app.service.send(Command::CheckConnection(true));
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            }
        }
    }

    pub(super) fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let queue_time = self.playback.queue.iter().map(|t| t.duration).sum::<f64>();
        let library = format!(
            "{} tracks · {} albums",
            self.view.track_count,
            self.view.albums.len()
        );
        // Many releases simply have no cover on the Cover Art Archive; that is not a failure.
        let failed_covers = self
            .art_failures
            .iter()
            .filter(|id| !id.starts_with("remote:https://coverartarchive.org/"))
            .count();
        let artwork_status = if failed_covers == 0 {
            String::new()
        } else {
            format!(" · {failed_covers} covers failed; Refresh to retry")
        };
        div()
            .id("library-status")
            .test_support()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .bg(colors.panel)
            .role(Role::Status)
            .aria_label(format!(
                "{library}; {}{artwork_status}. Queue: {}",
                self.status,
                self.playback.queue.len()
            ))
            .h(px(21.))
            .px_2()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(colors.line)
            .text_size(px(10.))
            .text_color(colors.muted)
            .children(self.connection_indicator(cx))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .child(format!("{library}   |   {}{artwork_status}", self.status)),
            )
            .child(div().ml_3().flex_shrink_0().child(format!(
                "Queued: {} · {}",
                self.playback.queue.len(),
                duration_label(queue_time)
            )))
            .child(
                short_button(
                    "status-right-panel",
                    Glyph::PanelRight,
                    "Show / hide right panel",
                )
                .on_click(cx.listener(|app, _, _, cx| {
                    app.preferences.layout.show_information =
                        !app.preferences.layout.show_information;
                    app.save_preferences();
                    cx.notify();
                })),
            )
            .into_any_element()
    }
}
