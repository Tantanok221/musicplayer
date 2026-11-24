use super::*;

impl MusicApp {
    pub(super) fn inspected_track(&self) -> Option<Arc<Track>> {
        match self.preferences.layout.info_source {
            InfoSource::Selected => self
                .selected
                .clone()
                .or_else(|| {
                    self.selected_album
                        .as_ref()
                        .and_then(|a| self.group_tracks.get(&a.id))
                        .and_then(|tracks| tracks.first().cloned())
                })
                .or_else(|| self.display_track()),
            InfoSource::Playing => self.display_track(),
        }
    }

    /// Track information, as a header part above a content part, so lyric
    /// animations redraw only the content.
    pub(super) fn track_information(&mut self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(ink(cx).panel)
            .child(
                self.part(
                    Part::InfoHeader,
                    style()
                        .w_full()
                        .h(px(sections::INFO_HEADER_HEIGHT))
                        .flex_shrink_0(),
                    cx,
                ),
            )
            .child(self.part(Part::InfoBody, style().w_full().flex_1().min_h_0(), cx))
            .into_any_element()
    }

    /// Track information's title row with its source switch, and its tabs.
    pub(super) fn track_information_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let source = self.preferences.layout.info_source;
        let tab = self.preferences.layout.info_tab;
        let mut header = div()
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .justify_between()
            .flex_shrink_0()
            .border_b_1()
            .border_color(colors.line)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child("Track Information"),
            );
        let mut sources = div().flex();
        for (value, label) in [
            (InfoSource::Selected, "Selected"),
            (InfoSource::Playing, "Playing"),
        ] {
            sources = sources.child(
                Button::new(SharedString::from(format!("info-{label}")))
                    .ghost()
                    .compact()
                    .h(px(22.))
                    .px_1()
                    .label(label)
                    .text_size(px(10.))
                    .text_color(if source == value {
                        colors.accent
                    } else {
                        colors.muted
                    })
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.preferences.layout.info_source = value;
                        app.save_preferences();
                        cx.notify();
                    })),
            );
        }
        header = header.child(sources);
        let mut tabs = div()
            .h(px(29.))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(colors.line);
        for (value, label) in [
            (InfoTab::Details, "Details"),
            (InfoTab::Lyrics, "Lyrics"),
            (InfoTab::Artwork, "Artwork"),
            (InfoTab::Properties, "Properties"),
        ] {
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("info-tab-{label}")))
                    .test_support()
                    .role(Role::Tab)
                    .aria_label(label)
                    .aria_selected(value == tab)
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(if value == tab {
                        colors.accent
                    } else {
                        colors.muted
                    })
                    .border_b_1()
                    .border_color(if value == tab {
                        colors.accent
                    } else {
                        colors.line
                    })
                    .cursor_pointer()
                    .child(label)
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.preferences.layout.info_tab = value;
                        if value == InfoTab::Lyrics {
                            app.preferences.layout.info_source = InfoSource::Playing;
                        }
                        app.lyrics_follow = None;
                        app.save_preferences();
                        cx.notify();
                    })),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.panel)
            .child(header)
            .child(tabs)
            .into_any_element()
    }

    /// Details, lyrics or artwork for the selected or playing track.
    pub(super) fn track_information_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let source = self.preferences.layout.info_source;
        let tab = self.preferences.layout.info_tab;
        let mut content = div()
            .id("information-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3();
        if let Some(track) = self.inspected_track() {
            match tab {
                InfoTab::Details => {
                    let art = self.art(&track.cover_art, 66., cx);
                    let summary = div()
                        .id("inspected-track")
                        .test_support()
                        .role(Role::Group)
                        .aria_label(format!(
                            "{} track: {}",
                            if source == InfoSource::Selected {
                                "Selected"
                            } else {
                                "Playing"
                            },
                            track.title
                        ))
                        .flex()
                        .gap_2()
                        .mb_3()
                        .child(art)
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(track.title.clone()),
                                )
                                .child(div().text_size(px(11.)).text_color(colors.muted).child(
                                    self.artist_links(
                                        track.artist_credits(),
                                        format!("track_information-artist-1-{}", track.id),
                                        cx,
                                    ),
                                ))
                                .child(div().text_size(px(10.)).text_color(colors.muted).child(
                                    self.year_link(
                                        track.year,
                                        format!("details-year-{}", track.id),
                                        cx,
                                    ),
                                )),
                        );
                    content = content.child(summary);
                    let mut fields = vec![
                        ("Album", track.album.clone()),
                        ("Album Artist", track.album_artist_label().into()),
                        ("Genre", track.genres_label()),
                        ("Time", duration_label(track.duration)),
                        ("Format", technical(&track)),
                        (
                            "Track",
                            format!(
                                "{} / Disc {}",
                                track.track.map_or("—".into(), |v| v.to_string()),
                                track.disc_number.unwrap_or(1)
                            ),
                        ),
                        ("Rating", format!("{} / 5", track.user_rating.unwrap_or(0))),
                        (
                            "Play count",
                            track.play_count.map_or("—".into(), |v| v.to_string()),
                        ),
                    ];
                    for contributor in &track.contributors {
                        fields.push((contributor.role.as_str(), contributor.artist.name.clone()));
                    }
                    if !track.music_brainz_id.is_empty() {
                        fields.push(("MusicBrainz", track.music_brainz_id.clone()));
                    }
                    if !track.comment.is_empty() {
                        fields.push(("Comment", track.comment.clone()));
                    }
                    for (label, value) in fields {
                        if !value.is_empty() {
                            let value = if label == "Album Artist" {
                                self.artist_link(
                                    value,
                                    track
                                        .album_artists
                                        .first()
                                        .map_or(String::new(), |artist| artist.id.clone()),
                                    format!("details-album-artist-{}", track.id),
                                    cx,
                                )
                            } else if label == "Genre" {
                                self.genre_links(&value, format!("details-genres-{}", track.id), cx)
                            } else {
                                div().child(value).into_any_element()
                            };
                            content = content.child(
                                div()
                                    .mb_2()
                                    .flex()
                                    .gap_2()
                                    .text_size(px(11.))
                                    .child(
                                        div()
                                            .w(px(78.))
                                            .flex_shrink_0()
                                            .text_color(colors.muted)
                                            .child(label.to_string()),
                                    )
                                    .child(div().min_w_0().flex_1().child(value)),
                            );
                        }
                    }
                }
                InfoTab::Artwork => {
                    let side = (self.preferences.layout.right_width - 28.).clamp(160., 480.);
                    content = content
                        .child(self.art(&track.cover_art, side, cx))
                        .child(div().pt_2().text_size(px(11.)).child(track.album.clone()));
                }
                InfoTab::Properties => {
                    content = content.child(properties_view(&track, cx));
                }
                InfoTab::Lyrics => {
                    content = div()
                        .id("information-lyrics")
                        .flex_1()
                        .min_h_0()
                        .child(self.lyrics_content(track.clone(), false, window, cx));
                }
            }
        } else {
            content = content.child(div().text_color(colors.muted).text_size(px(11.)).child(
                if source == InfoSource::Playing {
                    "No track playing."
                } else {
                    "Select a track to view its information."
                },
            ));
        }
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(colors.panel)
            .child(content)
            .into_any_element()
    }

    pub(super) fn information_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.workspace == Workspace::Queue && !self.blank {
            return self.track_information(cx);
        }
        v_resizable(SharedString::from(format!(
            "right-panels-{}",
            self.layout_generation
        )))
        .on_resize(cx.listener(
            |app, state: &Entity<gpui_kit::component::resizable::ResizableState>, _, cx| {
                if let Some(size) = state.read(cx).sizes().first() {
                    app.preferences.layout.queue_height = f32::from(*size);
                    app.save_preferences();
                }
            },
        ))
        .child(
            resizable_panel()
                .size(px(self.preferences.layout.queue_height))
                .size_range(px(100.)..px(1200.))
                .child(self.part(Part::SidebarQueue, style().size_full(), cx)),
        )
        .child(
            resizable_panel()
                .size_range(px(150.)..px(2000.))
                .child(self.track_information(cx)),
        )
        .into_any_element()
    }
}

/// Read-only tag and file details, grouped like MusicBee's Properties tab.
fn properties_view(track: &Track, cx: &App) -> AnyElement {
    let colors = ink(cx);
    let mut view = div()
        .id("track-properties")
        .test_support()
        .role(Role::Group)
        .aria_label(format!("Properties of {}", track.title))
        .flex()
        .flex_col()
        .gap_3();
    for section in crate::metadata::properties(track) {
        let mut group = div().flex().flex_col().gap_1().child(
            div()
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.muted)
                .child(section.title),
        );
        for row in section.rows {
            group = group.child(
                div()
                    .id(SharedString::from(format!("property-{}", row.key)))
                    .test_support()
                    .aria_label(format!("{}: {}", row.label, row.value))
                    .flex()
                    .gap_2()
                    .text_size(px(11.))
                    .child(
                        div()
                            .w(px(86.))
                            .flex_shrink_0()
                            .text_color(colors.muted)
                            .child(row.label),
                    )
                    .child(div().min_w_0().flex_1().child(row.value)),
            );
        }
        view = view.child(group);
    }
    view.into_any_element()
}
