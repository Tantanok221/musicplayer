//! The toolbar Downloads button: present only while Soulseek downloads exist,
//! with a ring for overall progress and a popover listing each release.
use super::*;
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::progress::{Progress, ProgressCircle};

fn size_label(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1e9 {
        format!("{:.2} GB", bytes / 1e9)
    } else if bytes >= 1e6 {
        format!("{:.0} MB", bytes / 1e6)
    } else {
        format!("{:.0} kB", bytes / 1e3)
    }
}

/// One row of the popover, from a download job or a click slskd has not answered.
#[derive(Clone)]
struct Row {
    release: String,
    title: String,
    artist_key: String,
    artist_name: String,
    caption: String,
    /// Percentage for the bar while files are moving.
    percent: Option<f32>,
    state: RowState,
}
#[derive(Clone, Copy, PartialEq)]
enum RowState {
    Starting,
    Active,
    Failed,
    Downloaded,
}

impl MusicApp {
    fn download_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(start) = self.download_start.as_ref().filter(|start| {
            start.failure.is_none() && !self.downloads.iter().any(|j| j.release == start.release)
        }) {
            rows.push(Row {
                release: start.release.clone(),
                title: start.title.clone(),
                artist_key: start.artist_key.clone(),
                artist_name: start.artist_name.clone(),
                caption: format!("Starting · asking {} for the folder", start.username),
                percent: None,
                state: RowState::Starting,
            });
        }
        for job in self.downloads.iter() {
            let state = match job.state {
                JobState::Active => RowState::Active,
                JobState::Failed => RowState::Failed,
                JobState::Downloaded => RowState::Downloaded,
            };
            let caption = match state {
                RowState::Downloaded => "Downloaded · waiting for Navidrome to scan".to_string(),
                _ => format!("{} · from {}", download_caption(job), job.username),
            };
            rows.push(Row {
                release: job.release.clone(),
                title: job.title.clone(),
                artist_key: job.artist.clone(),
                artist_name: job.artist_name.clone(),
                caption,
                percent: (state == RowState::Active && job.progress.total > 0)
                    .then(|| job.progress.done as f32 * 100. / job.progress.total as f32),
                state,
            });
        }
        // Work in progress first, then what needs attention, then what is done.
        rows.sort_by_key(|row| match row.state {
            RowState::Starting | RowState::Active => 0,
            RowState::Failed => 1,
            RowState::Downloaded => 2,
        });
        rows
    }

    /// Opens the artist page with the release's Soulseek panel showing.
    pub(super) fn show_download(
        &mut self,
        artist_key: &str,
        artist_name: &str,
        release: &str,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (folded, id) = artist_key.split_once('\n').unwrap_or((artist_key, ""));
        let name = if artist_name.is_empty() {
            folded
        } else {
            artist_name
        };
        self.open_artist(name.to_string(), id.to_string(), window, cx);
        self.selected_missing = Some(release.to_string());
        let query = format!("{name} {title}");
        self.soulseek_query
            .update(cx, |input, cx| input.set_value(query, window, cx));
        cx.notify();
    }

    /// The toolbar button, or nothing when there are no downloads to show.
    pub(super) fn downloads_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rows = self.download_rows();
        if rows.is_empty() {
            return None;
        }
        let colors = ink(cx);
        let moving = rows
            .iter()
            .filter(|row| matches!(row.state, RowState::Starting | RowState::Active))
            .count();
        let failed = rows.iter().any(|row| row.state == RowState::Failed);
        let (done, total) = self
            .downloads
            .iter()
            .filter(|job| job.state == JobState::Active)
            .fold((0, 0), |(done, total), job| {
                (done + job.progress.done, total + job.progress.total)
            });
        let label = match (moving, failed) {
            (0, true) => "Downloads · a download needs attention".to_string(),
            (0, false) => "Downloads · finished".to_string(),
            (n, _) => format!("Downloads · {n} in progress"),
        };
        let indicator = if moving > 0 {
            ProgressCircle::new("downloads-progress")
                .xsmall()
                .color(colors.accent)
                .loading(total == 0)
                .value(if total == 0 {
                    0.
                } else {
                    done as f32 * 100. / total as f32
                })
                .into_any_element()
        } else {
            Icon::new(Glyph::ArrowDownToLine)
                .size(px(14.))
                .into_any_element()
        };
        let trigger = Button::new("downloads-button")
            .ghost()
            .small()
            .tooltip(label.clone())
            .accessibility_label(label)
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(indicator)
                    .when(moving > 1, |button| {
                        button.child(div().text_size(px(10.)).child(moving.to_string()))
                    })
                    .when(failed && moving == 0, |button| {
                        button.child(
                            div()
                                .absolute()
                                .top(px(-2.))
                                .right(px(-3.))
                                .size(px(6.))
                                .rounded_full()
                                .bg(colors.accent),
                        )
                    }),
            );
        let app = cx.entity();
        Some(
            Popover::new("downloads-popover")
                .anchor(Anchor::TopRight)
                .trigger(trigger)
                .content(move |_, _, cx| {
                    let popover = cx.entity();
                    downloads_panel(&app, &popover, cx)
                })
                .into_any_element(),
        )
    }
}

fn downloads_panel(
    app: &Entity<MusicApp>,
    popover: &Entity<PopoverState>,
    cx: &mut App,
) -> AnyElement {
    let colors = ink(cx);
    let rows = app.read(cx).download_rows();
    let mut list = div()
        .id("downloads-list")
        .w(px(340.))
        .max_h(px(420.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .px_1()
                .pb_1()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Downloads"),
        );
    if rows.is_empty() {
        list = list.child(
            div()
                .px_1()
                .text_size(px(11.))
                .text_color(colors.muted)
                .child("No downloads."),
        );
    }
    for (index, row) in rows.into_iter().enumerate() {
        let show = {
            let (app, popover, row) = (app.clone(), popover.clone(), row.clone());
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                popover.update(cx, |state, cx| state.dismiss(window, cx));
                app.update(cx, |app, cx| {
                    app.show_download(
                        &row.artist_key,
                        &row.artist_name,
                        &row.release,
                        &row.title,
                        window,
                        cx,
                    )
                });
            }
        };
        let dismiss = {
            let (app, release) = (app.clone(), row.release.clone());
            move |cx: &mut App| {
                app.update(cx, |app, cx| {
                    app.service.send(Command::DismissDownload(release.clone()));
                    cx.notify();
                })
            }
        };
        let artist = if row.artist_name.is_empty() {
            row.artist_key
                .split_once('\n')
                .map_or(row.artist_key.clone(), |(name, _)| name.to_string())
        } else {
            row.artist_name.clone()
        };
        let mut actions = div().flex().items_center().gap_1().flex_shrink_0();
        match row.state {
            RowState::Starting | RowState::Active => {
                actions = actions.child(
                    Button::new(("download-show", index))
                        .ghost()
                        .xsmall()
                        .label("Show")
                        .on_click(show),
                );
            }
            RowState::Failed => {
                actions = actions.child(
                    Button::new(("download-retry", index))
                        .ghost()
                        .xsmall()
                        .label("Pick another folder")
                        .on_click(move |event, window, cx| {
                            dismiss(cx);
                            show(event, window, cx);
                        }),
                );
            }
            RowState::Downloaded => {
                let refresh = app.clone();
                actions = actions
                    .child(
                        Button::new(("download-refresh", index))
                            .ghost()
                            .xsmall()
                            .label("Refresh library")
                            .on_click(move |_, _, cx| {
                                refresh.read(cx).service.send(Command::Refresh);
                            }),
                    )
                    .child(
                        Button::new(("download-clear", index))
                            .ghost()
                            .xsmall()
                            .label("Clear")
                            .on_click(move |_, _, cx| dismiss(cx)),
                    );
            }
        }
        list = list.child(
            div()
                .id(SharedString::from(format!(
                    "download-release-{}",
                    row.release
                )))
                .test_support()
                .role(Role::Group)
                .aria_label(format!("{}: {}", row.title, row.caption))
                .px_1()
                .py_1()
                .flex()
                .flex_col()
                .gap_1()
                .border_t_1()
                .border_color(colors.line)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .truncate()
                                        .child(row.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(colors.muted)
                                        .truncate()
                                        .child(artist),
                                ),
                        )
                        .child(actions),
                )
                .when_some(row.percent, |item, percent| {
                    item.child(Progress::new(("download-bar", index)).value(percent))
                })
                .when(row.state == RowState::Starting, |item| {
                    item.child(Progress::new(("download-bar", index)).loading(true))
                })
                .child(
                    div()
                        .text_size(px(10.))
                        .when(row.state == RowState::Failed, |caption| {
                            caption.text_color(colors.accent)
                        })
                        .when(row.state != RowState::Failed, |caption| {
                            caption.text_color(colors.muted)
                        })
                        .child(row.caption),
                ),
        );
    }
    list.into_any_element()
}

impl MusicApp {
    pub(super) fn request_release_details(&mut self, group: &str) {
        if self.release_details_requested.insert(group.into()) {
            self.service.send(Command::ReleaseDetails(group.into()));
        }
    }

    /// Releases the artist has published that the library lacks, dimmed like
    /// similar artists that are not in the library.
    pub(super) fn missing_releases(
        &mut self,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let colors = ink(cx);
        let view = self
            .discographies
            .get(&cache_key(name, &self.artist_id))
            .cloned()?;
        let all = self.preferences.all_missing_releases;
        let downloads = self.downloads.clone();
        let job = |release: &MissingRelease| downloads.iter().any(|job| job.release == release.id);
        // A release being downloaded is listed first, whatever its type.
        let mut shown = view
            .missing
            .iter()
            .filter(|release| all || release.core || job(release))
            .cloned()
            .collect::<Vec<_>>();
        shown.sort_by_key(|release| !job(release));
        let others = view.missing.len() - shown.len();
        if shown.is_empty() && others == 0 {
            return view.loading.then(|| {
                div()
                    .mt_5()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child("Checking discography…")
                    .into_any_element()
            });
        }
        let (columns, side) = cover_layout((self.library_width - 8.).max(0.));
        let limit = if self.missing_expanded {
            shown.len()
        } else {
            columns * 2
        };
        let mut section = div().mt_5().min_w_0().child(
            div()
                .mb_3()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Missing from your library ({})", shown.len())),
                )
                .when(all || others > 0, |header| {
                    header.child(
                        Button::new("missing-release-types")
                            .ghost()
                            .small()
                            .label(if all {
                                "Albums and EPs only".to_string()
                            } else {
                                format!("Show singles, live, compilations… ({others})")
                            })
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.preferences.all_missing_releases =
                                    !app.preferences.all_missing_releases;
                                app.save_preferences();
                                cx.notify();
                            })),
                    )
                }),
        );
        let mut tiles: Vec<(String, AnyElement)> = Vec::new();
        for release in shown.iter().take(limit) {
            let selected = self.selected_missing.as_ref() == Some(&release.id);
            let job = self
                .downloads
                .iter()
                .find(|job| job.release == release.id)
                .cloned();
            let caption = match &job {
                Some(job) => download_caption(job),
                None => "Not in your library".into(),
            };
            let year = release
                .year
                .map_or("Year unknown".to_string(), |year| year.to_string());
            let open = release.clone();
            let artist = name.to_string();
            let art = self.art(&release.cover, side, cx);
            tiles.push((
                release.id.clone(),
                div()
                    .id(SharedString::from(format!(
                        "missing-release-{}",
                        release.id
                    )))
                    .test_support()
                    .role(Role::Button)
                    .aria_label(format!("Missing release {}", release.title))
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
                    .text_color(colors.muted)
                    .child(div().opacity(0.4).child(art))
                    .child(
                        div()
                            .mt_1()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(release.title.clone()),
                    )
                    .child(div().text_size(px(10.)).truncate().child(format!(
                        "{} · {}",
                        if release.upcoming {
                            "Upcoming".to_string()
                        } else {
                            year
                        },
                        release.kind
                    )))
                    .child(div().text_size(px(9.)).truncate().child(caption))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        if app.selected_missing.as_ref() == Some(&open.id) {
                            app.selected_missing = None;
                        } else {
                            // A lookup that failed earlier is tried again on reopening.
                            if matches!(app.release_details.get(&open.id), Some(Err(_))) {
                                app.release_details.remove(&open.id);
                                app.release_details_requested.remove(&open.id);
                            }
                            app.selected_missing = Some(open.id.clone());
                            app.selected_album = None;
                            let query = format!("{artist} {}", open.title);
                            app.soulseek_query
                                .update(cx, |input, cx| input.set_value(query, window, cx));
                        }
                        app.soulseek = None;
                        app.soulseek_open = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            ));
        }
        // The panel opens under the row holding the picked cover, or after the
        // last row when that cover is folded away.
        let selected = self
            .selected_missing
            .as_ref()
            .and_then(|id| shown.iter().find(|release| &release.id == id))
            .cloned();
        let rows = tiles.len().div_ceil(columns.max(1));
        let panel_row = selected.as_ref().map(|release| {
            tiles
                .iter()
                .position(|(id, _)| id == &release.id)
                .map_or(rows.saturating_sub(1), |index| index / columns.max(1))
        });
        let mut panel = selected.map(|release| {
            div()
                .py_3()
                .border_y_1()
                .border_color(colors.line)
                .child(self.missing_panel(name, release, cx))
                .into_any_element()
        });
        let mut grid = div().flex().flex_col().gap_4().min_w_0();
        let mut tiles = tiles.into_iter();
        for row in 0..rows {
            grid = grid.child(
                div()
                    .flex()
                    .gap_4()
                    .min_w_0()
                    .children(tiles.by_ref().take(columns.max(1)).map(|(_, tile)| tile)),
            );
            if panel_row == Some(row) {
                grid = grid.children(panel.take());
            }
        }
        section = section.child(grid);
        if shown.len() > columns * 2 {
            section = section.child(
                Button::new("missing-release-more")
                    .ghost()
                    .small()
                    .mt_2()
                    .label(if self.missing_expanded {
                        "Show fewer".to_string()
                    } else {
                        format!("Show all {}", shown.len())
                    })
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.missing_expanded = !app.missing_expanded;
                        cx.notify();
                    })),
            );
        }
        Some(section.into_any_element())
    }

    /// One missing release laid out like a library album: cover, details and
    /// track list, places to listen to it, then the Soulseek download.
    pub(super) fn missing_panel(
        &mut self,
        artist: &str,
        release: MissingRelease,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.request_release_details(&release.id);
        let colors = ink(cx);
        let details = self.release_details.get(&release.id).cloned();
        let loaded = details
            .as_ref()
            .and_then(|details| details.as_ref().ok())
            .cloned();
        let hide = (
            artist.to_string(),
            self.artist_id.clone(),
            release.id.clone(),
        );
        let page = format!("https://musicbrainz.org/release-group/{}", release.id);
        let note = |text: String| {
            div()
                .text_size(px(11.))
                .text_color(colors.muted)
                .child(text)
        };
        let mut facts = vec![
            release.kind.clone(),
            match (release.upcoming, release.year) {
                (true, Some(year)) => format!("Upcoming, {year}"),
                (_, Some(year)) => year.to_string(),
                _ => "Year unknown".into(),
            },
        ];
        if let Some(details) = &loaded {
            match (details.label.is_empty(), details.catalog_number.is_empty()) {
                (false, false) => {
                    facts.push(format!("{} ({})", details.label, details.catalog_number))
                }
                (false, true) => facts.push(details.label.clone()),
                _ => {}
            }
            if !details.country.is_empty() && details.country != "XW" {
                facts.push(details.country.clone());
            }
            if !details.tracks.is_empty() {
                let count = details.tracks.len();
                facts.push(format!(
                    "{count} {}",
                    if count == 1 { "track" } else { "tracks" }
                ));
            }
            if details.seconds() > 0 {
                facts.push(duration_label(details.seconds() as f64));
            }
        }
        let mut right = div()
            .flex_1()
            .min_w_0()
            .when(self.library_width < 520., |column| column.w_full())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .min_h(px(24.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(release.title.clone()),
                    )
                    .child(
                        Button::new("missing-open-musicbrainz")
                            .ghost()
                            .small()
                            .label("Open on MusicBrainz")
                            .on_click(move |_, _, cx| cx.open_url(&page)),
                    )
                    .child(
                        Button::new("missing-hide")
                            .ghost()
                            .small()
                            .label("Hide")
                            .tooltip("Stop listing this release as missing")
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.selected_missing = None;
                                app.service.send(Command::HideRelease(
                                    hide.0.clone(),
                                    hide.1.clone(),
                                    hide.2.clone(),
                                ));
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child(facts.join(" · ")),
            )
            .when_some(
                loaded.as_ref().filter(|details| !details.genres.is_empty()),
                |column, details| {
                    column.child(
                        div()
                            .text_size(px(10.))
                            .text_color(colors.muted)
                            .truncate()
                            .child(details.genres.join(", ")),
                    )
                },
            );
        match &details {
            None => {
                right = right.child(
                    div()
                        .h(px(60.))
                        .pt_2()
                        .child(note("Loading album details…".into())),
                );
            }
            Some(Err(error)) => {
                let message = format!("Could not load the track list: {error}");
                right = right.child(
                    div()
                        .id("release-details-error")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(message.clone())
                        .pt_2()
                        .child(note(message)),
                );
            }
            Some(Ok(details)) if details.tracks.is_empty() => {
                right = right.child(div().pt_2().child(note(
                    "MusicBrainz has no track list for this release.".into(),
                )));
            }
            Some(Ok(details)) => {
                let discs = details
                    .tracks
                    .iter()
                    .map(|track| track.disc)
                    .collect::<HashSet<_>>();
                let mut last_disc = None;
                for track in &details.tracks {
                    if discs.len() > 1 && last_disc != Some(track.disc) {
                        last_disc = Some(track.disc);
                        right = right.child(
                            div()
                                .h(px(23.))
                                .pt_1()
                                .text_size(px(10.))
                                .text_color(colors.muted)
                                .child(format!("Disc {}", track.disc)),
                        );
                    }
                    right = right.child(
                        div()
                            .id(SharedString::from(format!(
                                "release-detail-track-{}-{}-{}",
                                release.id, track.disc, track.number
                            )))
                            .test_support()
                            .role(Role::Group)
                            .aria_label(track.title.clone())
                            .w_full()
                            .min_w_0()
                            .h(px(27.))
                            .px_1()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(11.))
                            .child(
                                div()
                                    .w(px(36.))
                                    .flex_shrink_0()
                                    .text_color(colors.muted)
                                    .truncate()
                                    .child(track.number.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_shrink(1.)
                                            .truncate()
                                            .child(track.title.clone()),
                                    )
                                    .when(!track.artist.is_empty(), |title| {
                                        title.child(
                                            div()
                                                .min_w_0()
                                                .flex_1()
                                                .truncate()
                                                .text_color(colors.muted)
                                                .child(format!("· {}", track.artist)),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(38.))
                                    .flex_shrink_0()
                                    .text_color(colors.muted)
                                    .text_right()
                                    .child(
                                        track
                                            .seconds
                                            .map(|seconds| duration_label(seconds as f64))
                                            .unwrap_or_default(),
                                    ),
                            ),
                    );
                }
            }
        }
        if !release.upcoming {
            let mut links = div().flex().flex_wrap().items_center().gap_1();
            for (index, link) in listen_links(loaded.as_deref(), artist, &release.title)
                .into_iter()
                .enumerate()
            {
                let url = link.url.clone();
                links = links.child(
                    Button::new(("missing-listen", index))
                        .ghost()
                        .small()
                        .label(if link.direct {
                            link.service.clone()
                        } else {
                            format!("Search {}", link.service)
                        })
                        .tooltip(if link.direct {
                            format!("Open this release on {}", link.service)
                        } else {
                            format!("Search {} for this release", link.service)
                        })
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            right = right.child(
                div()
                    .mt_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.muted)
                            .child("Listen on"),
                    )
                    .child(links),
            );
        }
        right = right.child(
            div()
                .mt_3()
                .pt_2()
                .border_t_1()
                .border_color(colors.line)
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.muted)
                        .child("Download from Soulseek"),
                )
                .child(self.soulseek_block(artist, release.clone(), cx)),
        );
        div()
            .w_full()
            .min_w_0()
            .px_3()
            .flex()
            .gap_3()
            .items_start()
            .when(self.library_width < 520., |row| row.flex_col())
            .child(
                div()
                    .w(px(124.))
                    .flex_shrink_0()
                    .child(self.art(&release.cover, 124., cx)),
            )
            .child(right)
            .into_any_element()
    }

    /// The Soulseek half of a missing release: its download, or a search for
    /// the folders peers offer when slskd is linked.
    pub(super) fn soulseek_block(
        &mut self,
        artist: &str,
        release: MissingRelease,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let job = self
            .downloads
            .iter()
            .find(|job| job.release == release.id)
            .cloned();
        let mut block = div().flex().flex_col().gap_2().min_w_0();
        let note = |text: String| {
            div()
                .text_size(px(11.))
                .text_color(colors.muted)
                .child(text)
        };
        if release.upcoming {
            return block
                .child(note("This release is not out yet.".into()))
                .into_any_element();
        }
        if !self.preferences.downloads.enabled {
            return block
                .child(note(
                    "Link slskd in Settings → Downloads to look for this release on Soulseek."
                        .into(),
                ))
                .into_any_element();
        }
        if let Some(job) = job {
            let caption = format!("{} · from {}", download_caption(&job), job.username);
            let dismiss = job.release.clone();
            return block
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            note(caption.clone())
                                .id("missing-download-status")
                                .test_support()
                                .role(Role::Status)
                                .aria_label(caption),
                        )
                        .when(job.state != JobState::Active, |row| {
                            row.child(
                                Button::new("missing-download-dismiss")
                                    .ghost()
                                    .small()
                                    .label(if job.state == JobState::Failed {
                                        "Pick another folder"
                                    } else {
                                        "Clear"
                                    })
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.service.send(Command::DismissDownload(dismiss.clone()));
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .into_any_element();
        }
        let search = self
            .soulseek
            .as_ref()
            .filter(|(id, _)| id == &release.id)
            .map(|(_, update)| update.clone());
        let searching = matches!(search, Some(SearchUpdate::Searching(_)));
        let start = self
            .download_start
            .as_ref()
            .filter(|start| start.release == release.id);
        let starting = start.is_some_and(|start| start.failure.is_none());
        let id = release.id.clone();
        block = block.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div().flex_1().max_w(px(420.)).child(
                        Input::new(&self.soulseek_query)
                            .id("soulseek-query")
                            .small(),
                    ),
                )
                .child(
                    Button::new("missing-search")
                        .primary()
                        .small()
                        .label(if search.is_some() {
                            "Search again"
                        } else {
                            "Find on Soulseek"
                        })
                        .disabled(searching || starting)
                        .on_click(cx.listener(move |app, _, _, cx| {
                            let query = app.soulseek_query.read(cx).value().to_string();
                            app.soulseek = Some((id.clone(), SearchUpdate::Searching(0)));
                            app.soulseek_open = None;
                            app.download_start = None;
                            app.service.send(Command::SoulseekSearch(id.clone(), query));
                            cx.notify();
                        })),
                ),
        );
        if let Some(start) = start {
            let failed = start.failure.is_some();
            let tint = colors.accent;
            let message = match &start.failure {
                Some(reason) => format!(
                    "Download not started. {reason} Pick another folder below, or try again."
                ),
                None => format!(
                    "Starting download from {}… asking for the full folder and adding it to the queue. This can take up to a minute.",
                    start.username
                ),
            };
            block = block.child(
                div()
                    .id("download-start")
                    .test_support()
                    .aria_label(message.clone())
                    .role(if failed { Role::Alert } else { Role::Status })
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .border_l_2()
                    .border_color(tint)
                    .bg(colors.panel)
                    .text_size(px(11.))
                    .child(
                        Icon::new(if failed {
                            Glyph::CircleAlert
                        } else {
                            Glyph::LoaderCircle
                        })
                        .size(px(13.))
                        .flex_shrink_0()
                        .text_color(tint),
                    )
                    .child(div().flex_1().min_w_0().child(message)),
            );
        }
        let search_note = |text: String| {
            note(text.clone())
                .id("soulseek-search-state")
                .test_support()
                .role(Role::Status)
                .aria_label(text)
        };
        match search {
            None => {}
            Some(SearchUpdate::Searching(count)) => {
                block = block.child(search_note(format!(
                    "Searching Soulseek… {count} {} so far",
                    if count == 1 { "response" } else { "responses" }
                )));
            }
            Some(SearchUpdate::Failed(message)) => block = block.child(search_note(message)),
            Some(SearchUpdate::Results(candidates, expected)) => {
                if candidates.is_empty() {
                    block = block.child(search_note(
                        "No folders found. The Soulseek server hides some names; try different search text."
                            .into(),
                    ));
                } else {
                    block = block.child(search_note(format!(
                        "{} {} · best matches first{}",
                        candidates.len(),
                        if candidates.len() == 1 {
                            "folder"
                        } else {
                            "folders"
                        },
                        expected.map_or(String::new(), |tracks| format!(
                            " · the release has {tracks} tracks"
                        ))
                    )));
                    for (index, candidate) in candidates.iter().take(40).enumerate() {
                        block = block.child(
                            self.candidate_row(artist, &release, index, candidate, expected, cx),
                        );
                    }
                }
            }
        }
        block.into_any_element()
    }

    pub(super) fn candidate_row(
        &self,
        artist: &str,
        release: &MissingRelease,
        index: usize,
        candidate: &Candidate,
        expected: Option<u32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let open = self.soulseek_open == Some(index);
        let tracks = match (candidate.completeness, expected) {
            (Completeness::Unknown, _) | (_, None) => format!("{} tracks", candidate.tracks),
            (_, Some(expected)) => format!("{}/{expected} tracks", candidate.tracks),
        };
        let mut facts = vec![
            candidate.username.clone(),
            tracks,
            candidate.format.clone(),
            size_label(candidate.size),
        ];
        if candidate.seconds > 0 {
            facts.push(duration_label(candidate.seconds as f64));
        }
        facts.push(if candidate.free_slot {
            "free slot".into()
        } else {
            format!("queue {}", candidate.queue)
        });
        if candidate.speed > 0 {
            facts.push(format!("{}/s", size_label(candidate.speed)));
        }
        if candidate.locked > 0 {
            facts.push(format!("{} locked files left out", candidate.locked));
        }
        let start = self
            .download_start
            .as_ref()
            .filter(|start| start.release == release.id && start.failure.is_none());
        let starting = start.is_some();
        let clicked = start.is_some_and(|start| start.candidate == index);
        let request = DownloadRequest {
            artist: artist.to_string(),
            artist_id: self.artist_id.clone(),
            release: release.id.clone(),
            title: release.title.clone(),
            expected,
            candidate: candidate.clone(),
        };
        let mut row = div()
            .id(("soulseek-candidate", index))
            .px_2()
            .py_1()
            .min_w_0()
            .rounded_sm()
            .hover(|row| row.bg(colors.panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id(("soulseek-candidate-files", index))
                            .test_support()
                            .role(Role::Button)
                            .aria_label(format!("Show files in {}", candidate.folder()))
                            .flex_1()
                            .min_w_0()
                            .cursor_pointer()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .truncate()
                                    .when(candidate.completeness == Completeness::Partial, |name| {
                                        name.text_color(colors.muted)
                                    })
                                    .child(candidate.folder().to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(colors.muted)
                                    .truncate()
                                    .child(facts.join(" · ")),
                            )
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.soulseek_open =
                                    (app.soulseek_open != Some(index)).then_some(index);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("soulseek-download", index))
                            .small()
                            .label(if clicked { "Starting…" } else { "Download" })
                            .loading(clicked)
                            .disabled(starting && !clicked)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if app
                                    .download_start
                                    .as_ref()
                                    .is_some_and(|start| start.failure.is_none())
                                {
                                    return;
                                }
                                // Keep the results until slskd accepts, so a refusal can pick another.
                                app.download_start = Some(DownloadStart {
                                    release: request.release.clone(),
                                    candidate: index,
                                    username: request.candidate.username.clone(),
                                    title: request.title.clone(),
                                    artist_key: cache_key(&request.artist, &request.artist_id),
                                    artist_name: request.artist.clone(),
                                    failure: None,
                                });
                                app.service
                                    .send(Command::Download(Box::new(request.clone())));
                                cx.notify();
                            })),
                    ),
            );
        if open {
            let mut files = div()
                .mt_1()
                .pl_3()
                .text_size(px(10.))
                .text_color(colors.muted);
            for file in &candidate.files {
                files = files.child(div().truncate().child(format!(
                    "{} · {}",
                    file.name(),
                    size_label(file.size)
                )));
            }
            row = row.child(files);
        }
        row.into_any_element()
    }

    /// The unfinished Soulseek download that this library album is part of, if any.
    pub(super) fn album_download(&self, album: &Album) -> Option<&DownloadJob> {
        let title = crate::discography::fold(&album.name);
        let artists = album
            .artist_credits()
            .iter()
            .map(|credit| format!("{}\n", normalize(&credit.name)))
            .collect::<Vec<_>>();
        self.downloads.iter().find(|job| {
            job.state != JobState::Downloaded
                && crate::discography::fold(&job.title) == title
                && artists.iter().any(|artist| job.artist.starts_with(artist))
        })
    }
}
