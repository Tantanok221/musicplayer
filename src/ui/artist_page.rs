//! The artist page. Its library releases can run to hundreds of covers, so the
//! page is a list of rows and only the rows on screen are built each frame.
use super::*;

const POPULAR_PER_ALBUM: usize = 3;

const ARTIST_SONG_COLUMN_MIN: f32 = 300.;

/// Wrapped line count of the artist biography at its 11px system font size.
fn biography_lines(text: &str, width: f32, cx: &App) -> usize {
    if width <= 0. {
        return 0;
    }
    let mut wrapper = cx
        .text_system()
        .line_wrapper(font(crate::platform::UI_FONT), px(11.));
    text.lines()
        .map(|line| {
            1 + wrapper
                .wrap_line(&[LineFragment::text(line)], px(width), Default::default())
                .count()
        })
        .sum()
}

/// One row of the artist page.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    /// Portrait, name, biography, genres and the two song lists.
    Header,
    /// A release category's heading.
    Title(ReleaseCategory),
    /// One line of covers within a category.
    Covers(ReleaseCategory, usize),
    /// The selected release's panel, under the line holding its cover.
    Panel(ReleaseCategory),
    Missing,
    Similar,
    /// Space after the last row, matching the page's padding.
    End,
}

/// The rows the page shows, with the releases they draw from.
#[derive(Default)]
pub(super) struct Rows {
    rows: Vec<Row>,
    sections: Vec<(ReleaseCategory, Vec<Arc<Album>>)>,
    columns: usize,
    side: f32,
}

const CATEGORIES: [ReleaseCategory; 4] = [
    ReleaseCategory::Album,
    ReleaseCategory::Ep,
    ReleaseCategory::Single,
    ReleaseCategory::Other,
];

impl MusicApp {
    pub(super) fn artist_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let page = || {
            div()
                .id("artist-page")
                .size_full()
                .min_w_0()
                .overflow_y_scroll()
                .p_4()
        };
        let Some(albums) = self.artist_albums.clone() else {
            return page()
                .child(div().child("Loading artist releases…"))
                .into_any_element();
        };
        // Search results and artists with no releases are short; they scroll as one page.
        if !self.search_text.trim().is_empty() {
            let data = self.artist_data.clone().unwrap_or_default();
            let sections = search_releases(&albums, &data.tracks, &self.search_text);
            let mut page = page().child(self.artist_header(&albums, cx));
            if sections.is_empty() {
                return page
                    .child(
                        div()
                            .mt_5()
                            .text_color(colors.muted)
                            .child("No releases match your search."),
                    )
                    .into_any_element();
            }
            for (category, hits) in sections {
                page = page.child(self.artist_search_section(category, hits, cx));
            }
            return page.into_any_element();
        }
        if albums.is_empty() {
            let name = self.artist_name.clone().unwrap_or_default();
            let profile = self.current_profile();
            let header = self.artist_header(&albums, cx);
            let songs = self.artist_songs(cx);
            let missing = self.missing_releases(&name, cx);
            let similar = self.similar_artists(profile, cx);
            return page()
                .child(header)
                .child(songs)
                .children(missing)
                .child(similar)
                .child(
                    div()
                        .text_color(colors.muted)
                        .child("No releases for this artist in your library."),
                )
                .into_any_element();
        }
        self.sync_artist_rows(&albums);
        let entity = cx.entity();
        list(self.artist_list.clone(), move |ix, _, cx| {
            entity.update(cx, |app, cx| app.artist_row(ix, cx))
        })
        .size_full()
        .into_any_element()
    }

    fn current_profile(&self) -> Option<Arc<ArtistProfile>> {
        let name = self.artist_name.as_deref().unwrap_or_default();
        self.artist_profiles
            .get(&cache_key(name, &self.artist_id))
            .cloned()
    }

    /// Works out the rows for the current releases, width and selection, and
    /// tells the list about any that changed so it keeps its scroll position.
    fn sync_artist_rows(&mut self, albums: &[Arc<Album>]) {
        let (columns, side) = cover_layout((self.library_width - 8.).max(0.));
        let columns = columns.max(1);
        let mut rows = vec![Row::Header];
        let mut sections = Vec::new();
        for category in CATEGORIES {
            let releases = albums
                .iter()
                .filter(|album| album.category() == category)
                .cloned()
                .collect::<Vec<_>>();
            if releases.is_empty() {
                continue;
            }
            rows.push(Row::Title(category));
            let panel_row = self.selected_album.as_ref().and_then(|selected| {
                releases
                    .iter()
                    .position(|album| album.id == selected.id)
                    .map(|index| index / columns)
            });
            for row in 0..releases.len().div_ceil(columns) {
                rows.push(Row::Covers(category, row));
                if panel_row == Some(row) {
                    rows.push(Row::Panel(category));
                }
            }
            sections.push((category, releases));
        }
        rows.extend([Row::Missing, Row::Similar, Row::End]);
        let old = &self.artist_rows.rows;
        if *old != rows || self.artist_rows.columns != columns {
            // Replace only the rows between the unchanged start and end.
            let start = old.iter().zip(&rows).take_while(|(a, b)| a == b).count();
            let end = old[start..]
                .iter()
                .rev()
                .zip(rows[start..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let (start, end) = if self.artist_rows.columns != columns {
                (0, 0)
            } else {
                (start, end)
            };
            self.artist_list
                .splice(start..old.len() - end, rows.len() - start - end);
        }
        self.artist_rows = Rc::new(Rows {
            rows,
            sections,
            columns,
            side,
        });
    }

    fn artist_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.artist_rows.clone();
        let Some(row) = rows.rows.get(ix).copied() else {
            return div().into_any_element();
        };
        let releases = |category| {
            rows.sections
                .iter()
                .find(|(c, _)| *c == category)
                .map(|(_, releases)| releases.as_slice())
                .unwrap_or_default()
        };
        let body = match row {
            Row::Header => {
                let albums = self.artist_albums.clone().unwrap_or_default();
                div()
                    .pt_4()
                    .child(self.artist_header(&albums, cx))
                    .child(self.artist_songs(cx))
                    .into_any_element()
            }
            Row::Title(category) => div()
                .mt_5()
                .mb_3()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!(
                    "{} ({})",
                    category.label(),
                    releases(category).len()
                ))
                .into_any_element(),
            Row::Covers(category, line) => {
                let mut covers = div().pb_4().flex().gap_4().min_w_0();
                for album in releases(category)
                    .iter()
                    .skip(line * rows.columns)
                    .take(rows.columns)
                {
                    covers = covers.child(self.artist_release_card(album, rows.side, cx));
                }
                covers.into_any_element()
            }
            Row::Panel(_) => match self.selected_album.clone() {
                Some(album) => div()
                    .mb_4()
                    .border_y_1()
                    .border_color(ink(cx).line)
                    .child(self.album_panel(album, false, true, cx))
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Row::Missing => {
                let name = self.artist_name.clone().unwrap_or_default();
                self.missing_releases(&name, cx)
                    .unwrap_or_else(|| div().into_any_element())
            }
            Row::Similar => {
                let profile = self.current_profile();
                self.similar_artists(profile, cx)
            }
            Row::End => div().h(px(16.)).into_any_element(),
        };
        div()
            .w_full()
            .min_w_0()
            .px_4()
            .child(body)
            .into_any_element()
    }

    /// Portrait, name, release count, biography, profile link and genres.
    fn artist_header(&mut self, albums: &[Arc<Album>], cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let name = self.artist_name.clone().unwrap_or_default();
        let profile = self.current_profile();
        let data = self.artist_data.clone().unwrap_or_default();
        let fallback = albums
            .first()
            .map(|album| album.cover_art.as_str())
            .unwrap_or_default();
        let artist_id = self.artist_id.clone();
        let mut header = div().child(
            div()
                .mb_4()
                .flex()
                .items_center()
                .gap_4()
                .child(self.artist_portrait(&name, &artist_id, fallback, 80., cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(colors.muted)
                                .child("ARTIST"),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_size(px(25.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_size(px(11.))
                                .text_color(colors.muted)
                                .child(format!(
                                    "{} release{} in your library",
                                    albums.len(),
                                    if albums.len() == 1 { "" } else { "s" }
                                )),
                        ),
                ),
        );
        if let Some(profile) = &profile {
            if !profile.biography.is_empty() {
                let overflows = self.biography_expanded
                    || self.biography_line_count(&profile.biography, cx) > 3;
                header = header
                    .child(
                        // A distinct id per state: text keeps its measured layout across
                        // frames, so removing the clamp alone leaves three lines.
                        div()
                            .id(if self.biography_expanded {
                                "artist-bio-full"
                            } else {
                                "artist-bio-clamped"
                            })
                            .mb_1()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .when(!self.biography_expanded, |text| {
                                text.line_clamp(3).text_ellipsis()
                            })
                            .child(profile.biography.clone()),
                    )
                    .when(overflows, |header| {
                        header.child(
                            Button::new("artist-bio-more")
                                .ghost()
                                .small()
                                .label(if self.biography_expanded {
                                    "Show less"
                                } else {
                                    "Read more"
                                })
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.biography_expanded = !app.biography_expanded;
                                    cx.notify();
                                })),
                        )
                    });
            }
            if let Some(url) = profile_url(&profile.url) {
                header = header.child(
                    Button::new("artist-source")
                        .ghost()
                        .small()
                        .label("Artist profile ↗")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
        }
        if !data.genres.is_empty() {
            let mut chips = div()
                .mt_2()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .text_size(px(10.))
                .text_color(colors.muted)
                .child("From your library:");
            for (index, genre) in data.genres.iter().enumerate() {
                chips = chips.child(self.metadata_link(
                    ExplorerFilter::Genre(genre.clone()),
                    format!("artist-genre-{index}"),
                    cx,
                ));
            }
            header = header.child(chips);
        }
        header.into_any_element()
    }

    /// Wrapping the biography means shaping all of its text, so the line count
    /// is kept until the text or the width changes.
    fn biography_line_count(&mut self, text: &str, cx: &App) -> usize {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let key = (hasher.finish(), (self.library_width - 32.).to_bits());
        if let Some((hash, width, lines)) = self.bio_lines
            && (hash, width) == key
        {
            return lines;
        }
        let lines = biography_lines(text, self.library_width - 32., cx);
        self.bio_lines = Some((key.0, key.1, lines));
        lines
    }

    /// Popular songs and the user's most played, side by side when they fit.
    fn artist_songs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let popular = self
            .current_profile()
            .map(|profile| profile.popular.clone())
            .unwrap_or_default();
        let most_played = self
            .artist_data
            .as_ref()
            .map(|data| data.most_played.clone())
            .unwrap_or_default();
        let column = |section: AnyElement| {
            div()
                .flex_1()
                .min_w(px(ARTIST_SONG_COLUMN_MIN))
                .child(section)
        };
        let mut songs = div().mt_4().flex().flex_wrap().gap_4().min_w_0();
        if !popular.is_empty() {
            songs = songs.child(column(self.artist_song_section(
                "Popular songs",
                popular,
                false,
                cx,
            )));
        }
        songs
            .child(column(self.artist_song_section(
                "Your most played",
                most_played,
                true,
                cx,
            )))
            .into_any_element()
    }

    fn artist_release_card(
        &mut self,
        album: &Arc<Album>,
        side: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let open = album.clone();
        let selected = self
            .selected_album
            .as_ref()
            .is_some_and(|selected| selected.id == album.id);
        div()
            .id(SharedString::from(format!("artist-release-{}", album.id)))
            .test_support()
            .role(Role::Button)
            .aria_label(format!("Select release {}", album.name))
            .aria_selected(selected)
            .w(px(side))
            .min_w_0()
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
            .child(div().text_size(px(10.)).text_color(colors.muted).child(
                if let Some(year) = album.year {
                    self.metadata_link(
                        ExplorerFilter::Year(year),
                        format!("release-year-{}", album.id),
                        cx,
                    )
                } else {
                    div().child("Year unknown").into_any_element()
                },
            ))
            .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                app.select_album(open.clone(), cx);
                if event.click_count() == 2 {
                    app.service.send(Command::PlayAlbum(open.id.clone()));
                }
            }))
            .context_menu(self.album_menu(album, cx))
            .into_any_element()
    }
}

impl MusicApp {
    /// Popularity arrives later through `Event::TopSongs`; selection never waits for it.
    pub(super) fn request_popularity(&mut self, artists: &[NamedId]) {
        for artist in artists {
            if self
                .popularity_requested
                .insert(cache_key(&artist.name, &artist.id))
            {
                self.service
                    .send(Command::TopSongs(artist.name.clone(), artist.id.clone()));
            }
        }
    }

    pub(super) fn popular_tracks(&self, album_id: &str, artists: &[NamedId]) -> HashSet<String> {
        let mut popular = HashSet::new();
        for top in artists
            .iter()
            .filter_map(|artist| self.top_songs.get(&cache_key(&artist.name, &artist.id)))
        {
            for id in top.for_album(album_id, POPULAR_PER_ALBUM) {
                if popular.len() < POPULAR_PER_ALBUM {
                    popular.insert(id);
                }
            }
        }
        popular
    }

    pub(super) fn artist_portrait(
        &mut self,
        name: &str,
        id: &str,
        fallback: &str,
        side: f32,
        cx: &App,
    ) -> AnyElement {
        let key = cache_key(name, id);
        if self.profile_requested.insert(key.clone()) {
            self.service
                .send(Command::ArtistProfile(name.into(), id.into(), false));
        }
        let portrait = self
            .artist_profiles
            .get(&key)
            .map(|profile| profile.portrait.clone())
            .filter(|portrait| !portrait.is_empty() && !self.art_failures.contains(portrait))
            .unwrap_or_else(|| fallback.into());
        self.art(&portrait, side, cx)
    }

    pub(super) fn artist_song_section(
        &mut self,
        title: &'static str,
        tracks: Vec<Arc<Track>>,
        personal: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let expanded = if personal {
            self.most_played_expanded
        } else {
            self.popular_expanded
        };
        let mut section = div().w_full().min_w_0().flex().flex_col().gap_1().child(
            div()
                .mb_2()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        );
        if tracks.is_empty() {
            section = section.child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child("No plays recorded yet."),
            );
        }
        for (i, track) in tracks
            .iter()
            .take(if expanded { 10 } else { 5 })
            .enumerate()
        {
            let row = self.track_row(
                track.clone(),
                i + 1,
                format!("artist-{}-{}", personal, track.id),
                None,
                None,
                cx,
            );
            let art = self.art(&track.cover_art, 36., cx);
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .size(px(36.))
                            .flex_shrink_0()
                            .rounded_md()
                            .overflow_hidden()
                            .child(art),
                    )
                    .child(div().flex_1().min_w_0().child(row))
                    .when(personal, |row| {
                        row.child(
                            div()
                                .w(px(48.))
                                .flex_shrink_0()
                                .text_size(px(10.))
                                .text_color(colors.muted)
                                .child(format!("{} plays", track.play_count.unwrap_or(0))),
                        )
                    }),
            );
        }
        if tracks.len() > 5 {
            section = section.child(
                Button::new(if personal {
                    "more-personal"
                } else {
                    "more-popular"
                })
                .ghost()
                .small()
                .label(if expanded { "Show less" } else { "Show more" })
                .on_click(cx.listener(move |app, _, _, cx| {
                    if personal {
                        app.most_played_expanded = !app.most_played_expanded;
                    } else {
                        app.popular_expanded = !app.popular_expanded;
                    }
                    cx.notify();
                })),
            );
        }
        section.into_any_element()
    }

    pub(super) fn similar_artists(
        &mut self,
        profile: Option<Arc<ArtistProfile>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(profile) = profile.filter(|profile| !profile.similar.is_empty()) else {
            return div().into_any_element();
        };
        let colors = ink(cx);
        let mut cards = div().flex().flex_wrap().gap_3();
        for similar in profile.similar.iter().take(12) {
            let item = similar.clone();
            let fallback = self
                .view
                .navigator
                .artists
                .iter()
                .chain(&self.view.navigator.album_artists)
                .find(|artist| normalize(&artist.label) == normalize(&item.name))
                .map(|artist| artist.artwork.clone())
                .unwrap_or_default();
            let art = if item.in_library {
                self.artist_portrait(&item.name, &item.id, &fallback, 88., cx)
            } else {
                self.art(&item.portrait, 88., cx)
            };
            cards = cards.child(
                div()
                    .id(SharedString::from(format!("similar-{}", item.name)))
                    .role(Role::Button)
                    .aria_label(format!(
                        "{} {}",
                        if item.in_library {
                            "Open artist"
                        } else {
                            "View on Last.fm:"
                        },
                        item.name
                    ))
                    .w(px(112.))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .cursor_pointer()
                    .text_color(if item.in_library {
                        colors.text
                    } else {
                        colors.muted
                    })
                    .items_center()
                    .text_center()
                    .child(
                        div()
                            .size(px(88.))
                            .flex_shrink_0()
                            .rounded_full()
                            .overflow_hidden()
                            .when(!item.in_library, |image| image.opacity(0.4))
                            .child(art),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_size(px(11.))
                            .truncate()
                            .child(item.name.clone()),
                    )
                    .when(!item.in_library, |card| {
                        card.child(div().text_size(px(9.)).child("Not in your library"))
                    })
                    .on_click(cx.listener(move |app, _, window, cx| {
                        if item.in_library {
                            app.open_artist(item.name.clone(), item.id.clone(), window, cx);
                        } else {
                            cx.open_url(&lastfm_url(&item.name));
                        }
                    })),
            );
        }
        div()
            .mt_5()
            .child(
                div()
                    .mb_3()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Similar artists to {}", profile.name)),
            )
            .child(cards)
            .into_any_element()
    }

    pub(super) fn artist_search_section(
        &mut self,
        category: ReleaseCategory,
        hits: Vec<ReleaseHit>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let count = hits.len();
        let mut section = div().mt_5().min_w_0().child(
            div()
                .mb_3()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("{} ({count})", category.label())),
        );
        for hit in hits {
            let album = hit.album;
            let selected = self
                .selected_album
                .as_ref()
                .is_some_and(|selected| selected.id == album.id);
            let open = album.clone();
            let year = if let Some(year) = album.year {
                self.metadata_link(
                    ExplorerFilter::Year(year),
                    format!("search-release-year-{}", album.id),
                    cx,
                )
            } else {
                div().child("Year unknown").into_any_element()
            };
            let mut block = div()
                .id(SharedString::from(format!("artist-search-{}", album.id)))
                .mb_4()
                .min_w_0()
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "artist-search-release-{}",
                            album.id
                        )))
                        .role(Role::Button)
                        .aria_label(format!("Select release {}", album.name))
                        .flex()
                        .items_center()
                        .gap_3()
                        .cursor_pointer()
                        .pb_2()
                        .border_b_2()
                        .border_color(if selected {
                            colors.accent
                        } else {
                            colors.background
                        })
                        .child(self.art(&album.cover_art, 56., cx))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .truncate()
                                        .child(album.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(colors.muted)
                                        .child(year),
                                ),
                        )
                        .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                            app.select_album(open.clone(), cx);
                            if event.click_count() == 2 {
                                app.service.send(Command::PlayAlbum(open.id.clone()));
                            }
                        }))
                        .context_menu(self.album_menu(&album, cx)),
                );
            for (index, track) in hit.songs.iter().cloned().enumerate() {
                let number = track.track.map_or(index + 1, |number| number as usize);
                block = block.child(self.track_row(
                    track,
                    number,
                    format!("artist-search-{}-{index}", album.id),
                    None,
                    None,
                    cx,
                ));
            }
            if selected {
                block = block.child(
                    div()
                        .mt_2()
                        .border_y_1()
                        .border_color(colors.line)
                        .child(self.album_panel(album, false, true, cx)),
                );
            }
            section = section.child(block);
        }
        section.into_any_element()
    }
}
