use super::*;

impl MusicApp {
    pub(super) fn location(&self) -> Page {
        Page {
            workspace: self.workspace,
            artist: self
                .artist_name
                .clone()
                .map(|name| (name, self.artist_id.clone())),
            query: if self.workspace == Workspace::Explorer {
                self.query.clone()
            } else {
                LibraryQuery::default()
            },
            playlist: if self.workspace == Workspace::Playlists {
                self.playlists
                    .current()
                    .map(|playlist| (playlist.id.clone(), playlist.name.clone()))
            } else {
                None
            },
            search: match self.workspace {
                Workspace::Queue => self.queue_search.clone(),
                Workspace::Playlists => self.playlists.search().to_owned(),
                _ => String::new(),
            },
        }
    }

    /// Record the current page before navigating away from it.
    pub(super) fn remember_location(&mut self) {
        if self.restoring_location {
            return;
        }
        // An empty tab is remembered too, so Back can return to it.
        let location = (!self.blank).then(|| self.location());
        self.history.remember(location);
    }

    pub(super) fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = (!self.blank).then(|| self.location());
        if let Some(location) = self.history.back(current) {
            self.open_history_page(location, window, cx);
        }
    }

    pub(super) fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = (!self.blank).then(|| self.location());
        if let Some(location) = self.history.forward(current) {
            self.open_history_page(location, window, cx);
        }
    }

    pub(super) fn open_history_page(
        &mut self,
        location: Option<Page>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match location {
            Some(location) => {
                self.blank = false;
                self.restore_location(location, window, cx);
            }
            None => self.empty_active_tab(window, cx),
        }
    }

    pub(super) fn restore_location(
        &mut self,
        location: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restoring_location = true;
        if let Some((name, id)) = location.artist {
            self.open_artist(name, id, window, cx);
        } else {
            // switch_workspace puts these searches back into the search box.
            match location.workspace {
                Workspace::Queue => self.queue_search = location.search.clone(),
                Workspace::Playlists => self.playlists.set_search(location.search.clone()),
                _ => {}
            }
            self.switch_workspace(location.workspace, window, cx);
            match location.workspace {
                Workspace::Explorer => {
                    self.search_text = location.query.text.clone();
                    self.search.update(cx, |input, cx| {
                        input.set_value(self.search_text.clone(), window, cx)
                    });
                    self.explorer_query = location.query.clone();
                    self.query = location.query;
                    self.request_query(cx);
                }
                Workspace::Playlists => {
                    let current = self.playlists.current().map(|p| p.id.clone());
                    if current.as_ref() != location.playlist.as_ref().map(|(id, _)| id) {
                        self.selected = None;
                        // The full list arrives with the loaded playlist.
                        self.playlists.close();
                        if let Some((id, name)) = location.playlist {
                            self.service.send(Command::Playlist(id.clone()));
                            self.playlists.open(Playlist {
                                id,
                                name,
                                ..Default::default()
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        self.restoring_location = false;
        cx.notify();
    }

    pub(super) fn switch_workspace(
        &mut self,
        workspace: Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if workspace != self.workspace || self.artist_name.is_some() || self.blank {
            self.remember_location();
        }
        self.blank = false;
        if self.workspace == Workspace::Explorer {
            self.explorer_query = self.query.clone();
        }
        self.workspace = workspace;
        self.artist_name = None;
        self.artist_albums = None;
        self.lyrics_follow = None;
        self.settings = false;
        self.search_text = match workspace {
            Workspace::Explorer => {
                self.query = self.explorer_query.clone();
                self.query.text.clone()
            }
            Workspace::Playlists => self.playlists.search().to_owned(),
            Workspace::Queue => self.queue_search.clone(),
            Workspace::NowPlaying | Workspace::Discover => String::new(),
        };
        self.search.update(cx, |input, cx| {
            input.set_value(self.search_text.clone(), window, cx)
        });
        if self.is_library() {
            self.request_query(cx);
        }
        if workspace == Workspace::Queue {
            self.queue_ui.focus.focus(window, cx);
        }
        if workspace == Workspace::Discover {
            self.request_discover();
        }
        cx.notify();
    }

    pub(super) fn play_track(&self, track: &Track) {
        self.service.send(Command::PlayTrack(track.id.clone()));
    }

    pub(super) fn open_artist(
        &mut self,
        name: String,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if name.trim().is_empty() {
            return;
        }
        self.remember_location();
        self.blank = false;
        let origin = if self.artist_name.is_some() {
            self.artist_origin
        } else {
            self.workspace
        };
        self.switch_workspace(Workspace::Explorer, window, cx);
        self.artist_origin = origin;
        self.artist_name = Some(name.clone());
        self.artist_id = id.clone();
        self.artist_request += 1;
        self.artist_data = None;
        self.biography_expanded = false;
        self.popular_expanded = false;
        self.most_played_expanded = false;
        self.artist_albums = None;
        // A new artist starts at the top with its rows measured afresh.
        self.artist_rows = Rc::default();
        self.artist_list.reset(0);
        self.selected_album = None;
        self.selected_missing = None;
        self.missing_expanded = false;
        self.soulseek = None;
        self.group_tracks.clear();
        self.group_requested.clear();
        self.search_text.clear();
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.service
            .send(Command::Artist(self.artist_request, name, id));
        cx.notify();
    }

    pub(super) fn open_explorer_filter(
        &mut self,
        filter: ExplorerFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remember_location();
        self.switch_workspace(Workspace::Explorer, window, cx);
        self.query = filter.query();
        self.explorer_query = self.query.clone();
        self.search_text.clear();
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.request_query(cx);
        self.focus.focus(window, cx);
    }

    pub(super) fn metadata_link(
        &self,
        filter: ExplorerFilter,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = match &filter {
            ExplorerFilter::Genre(genre) => genre.clone(),
            ExplorerFilter::Year(year) => year.to_string(),
            ExplorerFilter::RecordLabel(label) => label.clone(),
        };
        let kind = match &filter {
            ExplorerFilter::Genre(_) => "genre",
            ExplorerFilter::Year(_) => "year",
            ExplorerFilter::RecordLabel(_) => "record label",
        };
        div()
            .id(SharedString::from(key))
            .test_support()
            .role(Role::Button)
            .aria_label(format!("Explore {kind}: {label}"))
            .cursor_pointer()
            .hover(|link| link.text_color(ink(cx).accent))
            .child(label)
            .on_click(cx.listener(move |app, _, window, cx| {
                cx.stop_propagation();
                app.open_explorer_filter(filter.clone(), window, cx);
            }))
            .into_any_element()
    }

    pub(super) fn genre_links(
        &self,
        genres: &str,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut links = div()
            .flex()
            .flex_wrap()
            .min_w_0()
            .items_center()
            .gap_1()
            .when(key.starts_with("playlist-link-"), |links| {
                links.flex_nowrap().overflow_hidden()
            });
        for (index, genre) in genres
            .split(" · ")
            .map(str::trim)
            .filter(|genre| !genre.is_empty())
            .enumerate()
        {
            if index > 0 {
                links = links.child("·");
            }
            links = links.child(self.metadata_link(
                ExplorerFilter::Genre(genre.into()),
                format!("{key}-{index}"),
                cx,
            ));
        }
        links.into_any_element()
    }

    pub(super) fn year_link(
        &self,
        year: Option<i32>,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        year.map_or_else(
            || div().into_any_element(),
            |year| self.metadata_link(ExplorerFilter::Year(year), key, cx),
        )
    }

    pub(super) fn artist_link(
        &self,
        name: String,
        id: String,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.artist_links(artist_credits(&name, &[], &id), key, cx)
    }

    pub(super) fn artist_links(
        &self,
        credits: Vec<NamedId>,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut links = div()
            .flex()
            .flex_wrap()
            .items_center()
            .min_w_0()
            .gap(px(3.));
        // Fixed-height rows must stay on one line; wrapped credits would spill
        // into the next row.
        if ["footer", "queue-artist", "card-artist", "playlist-link-"]
            .iter()
            .any(|prefix| key.starts_with(prefix))
        {
            links = links.flex_nowrap().overflow_hidden();
        }
        for (index, credit) in credits.into_iter().enumerate() {
            if index > 0 {
                links = links.child(div().text_color(ink(cx).muted).child("·"));
            }
            let name = credit.name;
            let id = credit.id;
            links = links.child(
                div()
                    .id(SharedString::from(format!("{key}-{index}")))
                    .test_support()
                    .role(Role::Button)
                    .aria_label(format!("Open artist {name}"))
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .hover(|link| link.text_color(ink(cx).accent))
                    .child(name.clone())
                    .on_click(cx.listener(move |app, _, window, cx| {
                        cx.stop_propagation();
                        app.open_artist(name.clone(), id.clone(), window, cx);
                    })),
            );
        }
        links.into_any_element()
    }
}
