//! The toolbar Downloads button: present only while Soulseek downloads exist,
//! with a ring for overall progress and a popover listing each release.
use super::*;
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::progress::{Progress, ProgressCircle};

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
