use super::*;
use crate::playlists::PlaylistAlbum;
use tracks::TrackPosition;

enum AlbumPanelSource<'a> {
    Library {
        new_artist: bool,
        selected_panel: bool,
    },
    Playlist {
        group: &'a PlaylistAlbum,
        playlist: &'a Playlist,
        editable: bool,
        has_details: bool,
    },
}

impl MusicApp {
    pub(super) fn album_group(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(album) = self.view.albums.get(index).cloned() else {
            return div().into_any_element();
        };
        let selected_panel =
            self.main_view() == MainView::AlbumCovers || self.artist_name.is_some();
        let new_artist = !selected_panel
            && self.query.ordering.as_ref().is_some_and(|spec| {
                spec.grouping != Grouping::None
                    && (index == 0
                        || spec.group_label(&self.view.albums[index - 1])
                            != spec.group_label(&album))
            });
        self.album_panel(album, new_artist, selected_panel, cx)
    }

    pub(super) fn album_metadata(
        &self,
        album: &Album,
        show_artist: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let row = |label: &'static str, value: AnyElement| {
            div()
                .flex()
                .items_start()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .w(px(38.))
                        .flex_shrink_0()
                        .text_color(colors.muted)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(colors.text)
                        .child(value),
                )
        };
        let mut metadata = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .pb_2()
            .min_w_0()
            .text_size(px(10.));
        let credits = album.artist_credits();
        if show_artist && !credits.is_empty() {
            metadata = metadata.child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text)
                    .child(self.artist_links(credits, format!("album-artist-{}", album.id), cx)),
            );
        }
        if album.year.is_some() {
            metadata = metadata.child(row(
                "Year",
                self.year_link(album.year, format!("album-year-{}", album.id), cx),
            ));
        }
        if !album.genre.trim().is_empty() {
            metadata = metadata.child(row(
                "Genre",
                self.genre_links(&album.genre, format!("album-genres-{}", album.id), cx),
            ));
        }
        let labels = album.record_label_names();
        if !labels.is_empty() {
            let mut links = div().flex().flex_wrap().items_center().min_w_0().gap_1();
            for (index, label) in labels.into_iter().enumerate() {
                if index > 0 {
                    links = links.child(div().text_color(colors.muted).child("·"));
                }
                links = links.child(self.metadata_link(
                    ExplorerFilter::RecordLabel(label),
                    format!("album-label-{}-{index}", album.id),
                    cx,
                ));
            }
            metadata = metadata.child(row("Label", links.into_any_element()));
        }
        metadata.into_any_element()
    }

    pub(super) fn album_panel(
        &mut self,
        album: Arc<Album>,
        new_artist: bool,
        selected_panel: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.request_group(&album.id);
        self.render_album_panel(
            &album,
            AlbumPanelSource::Library {
                new_artist,
                selected_panel,
            },
            cx,
        )
    }

    pub(super) fn playlist_album_panel(
        &mut self,
        group: &PlaylistAlbum,
        playlist: &Playlist,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let details = self.playlists.album_details().cloned();
        let has_details = details.is_some();
        let album = details.unwrap_or_else(|| Arc::new(group.fallback_album()));
        self.render_album_panel(
            &album,
            AlbumPanelSource::Playlist {
                group,
                playlist,
                editable,
                has_details,
            },
            cx,
        )
    }

    fn render_album_panel(
        &mut self,
        album: &Arc<Album>,
        source: AlbumPanelSource<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (new_artist, selected_panel, has_details) = match source {
            AlbumPanelSource::Library {
                new_artist,
                selected_panel,
            } => (new_artist, selected_panel, true),
            AlbumPanelSource::Playlist { has_details, .. } => (false, true, has_details),
        };
        let colors = ink(cx);
        let tracks = match source {
            AlbumPanelSource::Library { .. } => self.group_tracks.get(&album.id).map(|tracks| {
                tracks
                    .iter()
                    .enumerate()
                    .map(|(i, track)| (track.clone(), format!("group-{}-{i}", album.id), None))
                    .collect::<Vec<_>>()
            }),
            AlbumPanelSource::Playlist {
                group,
                playlist,
                editable,
                ..
            } => Some(
                group
                    .entries
                    .iter()
                    .map(|&index| {
                        (
                            playlist.entry[index].clone(),
                            format!("playlist-{index}"),
                            Some(TrackPosition::playlist(playlist, index, editable)),
                        )
                    })
                    .collect(),
            ),
        };
        let panel_key = match source {
            AlbumPanelSource::Playlist { group, .. } if album.id.is_empty() => &group.key,
            _ => &album.id,
        };
        let cover = div()
            .id(SharedString::from(format!("album-art-{panel_key}")))
            .child(self.art(&album.cover_art, 124., cx))
            .when(
                matches!(source, AlbumPanelSource::Library { .. }),
                |cover| {
                    let selection = album.clone();
                    cover.cursor_pointer().on_click(
                        cx.listener(move |app, _, _, cx| app.select_album(selection.clone(), cx)),
                    )
                },
            );
        let cover = if has_details {
            cover
                .context_menu(self.album_menu(album, cx))
                .into_any_element()
        } else {
            cover.into_any_element()
        };
        let mut right = div()
            .flex_1()
            .min_w_0()
            .when(self.library_width < 520., |column| column.w_full())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(album.name.clone()),
                    )
                    .when(has_details, |header| {
                        header.child(self.album_favorite(album, cx))
                    }),
            )
            .child(
                self.album_metadata(
                    album,
                    selected_panel
                        || self
                            .query
                            .ordering
                            .as_ref()
                            .is_none_or(|spec| spec.grouping != Grouping::AlbumArtist),
                    cx,
                ),
            );
        if has_details && let Some(job) = self.album_download(album) {
            let arrived = match job.expected {
                Some(expected) if album.song_count < expected => format!(
                    " · {} of {expected} tracks are in the library so far",
                    album.song_count
                ),
                _ => String::new(),
            };
            right = right.child(div().text_size(px(10.)).text_color(colors.accent).child(
                match job.state {
                    JobState::Failed => format!(
                        "{}{arrived} · open the artist page to pick another folder",
                        download_caption(job)
                    ),
                    _ => format!("{} from Soulseek{arrived}", download_caption(job)),
                },
            ));
        }
        if let Some(tracks) = tracks {
            let popular = self.popular_tracks(&album.id, &album.artist_credits());
            let discs = tracks
                .iter()
                .filter_map(|(track, _, _)| track.disc_number)
                .collect::<HashSet<_>>();
            let mut last_disc = None;
            for (i, (track, key, position)) in tracks.into_iter().enumerate() {
                if discs.len() > 1 && last_disc != track.disc_number {
                    last_disc = track.disc_number;
                    right = right.child(
                        div()
                            .h(px(23.))
                            .pt_1()
                            .text_size(px(10.))
                            .text_color(colors.muted)
                            .child(format!("Disc {}", last_disc.unwrap_or(1))),
                    );
                }
                let number = track.track.unwrap_or(i as u32 + 1) as usize;
                let flame = Some(popular.contains(&track.id));
                right = right.child(self.track_row(track, number, key, position, flame, cx));
            }
        } else {
            right = right.child(
                div()
                    .h(px(90.))
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child("Loading album tracks…"),
            );
        }
        div()
            .id(SharedString::from(format!("album-panel-{panel_key}")))
            .test_support()
            .role(Role::Group)
            .aria_label(format!("Album {} by {}", album.name, album.artist))
            .w_full()
            .min_w_0()
            .px_3()
            .pb_4()
            .pt_2()
            .flex()
            .flex_col()
            .gap_2()
            .when(new_artist, |s| {
                s.child(
                    div()
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(colors.line)
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(
                            if self
                                .query
                                .ordering
                                .as_ref()
                                .is_some_and(|spec| spec.grouping == Grouping::AlbumArtist)
                            {
                                self.artist_links(
                                    album.artist_credits(),
                                    format!("group-artist-{}", album.id),
                                    cx,
                                )
                            } else {
                                div()
                                    .child(
                                        self.query
                                            .ordering
                                            .as_ref()
                                            .map(|spec| spec.group_label(album))
                                            .unwrap_or_default(),
                                    )
                                    .into_any_element()
                            },
                        ),
                )
            })
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .gap_3()
                    .items_start()
                    .when(self.library_width < 520., |row| row.flex_col())
                    .child(
                        div()
                            .w(px(124.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(cover)
                            .when(has_details, |cover| {
                                cover.child(self.album_rating(album, cx))
                            })
                            .when(!selected_panel, |cover| {
                                cover.child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(colors.muted)
                                        .truncate()
                                        .child(album.name.clone()),
                                )
                            }),
                    )
                    .child(right),
            )
            .into_any_element()
    }
}
