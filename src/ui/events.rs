use super::*;

impl MusicApp {
    pub(super) fn event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::NativeAvailable(available) => self.smart.available = available,
            Event::SmartRules(id, rules) => self.smart_rules_loaded(id, rules, window, cx),
            Event::SmartPlaylistSaved(id) => self.smart_saved(id, window, cx),
            Event::NativePasswordRequired => self.smart_password_required(window, cx),
            Event::Ready(account, prefs) => {
                self.album_deletion = None;
                if self
                    .account_cache
                    .reset_for_account(self.account.as_ref(), account.as_ref())
                {
                    self.close_playlist_creation(window, cx);
                    self.playlist_rename = None;
                    self.smart = Default::default();
                    self.playlists = Default::default();
                    self.artist_request += 1;
                    self.artist_name = None;
                    self.artist_albums = None;
                }
                let show_settings = self.settings || account.is_none();
                self.account = account.clone();
                // The first frame drew panels at default sizes before these arrived.
                if self.preferences.layout != prefs.layout {
                    self.layout_generation += 1;
                }
                self.preferences = prefs;
                self.preferences.tabs = std::mem::take(&mut self.preferences.tabs).normalized();
                self.prefs_loaded = true;
                self.histories.clear();
                self.history = History::default();
                self.feedback.release_albums();
                self.query.ordering = Some(self.preferences.sorting.get(self.sort_scope()));
                self.explorer_query.ordering = self.query.ordering.clone();
                self.request_query(cx);
                self.apply_theme(window, cx);
                self.volume.update(cx, |slider, cx| {
                    slider.set_value(self.preferences.volume, window, cx)
                });
                let folder = self.preferences.metadata.library_folder.clone();
                self.metadata_folder
                    .update(cx, |input, cx| input.set_value(folder, window, cx));
                let key = self.preferences.metadata.acoustid_key.clone();
                self.acoustid_key
                    .update(cx, |input, cx| input.set_value(key, window, cx));
                let slskd = self.preferences.downloads.url.clone();
                self.slskd_url
                    .update(cx, |input, cx| input.set_value(slskd, window, cx));
                let lastfm = self.preferences.lastfm_user.clone();
                self.lastfm_user
                    .update(cx, |input, cx| input.set_value(lastfm, window, cx));
                self.discographies.clear();
                if let Some(account) = account {
                    self.server_url
                        .update(cx, |input, cx| input.set_value(account.url, window, cx));
                    self.username.update(cx, |input, cx| {
                        input.set_value(account.username, window, cx)
                    });
                }
                // Reopen the saved tabs on the page the active one was showing.
                self.show_active_tab(window, cx);
                // Restoring tab content must not dismiss sign-in or an open form.
                self.settings = show_settings;
            }
            Event::SignInRequired(message) => {
                self.settings = true;
                self.status = message;
                self.error = None;
            }
            Event::Connected(account, label) => {
                self.account = Some(account);
                if self.settings {
                    self.focus.focus(window, cx);
                }
                self.settings = false;
                self.status = label;
                self.error = None;
                self.art_requested.clear();
                self.art_seen.clear();
                self.art_prefetched.clear();
                self.art_thumbs.clear();
                self.art_failures.clear();
                self.lyrics.clear();
                self.lyrics_requested.clear();
                self.lyrics_follow = None;
                self.highlight_album = None;
                self.highlight_requested = None;
                self.password
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
            Event::View(request, view) => {
                if request == self.request {
                    if self.workspace == Workspace::Discover && self.artist_name.is_none() {
                        self.request_discover();
                    }
                    if let Some(name) = &self.artist_name {
                        self.service.send(Command::Artist(
                            self.artist_request,
                            name.clone(),
                            self.artist_id.clone(),
                        ));
                    }

                    let ids = view
                        .albums
                        .iter()
                        .map(|a| a.id.as_str())
                        .collect::<Vec<_>>();
                    let old = self
                        .view
                        .albums
                        .iter()
                        .map(|a| a.id.as_str())
                        .collect::<Vec<_>>();
                    let sections = self
                        .query
                        .ordering
                        .as_ref()
                        .filter(|s| s.grouping != Grouping::None)
                        .map(|spec| {
                            view.albums
                                .iter()
                                .enumerate()
                                .filter_map(|(i, a)| {
                                    (i == 0
                                        || spec.group_label(&view.albums[i - 1])
                                            != spec.group_label(a))
                                    .then_some(i)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let reset = ids != old || self.cover_sections != sections;
                    self.cover_sections = sections;
                    self.tracks = view.tracks.clone();
                    self.track_total = view.track_count;
                    self.loading_more = false;
                    self.view = view;
                    if reset {
                        self.alphabet_jump = None;
                        self.album_list.reset(self.album_row_count());
                        self.cover_list.reset(
                            self.cover_rows().row_count()
                                + usize::from(self.alphabet_jump.is_some()),
                        );
                    }
                    self.group_requested.clear();
                    self.group_tracks.clear();
                    if self.selected_album.as_ref().is_some_and(|selected| {
                        !self.view.albums.iter().any(|a| a.id == selected.id)
                    }) {
                        self.selected_album = None;
                    }
                    if let Some(id) = self.pending_explorer_album.take()
                        && let Some(album) = self
                            .view
                            .albums
                            .iter()
                            .find(|album| album.id == id)
                            .cloned()
                    {
                        self.select_album(album, cx);
                    }
                }
            }
            Event::Page(request, revision, offset, tracks) => {
                if request == self.request
                    && revision == self.view.revision
                    && offset == self.tracks.len()
                {
                    self.tracks.extend(tracks);
                    self.loading_more = false;
                }
            }
            Event::GroupTracks(request, id, tracks) => {
                if request == self.request {
                    self.group_tracks.insert(id.clone(), tracks);
                    if let Some(index) = self.view.albums.iter().position(|a| a.id == id) {
                        self.album_list.splice(index..index + 1, 1);
                        let row = self.cover_rows().row_for_album(index);
                        self.cover_list.splice(row..row + 1, 1);
                    }
                }
            }
            Event::Discover(request, shelves) => {
                if request == self.discover_request {
                    self.discover = Some(shelves);
                }
            }
            Event::Artist(request, data) => {
                if self.artist_name.is_some() && self.artist_request == request {
                    self.artist_albums = Some(data.albums.clone());
                    self.artist_data = Some(data);
                }
            }
            Event::ArtistProfile(key, profile) => {
                self.account_cache.artist_profiles.insert(key, profile);
            }
            Event::TopSongs(key, top) => {
                self.account_cache.top_songs.insert(key, top);
            }
            Event::Discography(key, view) => {
                self.discographies.insert(key, view);
            }
            Event::SoulseekSearch(release, update) => {
                if self.selected_missing.as_ref() == Some(&release) {
                    self.soulseek = Some((release, update));
                    self.soulseek_open = None;
                }
            }
            Event::ReleaseDetails(group, details) => {
                self.release_details.insert(group, details);
            }
            Event::Downloads(jobs) => {
                if self.download_start.as_ref().is_some_and(|start| {
                    start.failure.is_none() && jobs.iter().any(|job| job.release == start.release)
                }) {
                    self.download_start = None;
                }
                self.downloads = jobs;
            }
            Event::DownloadNotStarted(release, reason) => {
                if let Some(start) = self
                    .download_start
                    .as_mut()
                    .filter(|start| start.release == release)
                {
                    start.failure = Some(reason);
                }
            }
            Event::SlskdStatus(status) => self.slskd_status = Some(status),
            Event::TagsWritten(report) => self.tags_written(report, window, cx),
            Event::TagProgress(progress) => self.tag_progress(progress),
            Event::Album(id, tracks) if self.tag_editor_album.as_ref() == Some(&id) => {
                self.tag_editor_album = None;
                let tracks = tracks.iter().map(|track| (**track).clone()).collect();
                self.open_tag_editor(tracks, window, cx);
            }
            Event::Album(id, tracks) => {
                if self.highlight_requested.as_ref() == Some(&id) {
                    self.highlight_album = Some((id, tracks));
                }
            }
            Event::AlbumDetails(album) => {
                if self.playlists.update_album(album.clone()) {
                    self.request_popularity(&album.artist_credits());
                }
            }
            Event::Playlists(playlists) => self.playlists.summaries_loaded(playlists),
            Event::Playlist(playlist) => self.playlists.detail_loaded(playlist),
            Event::TrackUpdated(track) => self.apply_track_update(track),
            Event::ReleaseCandidates(query, result) => self.releases_found(query, result),
            Event::ReleaseLoaded(id, result) => self.release_loaded(id, result),
            Event::SoundIdentified(id, result) => self.sound_identified(id, result),
            Event::AlbumUpdated(album) => self.apply_album_update(album),
            Event::Artwork(id, path, thumbs) => {
                self.art_failures.remove(&id);
                if thumbs {
                    self.art_thumbs.insert(id.clone());
                }
                self.account_cache.artwork.insert(id, path);
            }
            Event::Backdrop(id, backdrop) => {
                self.account_cache.backdrops.insert(id, backdrop);
            }
            Event::ArtworkFailed(id) => {
                self.art_failures.insert(id);
            }
            Event::ArtworkRetrying => {
                self.art_failures.clear();
                self.account_cache.profile_requested.clear();
                self.account_cache.popularity_requested.clear();
                if let Some(name) = &self.artist_name {
                    self.service.send(Command::ArtistProfile(
                        name.clone(),
                        self.artist_id.clone(),
                        true,
                    ));
                }
            }
            Event::Lyrics(id, text) => {
                self.lyrics.insert(id, text);
            }
            Event::Playback(snapshot) => {
                if snapshot.queue_revision < self.playback.queue_revision {
                    return;
                }
                // Snapshots arrive about five times a second while playing; when
                // only the position moved, redraw just what shows it.
                let previous = &self.playback;
                let position_only = snapshot.track.as_ref().map(|track| &track.id)
                    == previous.track.as_ref().map(|track| &track.id)
                    && snapshot.playing == previous.playing
                    && snapshot.buffering == previous.buffering
                    && snapshot.duration == previous.duration
                    && snapshot.index == previous.index
                    && snapshot.queue_revision == previous.queue_revision
                    && snapshot.can_undo_queue == previous.can_undo_queue
                    && snapshot.error == previous.error;
                let progress = if snapshot.duration > 0. {
                    snapshot.position / snapshot.duration * 1000.
                } else {
                    0.
                };
                if self.scrub.is_none() {
                    self.seek.update(cx, |slider, cx| {
                        slider.set_value(progress as f32, window, cx)
                    });
                }
                self.playback = snapshot;
                self.playback_at = Instant::now();
                self.queue_ui.retain_selection(&self.playback.queue_ids);
                if position_only {
                    self.notify_part(Part::Player, cx);
                    // Timed lyrics draw in their own part, so redraw it when
                    // the line moves or the next one needs a wake armed.
                    if self.lyrics_need_redraw() {
                        self.notify_part(Part::NowLyrics, cx);
                        self.notify_part(Part::InfoBody, cx);
                    }
                    if self.workspace == Workspace::Queue && !self.blank {
                        // The playing queue row shows elapsed time.
                        self.notify_part(Part::Center, cx);
                    }
                    return;
                }
            }
            Event::Status(status) => self.status = status,
            Event::Connection(connection) => self.connection = connection,
            Event::Error(error) => {
                if self.fail_album_playlist_creation(&error) {
                    cx.notify();
                    return;
                }
                if self.smart.editor.is_some()
                    && (self.playlist_creation.is_none() || self.creating_smart())
                {
                    self.smart.error = Some(error.clone());
                    self.smart.password_busy = false;
                    if !self.smart.needs_password {
                        self.smart.loading = false;
                        self.smart.saving = false;
                    }
                }
                if self.status == "Connecting…" {
                    self.status = "Could not connect".into();
                }
                self.error = Some(error);
                self.loading_more = false;
            }
            Event::MutationDone(mutation) => {
                if self.feedback.finished(&mutation) {
                    self.error = None;
                }
                self.complete_playlist_creation(&mutation, window, cx);
                if let Mutation::RenamePlaylist(ref id, _) = mutation
                    && self.playlist_rename.as_ref() == Some(id)
                {
                    self.playlist_rename = None;
                }
                self.status = match &mutation {
                    Mutation::ReorderPlaylist { .. } => "Playlist order saved".into(),
                    Mutation::AddToPlaylist(id, ids) => {
                        let name = self.playlists.name_of(id).unwrap_or("the playlist");
                        match ids.len() {
                            1 => format!("Added 1 song to “{name}”"),
                            count => format!("Added {count} songs to “{name}”"),
                        }
                    }
                    _ => "Saved to Navidrome".into(),
                };
            }
            Event::MutationFailed(mutation, error) => {
                if !self.fail_playlist_creation(&mutation, &error) {
                    self.feedback.failed(mutation);
                    self.error = Some(error);
                }
            }
        }
        cx.notify();
    }
}
