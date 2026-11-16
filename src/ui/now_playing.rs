use super::*;

const ALBUM_PREVIEW_ROWS: usize = 6;

const UP_NEXT_PREVIEW_ROWS: usize = 8;

const BACKDROP_FADE_SECONDS: f32 = 0.7;

/// Fraction of the lyrics viewport height where the current line rests.
const LYRIC_ANCHOR: f32 = 0.4;

const LYRIC_SCROLL_SECONDS: f32 = 0.6;

/// The spectrum redraws at 60 fps; display rate (up to 120 Hz) costs twice the CPU for no visible gain.
const SPECTRUM_FRAME: Duration = Duration::from_micros(16_667);

/// Low and high bar colors: the album's two leading colors, lifted so they
/// read against the scrim, or the theme accent before a palette exists.
/// Three breathing dots marking an instrumental break in timed lyrics.
fn lyric_gap(index: usize, text_size: f32, leading: f32, color: Hsla) -> impl IntoElement {
    let side = (text_size * 0.32).round();
    div()
        .h(px(leading))
        .flex()
        .items_center()
        .gap(px(side * 0.8))
        .children((0..3).map(move |dot| {
            div()
                .size(px(side))
                .rounded_full()
                .bg(color)
                .with_animation(
                    SharedString::from(format!("lyric-gap-{index}-{dot}")),
                    Animation::new(Duration::from_millis(1600)).repeat(),
                    move |element, t| {
                        let phase = (t - dot as f32 * 0.16).rem_euclid(1.);
                        let wave = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
                        element.opacity(0.25 + 0.75 * wave)
                    },
                )
        }))
}

fn spectrum_colors(palette: Option<[u32; 3]>, accent: Hsla, dark: bool) -> (Hsla, Hsla) {
    let vivid = |color: u32| {
        let mut color = Hsla::from(rgb(color));
        color.s = color.s.max(0.45);
        color.l = if dark {
            color.l.clamp(0.55, 0.72)
        } else {
            color.l.clamp(0.36, 0.5)
        };
        color
    };
    match palette {
        Some([first, second, _]) => (vivid(first), vivid(second)),
        None => (accent, accent),
    }
}

/// How much of the theme background covers the Now Playing backdrop.
fn scrim_alpha(dark: bool) -> f32 {
    if dark { 0.55 } else { 0.62 }
}

/// Moves `color` toward `toward` until it reads at `READABLE` against a
/// background of luma `under`; colors that already do are left alone.
fn readable_on(color: Hsla, toward: Hsla, under: f32) -> Hsla {
    use crate::backdrop::{READABLE, contrast};
    (0..=10)
        .map(|step| mix(color, toward, step as f32 / 10.))
        .find(|candidate| contrast(luma_of(*candidate), under) >= READABLE)
        .unwrap_or(toward)
}

impl MusicApp {
    /// Playback position extrapolated from the last snapshot, which arrives every ~200ms.
    pub(super) fn live_position(&self) -> f64 {
        let mut position = self.playback.position;
        if self.playback.playing && !self.playback.buffering {
            position += self.playback_at.elapsed().as_secs_f64().min(0.5);
        }
        if self.playback.duration > 0. {
            position = position.min(self.playback.duration);
        }
        position
    }

    /// Redraws once the next timed line is due, so lyrics stay on the beat without a frame loop.
    pub(super) fn schedule_lyric_wake(
        &mut self,
        lyrics: &Lyrics,
        following: bool,
        now: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = following
            .then(|| {
                (0..lyrics.line.len())
                    .filter_map(|index| Some((index, lyrics.line_position(index)?)))
                    .filter(|(_, start)| *start > now)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
            })
            .flatten()
            .filter(|(_, start)| start - now < 0.4);
        let Some((index, start)) = next else {
            self.lyrics_wake = None;
            return;
        };
        if self
            .lyrics_wake
            .as_ref()
            .is_some_and(|(due, _)| *due == index)
        {
            return;
        }
        let delay = Duration::from_secs_f64(start - now + 0.004);
        // Only the lyrics change on the beat, so only their part redraws.
        let view = window.current_view();
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |_, cx| {
                let cx: &mut App = cx;
                cx.notify(view);
            })
            .ok();
        });
        self.lyrics_wake = Some((index, task));
    }

    /// Redraws the view being rendered after `interval`, coalescing requests,
    /// for animations that don't need display rate. Only that view redraws,
    /// so a part's animation leaves the rest of the window cached.
    pub(super) fn request_frame_after(
        &mut self,
        interval: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = window.current_view();
        if self.frame_timers.contains_key(&view) {
            return;
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(interval).await;
            this.update(cx, |app, cx| {
                app.frame_timers.remove(&view);
                let cx: &mut App = cx;
                cx.notify(view);
            })
            .ok();
        });
        self.frame_timers.insert(view, task);
    }

    pub(super) fn lyrics_content(
        &mut self,
        track: Arc<Track>,
        highlight: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        if !self.lyrics.contains_key(&track.id) && self.lyrics_requested.insert(track.id.clone()) {
            self.service.send(Command::Lyrics(track.clone()));
        }
        let dark = colors.dark;
        let reduce_motion = cx.reduce_motion();
        let scroll = self.lyrics_scroll.clone();
        let viewport = scroll.bounds();
        let height = f32::from(viewport.size.height);
        let (text_size, leading, row_y, row_x, fade, weight) = if highlight {
            (28., 36., 9., 14., 120., FontWeight::BOLD)
        } else {
            (15., 22., 6., 8., 56., FontWeight::SEMIBOLD)
        };
        // The large view sits on a tinted backdrop where pure white reads best.
        let ink_color = if highlight && dark {
            gpui_kit::white()
        } else {
            colors.text
        };
        let mut content = div()
            .id("lyrics-scroll")
            .test_support()
            .role(Role::Group)
            .aria_label(format!("Lyrics for {}", track.title))
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll);
        match self.lyrics.get(&track.id).cloned() {
            Some(Ok(lyrics)) if !lyrics.is_empty() => {
                let is_playing = self
                    .playback
                    .track
                    .as_ref()
                    .is_some_and(|playing| playing.id == track.id);
                let following = is_playing && lyrics.is_timed();
                let now = self.live_position();
                self.schedule_lyric_wake(&lyrics, following, now, window, cx);
                let active = is_playing.then(|| lyrics.active_line(now)).flatten();
                // Padding lets the first and last lines reach the anchor.
                content = if following && height > 0. {
                    content
                        .px_3()
                        .pt(px(height * LYRIC_ANCHOR))
                        .pb(px(height * (1. - LYRIC_ANCHOR)))
                } else {
                    content.p_3()
                };
                let target = |index: usize| -> Option<f32> {
                    if !following {
                        return Some(0.);
                    }
                    let item = scroll.bounds_for_item(index)?;
                    let anchor = viewport.top() + viewport.size.height * LYRIC_ANCHOR;
                    Some(
                        f32::from(anchor - item.center().y)
                            .clamp(-f32::from(scroll.max_offset().y), 0.),
                    )
                };
                let focus_index = active.unwrap_or(0);
                let follow = (track.id.clone(), active);
                if self.lyrics_follow.as_ref() != Some(&follow) {
                    let same_track = self
                        .lyrics_follow
                        .as_ref()
                        .is_some_and(|(id, _)| id == &track.id);
                    if same_track && following && !reduce_motion {
                        self.lyrics_scroll_motion =
                            Some((Instant::now(), f32::from(scroll.offset().y), focus_index));
                    } else {
                        // Layout for new lyrics lands a frame later, so settle over a few.
                        self.lyrics_scroll_motion = None;
                        self.lyrics_snap = 3;
                    }
                    self.lyrics_follow = Some(follow);
                }
                if let Some((started, from, index)) = self.lyrics_scroll_motion {
                    let progress = (started.elapsed().as_secs_f32() / LYRIC_SCROLL_SECONDS).min(1.);
                    let eased = 1. - (1. - progress).powi(4);
                    // Re-read the target each frame so late layout changes are absorbed.
                    if let Some(target) = target(index) {
                        scroll.set_offset(point(
                            scroll.offset().x,
                            px(from + (target - from) * eased),
                        ));
                    }
                    if progress < 1. {
                        window.request_animation_frame();
                    } else {
                        self.lyrics_scroll_motion = None;
                    }
                } else if self.lyrics_snap > 0 {
                    self.lyrics_snap -= 1;
                    match target(focus_index) {
                        Some(target) => scroll.set_offset(point(scroll.offset().x, px(target))),
                        None => scroll.scroll_to_top_of_item(focus_index),
                    }
                    window.request_animation_frame();
                }
                let offset_y = scroll.offset().y;
                for (index, line) in lyrics.line.iter().enumerate() {
                    let highlighted = lyrics.line_is_active(index, active);
                    // Sung lines recede most; upcoming lines dim with distance.
                    let rest = match active {
                        _ if !following => 0.86,
                        None => 0.55,
                        Some(current) if index < current => 0.3,
                        Some(current) => (0.6 - 0.06 * (index - current) as f32).max(0.34),
                    };
                    let alpha = transition(
                        SharedString::from(format!("lyric-alpha-{}-{highlight}-{index}", track.id)),
                        if highlighted { 1_f32 } else { rest },
                        Transition::new(Duration::from_millis(420)),
                        window,
                        cx,
                    )
                    .clamp(0., 1.);
                    // Lines dissolve toward the viewport edges instead of being clipped.
                    let edge = if following && height > 0. {
                        scroll.bounds_for_item(index).map_or(1., |item| {
                            let center = f32::from(item.center().y + offset_y - viewport.top());
                            let t = (center.min(height - center) / fade).clamp(0., 1.);
                            t * t * (3. - 2. * t)
                        })
                    } else {
                        1.
                    };
                    let shade = alpha * edge;
                    let empty = line.value.trim().is_empty();
                    let seek = is_playing.then(|| lyrics.line_position(index)).flatten();
                    let player = self.service.player.clone();
                    let mut row = div()
                        .id(SharedString::from(format!("lyric-{index}")))
                        .test_support()
                        .role(Role::Group)
                        .aria_label(format!(
                            "{}{}",
                            if highlighted { "Current lyric: " } else { "" },
                            if empty && highlighted && following {
                                "Instrumental"
                            } else {
                                line.value.as_str()
                            }
                        ))
                        .max_w(px(if highlight { 880. } else { 640. }))
                        .px(px(row_x))
                        .py(px(row_y))
                        .rounded_lg()
                        .text_size(px(text_size))
                        .line_height(px(leading))
                        .font_weight(weight)
                        .text_color(ink_color.opacity(shade));
                    row = if empty && highlighted && following {
                        row.child(lyric_gap(
                            index,
                            text_size,
                            leading,
                            ink_color.opacity(edge),
                        ))
                    } else if empty {
                        row.child(" ")
                    } else {
                        row.child(line.value.clone())
                    };
                    content = content.child(row.when_some(seek, |row, position| {
                        row.cursor_pointer()
                            .hover(|style| {
                                style
                                    .bg(ink_color.opacity(0.07 * edge))
                                    .text_color(ink_color.opacity(shade.max(0.8 * edge)))
                            })
                            .on_click(move |_, _, _| {
                                player.send(PlayerCommand::Seek(position));
                            })
                    }));
                }
                if !lyrics.is_timed() {
                    content = content.child(
                        div()
                            .mt_3()
                            .px(px(row_x))
                            .text_size(px(10.))
                            .text_color(colors.muted)
                            .child("Plain lyrics · no timing supplied"),
                    );
                }
            }
            value => {
                let retry = matches!(value, Some(Err(_)));
                let message = match value {
                    None => "Loading lyrics…".to_string(),
                    Some(Ok(_)) => "No lyrics supplied by the server.".to_string(),
                    Some(Err(error)) => error,
                };
                content = content.p_3().child(
                    div()
                        .px(px(row_x))
                        .text_size(px(if highlight { 15. } else { 12. }))
                        .text_color(colors.muted)
                        .child(message),
                );
                if retry {
                    let id = track.id.clone();
                    content = content.child(
                        Button::new("retry-lyrics")
                            .ghost()
                            .compact()
                            .label("Retry lyrics")
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.lyrics.remove(&id);
                                app.lyrics_requested.remove(&id);
                                app.lyrics_follow = None;
                                cx.notify();
                            })),
                    );
                }
            }
        }
        content.into_any_element()
    }

    /// The cover, track and album details, album tracks and Up Next.
    pub(super) fn now_playing_details(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(track) = self.playback.track.clone() else {
            return div().into_any_element();
        };
        let theme_dark = cx.theme().is_dark();
        let legible = self.backdrop_ink(&track.cover_art, theme_dark);
        // Colors chosen to read on this album's art; see `backdrop_ink`.
        let _scope = legible.map(|(ink, _)| InkScope::new(ink));
        let plates = legible.map_or([0.; 2], |(_, plates)| plates);
        let colors = ink(cx);
        if !track.album_id.is_empty() && self.highlight_requested.as_ref() != Some(&track.album_id)
        {
            self.highlight_requested = Some(track.album_id.clone());
            self.highlight_album = None;
            self.album_tracks_expanded = false;
            self.service.send(Command::Album(track.album_id.clone()));
        }
        let album_artists = track.album_artist_credits();
        self.request_popularity(&album_artists);
        let popular = self.popular_tracks(&track.album_id, &album_artists);
        let album_tracks = self
            .highlight_album
            .as_ref()
            .filter(|(id, _)| id == &track.album_id)
            .map(|(_, tracks)| tracks.clone())
            .unwrap_or_else(|| {
                self.playback
                    .queue
                    .iter()
                    .filter(|item| {
                        if track.album_id.is_empty() {
                            item.id == track.id
                        } else {
                            item.album_id == track.album_id
                        }
                    })
                    .cloned()
                    .collect()
            });
        let album_time = album_tracks.iter().map(|track| track.duration).sum::<f64>();

        let cover_side = (f32::from(window.viewport_size().width) * 0.3).clamp(260., 360.);
        let cover = match self.artwork.get(&track.cover_art) {
            Some(path) => img(path.clone())
                .image_cache(&self.image_cache)
                .size(px(cover_side))
                .rounded_xl()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => self.art(&track.cover_art, cover_side, cx),
        };

        let current = album_tracks.iter().position(|item| item.id == track.id);
        let collapsed = !self.album_tracks_expanded && album_tracks.len() > ALBUM_PREVIEW_ROWS;
        let visible = if collapsed {
            let start = current
                .unwrap_or(0)
                .saturating_sub(1)
                .min(album_tracks.len() - ALBUM_PREVIEW_ROWS);
            start..start + ALBUM_PREVIEW_ROWS
        } else {
            0..album_tracks.len()
        };
        let mut album_list = div().flex().flex_col().gap_1();
        for index in visible {
            let item = album_tracks[index].clone();
            album_list = album_list.child(self.track_row(
                item.clone(),
                item.track.unwrap_or(index as u32 + 1) as usize,
                format!("highlight-album-{index}"),
                None,
                Some(popular.contains(&item.id)),
                cx,
            ));
        }
        if album_tracks.len() > ALBUM_PREVIEW_ROWS {
            album_list = album_list.child(
                Button::new("album-tracks-toggle")
                    .ghost()
                    .small()
                    .text_color(colors.muted)
                    .label(if self.album_tracks_expanded {
                        "Show fewer".to_string()
                    } else {
                        format!("Show all {} tracks", album_tracks.len())
                    })
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.album_tracks_expanded = !app.album_tracks_expanded;
                        cx.notify();
                    })),
            );
        }

        let up_next = self.up_next(&album_tracks, cx);
        let hero = div()
            .id("now-playing-album")
            .w(px(cover_side + 64.))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .overflow_y_scroll()
            .px_8()
            .pt_8()
            .pb_4()
            .when(plates[0] > 0., |hero| {
                hero.bg(colors.background.opacity(plates[0]))
            })
            .child(
                div()
                    .size(px(cover_side))
                    .rounded_xl()
                    .shadow_2xl()
                    .child(cover),
            )
            .child(
                div()
                    .mt_6()
                    .text_size(px(24.))
                    .line_height(px(30.))
                    .font_weight(FontWeight::BOLD)
                    .child(track.title.clone()),
            )
            .child(
                div()
                    .mt_1()
                    .text_size(px(15.))
                    .text_color(colors.text.opacity(0.85))
                    .child(self.artist_links(
                        track.artist_credits(),
                        format!("now-playing-artist-{}", track.id),
                        cx,
                    )),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(colors.muted)
                    .child(
                        div()
                            .text_color(colors.text.opacity(0.8))
                            .child(track.album.clone()),
                    )
                    .child("·")
                    .child(
                        self.artist_link(
                            track.album_artist_label().to_string(),
                            track
                                .album_artists
                                .first()
                                .map_or(String::new(), |artist| artist.id.clone()),
                            format!("playing-album-artist-{}", track.id),
                            cx,
                        ),
                    ),
            )
            .child(
                div()
                    .mt_1()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child(self.year_link(track.year, format!("playing-year-{}", track.id), cx))
                    .child(self.genre_links(
                        &track.genres_label(),
                        format!("playing-genres-{}", track.id),
                        cx,
                    ))
                    .child(format!(
                        "{} tracks · {}",
                        album_tracks.len(),
                        duration_label(album_time)
                    )),
            )
            .child(
                div()
                    .mt_1()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child(technical(&track)),
            )
            .child(
                div()
                    .mt_6()
                    .mb_2()
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.muted)
                    .child("ON THIS ALBUM"),
            )
            .child(album_list)
            .children(up_next);
        hero.into_any_element()
    }

    /// The blurred cover behind Now Playing, crossfading between albums.
    pub(super) fn now_playing_backdrop(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(track) = self.playback.track.clone() else {
            return div().into_any_element();
        };
        let theme_dark = cx.theme().is_dark();
        let fade = self
            .backdrop_ink(&track.cover_art, theme_dark)
            .map_or(theme_ink(theme_dark).background, |(ink, _)| ink.background);
        self.backdrop(&track.cover_art, fade, window, cx)
    }

    /// Now Playing's synchronized lyrics.
    pub(super) fn now_playing_lyrics(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(track) = self.playback.track.clone() else {
            return div().into_any_element();
        };
        let legible = self.backdrop_ink(&track.cover_art, cx.theme().is_dark());
        let _scope = legible.map(|(ink, _)| InkScope::new(ink));
        let plate = legible.map_or(0., |(_, plates)| plates[1]);
        let colors = ink(cx);
        let lyrics = self.lyrics_content(track, true, window, cx);
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .pl_4()
            .pr_8()
            .pt_6()
            .when(plate > 0., |lyrics| {
                lyrics.bg(colors.background.opacity(plate))
            })
            .child(lyrics)
            .into_any_element()
    }

    /// The output spectrum along the bottom of Now Playing.
    pub(super) fn now_playing_spectrum(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(track) = self.playback.track.clone() else {
            return div().into_any_element();
        };
        let legible = self.backdrop_ink(&track.cover_art, cx.theme().is_dark());
        let _scope = legible.map(|(ink, _)| InkScope::new(ink));
        let palette = self
            .backdrops
            .get(&track.cover_art)
            .map(|backdrop| backdrop.palette);
        let dark = ink(cx).dark;
        self.spectrum_band(palette, dark, window, cx)
    }

    /// Arranges Now Playing's parts, each cached separately so the spectrum
    /// and lyric animations redraw only themselves.
    pub(super) fn now_playing_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(track) = self.playback.track.clone() else {
            return self.empty(
                "Play an album or track to see its artwork, details, and lyrics here.",
                cx,
            );
        };
        let theme_dark = cx.theme().is_dark();
        let legible = self.backdrop_ink(&track.cover_art, theme_dark);
        let accent = legible.map_or(theme_ink(theme_dark).accent, |(ink, _)| ink.accent);
        let cover_side = (f32::from(window.viewport_size().width) * 0.3).clamp(260., 360.);
        let spectrum = self.preferences.layout.show_spectrum.then(|| {
            self.part(
                Part::Spectrum,
                style()
                    .w_full()
                    .h(px(sections::SPECTRUM_BAND_HEIGHT))
                    .flex_shrink_0(),
                cx,
            )
        });
        div()
            .relative()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme_ink(theme_dark).background)
            .child(self.part(Part::NowBackdrop, style().absolute().inset_0(), cx))
            .child(
                div()
                    .relative()
                    .size_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when_some(
                        self.error.clone().or_else(|| self.playback.error.clone()),
                        |page, error| {
                            page.child(
                                div()
                                    .px_8()
                                    .py_2()
                                    .text_size(px(11.))
                                    .text_color(accent)
                                    .child(error),
                            )
                        },
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(self.part(
                                Part::NowDetails,
                                style().w(px(cover_side + 64.)).h_full().flex_shrink_0(),
                                cx,
                            ))
                            .child(self.part(
                                Part::NowLyrics,
                                style().flex_1().min_w_0().min_h_0().h_full(),
                                cx,
                            )),
                    )
                    .children(spectrum),
            )
            .into_any_element()
    }

    /// Queued tracks after the current one, unless everything left in the
    /// queue is from the playing album, which the album tracks already show.
    pub(super) fn up_next(
        &mut self,
        album_tracks: &[Arc<Track>],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let queue = self.playback.queue.clone();
        let start = self.playback.index.map_or(0, |index| index + 1);
        if start >= queue.len() {
            return None;
        }
        let album = self
            .playback
            .track
            .as_ref()
            .map(|track| track.album_id.as_str())
            .filter(|id| !id.is_empty());
        let from_album = |track: &Arc<Track>| {
            album.is_some_and(|id| track.album_id == id)
                || album_tracks.iter().any(|item| item.id == track.id)
        };
        if queue[start..].iter().all(from_album) {
            return None;
        }
        let colors = ink(cx);
        let upcoming = queue.len() - start;
        let shown = if self.up_next_expanded {
            upcoming
        } else {
            upcoming.min(UP_NEXT_PREVIEW_ROWS)
        };
        let mut list = div().flex().flex_col().gap_1();
        for index in start..start + shown {
            list = list.child(self.track_row(
                queue[index].clone(),
                index + 1,
                format!("now-up-next-{index}"),
                Some(index),
                None,
                cx,
            ));
        }
        if upcoming > UP_NEXT_PREVIEW_ROWS {
            list = list.child(
                Button::new("up-next-toggle")
                    .ghost()
                    .small()
                    .text_color(colors.muted)
                    .label(if self.up_next_expanded {
                        "Show fewer".to_string()
                    } else {
                        format!("Show all {upcoming} upcoming")
                    })
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.up_next_expanded = !app.up_next_expanded;
                        cx.notify();
                    })),
            );
        }
        Some(
            div()
                .child(
                    div()
                        .mt_6()
                        .mb_2()
                        .flex()
                        .justify_between()
                        .text_size(px(10.))
                        .text_color(colors.muted)
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("UP NEXT"))
                        .child(format!("{upcoming} in queue")),
                )
                .child(list)
                .into_any_element(),
        )
    }

    /// Text colors for Now Playing that read on this album's backdrop, and
    /// the plate behind each column; `None` until the backdrop is ready.
    pub(super) fn backdrop_ink(&self, cover: &str, theme_dark: bool) -> Option<(Ink, [f32; 2])> {
        use crate::backdrop::{Ink as Tone, contrast, legibility};
        let backdrop = self.backdrops.get(cover)?;
        let (light, dark) = (theme_ink(true), theme_ink(false));
        let tone = |ink: &Ink| Tone {
            text: luma_of(ink.text),
            background: luma_of(ink.background),
        };
        let choice = legibility(
            backdrop.tones,
            luma_of(theme_ink(theme_dark).background),
            scrim_alpha(theme_dark),
            tone(&light),
            tone(&dark),
        );
        let mut ink = if choice.light_text { light } else { dark };
        // Secondary colors must hold up where the text is hardest to read.
        let text = luma_of(ink.text);
        let under = if contrast(text, choice.worst[0]) <= contrast(text, choice.worst[1]) {
            choice.worst[0]
        } else {
            choice.worst[1]
        };
        ink.muted = readable_on(ink.muted, ink.text, under);
        ink.accent = readable_on(ink.accent, ink.text, under);
        Some((ink, choice.plates))
    }

    /// Blurred cover layers under a theme-colored scrim, crossfading on
    /// change, fading at the bottom toward `fade`.
    pub(super) fn backdrop(
        &mut self,
        cover: &str,
        fade: Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let dark = cx.theme().is_dark();
        if !self.backdrops.contains_key(cover)
            && self.artwork.contains_key(cover)
            && self.backdrop_requested.insert(cover.to_string())
        {
            self.service.send(Command::Backdrop(cover.to_string()));
        }
        let ready = self
            .backdrops
            .contains_key(cover)
            .then(|| cover.to_string());
        if ready != self.backdrop_shown {
            self.backdrop_previous = self.backdrop_shown.take();
            self.backdrop_shown = ready;
            self.backdrop_changed = Instant::now();
        }
        let progress = if cx.reduce_motion() {
            1.
        } else {
            (self.backdrop_changed.elapsed().as_secs_f32() / BACKDROP_FADE_SECONDS).min(1.)
        };
        if progress < 1. {
            window.request_animation_frame();
        } else {
            self.backdrop_previous = None;
        }
        let layer = |id: &Option<String>, opacity: f32| {
            id.as_ref()
                .and_then(|id| self.backdrops.get(id))
                .map(|backdrop| {
                    img(backdrop.path.clone())
                        .image_cache(&self.image_cache)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .opacity(opacity)
                })
        };
        let eased = 1. - (1. - progress).powi(3);
        div()
            .absolute()
            .inset_0()
            .children(layer(&self.backdrop_previous, 1.))
            .children(layer(&self.backdrop_shown, eased))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(colors.background.opacity(scrim_alpha(dark))),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(220.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(fade.opacity(0.), 0.),
                        linear_color_stop(fade.opacity(0.7), 1.),
                    )),
            )
            .into_any_element()
    }

    pub(super) fn spectrum_band(
        &mut self,
        palette: Option<[u32; 3]>,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let now = Instant::now();
        let dt = self
            .spectrum_tick
            .map_or(0., |tick| now.duration_since(tick).as_secs_f32());
        self.spectrum_tick = Some(now);
        let rate = self
            .service
            .player
            .spectrum_input(&mut self.spectrum_window);
        let moving = self
            .spectrum
            .update(rate.map(|rate| (&*self.spectrum_window, rate)), dt);
        if moving && window.is_visible() {
            self.request_frame_after(SPECTRUM_FRAME, window, cx);
        } else if !moving {
            self.spectrum_tick = None;
        }
        let (low, high) = spectrum_colors(palette, colors.accent, dark);
        let height = SPECTRUM_HEIGHT;
        let levels = *self.spectrum.levels();
        let peaks = *self.spectrum.peaks();
        // One canvas instead of ~100 positioned divs keeps per-frame layout cheap.
        let bars = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let gap = 3.;
                let width = f32::from(bounds.size.width);
                let bar =
                    ((width - gap * (SPECTRUM_BANDS - 1) as f32) / SPECTRUM_BANDS as f32).max(1.);
                let bottom = f32::from(bounds.bottom());
                for band in 0..SPECTRUM_BANDS {
                    let color = mix(low, high, band as f32 / (SPECTRUM_BANDS - 1) as f32);
                    let left = f32::from(bounds.left()) + band as f32 * (bar + gap);
                    let level = (levels[band] * height).max(2.);
                    window.paint_quad(
                        fill(
                            Bounds::new(
                                point(px(left), px(bottom - level)),
                                size(px(bar), px(level)),
                            ),
                            linear_gradient(
                                0.,
                                linear_color_stop(color.opacity(0.25), 0.),
                                linear_color_stop(color.opacity(0.95), 1.),
                            ),
                        )
                        .corner_radii(Corners {
                            top_left: px(2.),
                            top_right: px(2.),
                            bottom_left: px(0.),
                            bottom_right: px(0.),
                        }),
                    );
                    let peak = peaks[band];
                    if peak > 0.02 {
                        let top = bottom - peak * height - 4.;
                        window.paint_quad(
                            fill(
                                Bounds::new(point(px(left), px(top)), size(px(bar), px(2.))),
                                color.opacity(0.9),
                            )
                            .corner_radii(px(1.)),
                        );
                    }
                }
            },
        )
        .w_full()
        .h(px(height));
        let mut scale = div().relative().h(px(14.)).mt_1();
        for (hz, label) in [
            (60., "60"),
            (250., "250"),
            (1_000., "1k"),
            (4_000., "4k"),
            (10_000., "10k"),
        ] {
            scale = scale.child(
                div()
                    .absolute()
                    .left(relative(frequency_position(hz)))
                    .ml(px(-8.))
                    .w(px(16.))
                    .flex()
                    .justify_center()
                    .text_size(px(9.))
                    .text_color(colors.muted.opacity(0.8))
                    .child(label),
            );
        }
        div()
            .id("output-visualizer")
            .role(Role::Group)
            .aria_label("Audio spectrum")
            .flex_shrink_0()
            .relative()
            .px_8()
            .pt_2()
            .pb_3()
            .when(self.playback.buffering, |band| {
                band.child(
                    div()
                        .absolute()
                        .top_0()
                        .right(px(32.))
                        .text_size(px(10.))
                        .text_color(colors.muted)
                        .child("Buffering…"),
                )
            })
            .child(bars)
            .child(scale)
            .into_any_element()
    }
}
