//! The opt-in tag editor, its MusicBrainz lookup, and the notice that offers
//! to undo an edit.
use super::*;
use crate::autotag::{self, Release, ReleaseCandidate, ReleaseQuery, TrackMatch};
use crate::tags::{Change, EditorForm, Entries, MULTIPLE_VALUES, TagField, TagReport};
use gpui_kit::component::checkbox::Checkbox;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct TagEditor {
    form: EditorForm,
    inputs: Vec<(TagField, Entity<InputState>)>,
    /// Per song, the table's inputs.
    cells: Vec<Vec<(TagField, Entity<InputState>)>>,
    cleared: BTreeSet<TagField>,
    /// Per song, lookup values with no input of their own.
    extra: Vec<BTreeMap<TagField, String>>,
    review: Option<Vec<Change>>,
    lookup: Lookup,
    candidates: Vec<ReleaseCandidate>,
    saving: bool,
    failures: Vec<(String, String)>,
}

enum Lookup {
    Idle,
    Searching(ReleaseQuery),
    Choosing,
    Loading(String),
    Matched {
        release: Arc<Release>,
        matches: Vec<TrackMatch>,
        /// Field groups the user unticked; `MusicBrainzTrack` stands for the IDs.
        skipped: BTreeSet<TagField>,
    },
    Failed(String),
}

/// The fields a lookup can fill, in the order the checkboxes show them.
const FILLED: [TagField; 8] = [
    TagField::Title,
    TagField::Artist,
    TagField::AlbumArtist,
    TagField::Album,
    TagField::Year,
    TagField::Track,
    TagField::Disc,
    TagField::Genre,
];

fn text_input(
    value: String,
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut Context<MusicApp>,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut input = InputState::new(window, cx).placeholder(placeholder);
        input.set_value(value, window, cx);
        input
    })
}

impl TagEditor {
    fn entries(&self, cx: &App) -> Entries {
        let read = |inputs: &[(TagField, Entity<InputState>)]| {
            inputs
                .iter()
                .map(|(field, input)| (*field, input.read(cx).value().to_string()))
                .collect::<Vec<_>>()
        };
        Entries {
            shared: read(&self.inputs),
            cleared: self.cleared.clone(),
            cells: self.cells.iter().map(|row| read(row)).collect(),
            extra: self.extra.clone(),
        }
    }
}

impl MusicApp {
    pub(super) fn open_tag_editor(
        &mut self,
        tracks: Vec<Track>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if tracks.is_empty() {
            return;
        }
        let form = EditorForm::new(tracks);
        let inputs = form
            .fields
            .iter()
            .map(|(field, shared)| {
                let placeholder = if shared.is_some() {
                    ""
                } else {
                    MULTIPLE_VALUES
                };
                let value = shared.clone().unwrap_or_default();
                (*field, text_input(value, placeholder, window, cx))
            })
            .collect();
        let cells = form
            .tracks
            .iter()
            .map(|track| {
                form.columns
                    .iter()
                    .map(|field| (*field, text_input(field.shown(track), "", window, cx)))
                    .collect()
            })
            .collect();
        self.tag_editor = Some(TagEditor {
            extra: vec![BTreeMap::new(); form.tracks.len()],
            form,
            inputs,
            cells,
            cleared: BTreeSet::new(),
            review: None,
            lookup: Lookup::Idle,
            candidates: vec![],
            saving: false,
            failures: vec![],
        });
        cx.notify();
    }

    /// Opens the editor on an album once the service has sent its songs.
    pub(super) fn open_album_tag_editor(&mut self, id: String) {
        self.tag_editor_album = Some(id.clone());
        self.service.send(Command::Album(id));
    }

    /// Shows what would change; a retry after failures writes straight away.
    fn save_tags(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.tag_editor.as_mut() else {
            return;
        };
        let entries = editor.entries(cx);
        if !editor.failures.is_empty() {
            self.write_tags(cx);
            return;
        }
        let changes = editor.form.changes(&entries);
        if changes.is_empty() {
            self.tag_editor = None;
        } else {
            editor.review = Some(changes);
        }
        cx.notify();
    }

    fn write_tags(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.tag_editor.as_mut() else {
            return;
        };
        let edits = editor.form.edits(&editor.entries(cx));
        editor.review = None;
        if edits.is_empty() {
            self.tag_editor = None;
        } else {
            editor.saving = true;
            editor.failures.clear();
            self.service.send(Command::WriteTags(edits));
        }
        cx.notify();
    }

    fn clear_tag(&mut self, field: TagField, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.tag_editor.as_mut() else {
            return;
        };
        editor.cleared.insert(field);
        if let Some((_, input)) = editor.inputs.iter().find(|(f, _)| *f == field) {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    fn find_releases(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.tag_editor.as_mut() else {
            return;
        };
        let query = ReleaseQuery::for_tracks(&editor.form.tracks);
        editor.lookup = Lookup::Searching(query.clone());
        self.service.send(Command::FindReleases(query));
        cx.notify();
    }

    pub(super) fn releases_found(
        &mut self,
        query: ReleaseQuery,
        result: Result<Vec<ReleaseCandidate>, String>,
    ) {
        let Some(editor) = self.tag_editor.as_mut() else {
            return;
        };
        if !matches!(&editor.lookup, Lookup::Searching(asked) if *asked == query) {
            return;
        }
        editor.lookup = match result {
            Ok(found) if found.is_empty() => Lookup::Failed(format!(
                "MusicBrainz found no release called {}.",
                query.album
            )),
            Ok(found) => {
                editor.candidates = found;
                Lookup::Choosing
            }
            Err(error) => Lookup::Failed(error),
        };
    }

    fn choose_release(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(editor) = self.tag_editor.as_mut() else {
            return;
        };
        editor.lookup = Lookup::Loading(id.clone());
        self.service.send(Command::LoadRelease(id));
        cx.notify();
    }

    pub(super) fn release_loaded(&mut self, id: String, result: Result<Arc<Release>, String>) {
        let Some(editor) = self.tag_editor.as_mut() else {
            return;
        };
        if !matches!(&editor.lookup, Lookup::Loading(asked) if *asked == id) {
            return;
        }
        editor.lookup = match result {
            Ok(release) => Lookup::Matched {
                matches: autotag::match_tracks(&editor.form.tracks, &release),
                release,
                skipped: BTreeSet::new(),
            },
            Err(error) => Lookup::Failed(error),
        };
    }

    fn toggle_filled(&mut self, field: TagField, cx: &mut Context<Self>) {
        if let Some(TagEditor {
            lookup: Lookup::Matched { skipped, .. },
            ..
        }) = self.tag_editor.as_mut()
            && !skipped.remove(&field)
        {
            skipped.insert(field);
        }
        cx.notify();
    }

    /// Puts the matched release's values into the form, for the user to check
    /// and save. Unmatched songs keep theirs.
    fn apply_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.tag_editor.as_mut() else {
            return;
        };
        let Lookup::Matched {
            release,
            matches,
            skipped,
        } = std::mem::replace(&mut editor.lookup, Lookup::Idle)
        else {
            return;
        };
        let proposals: Vec<Option<BTreeMap<TagField, String>>> = matches
            .iter()
            .map(|found| {
                let (index, _) = (*found)?;
                let mut values = autotag::proposed(&release, &release.tracks[index]);
                values.retain(|field, _| {
                    let group = if TagField::IDS.contains(field) {
                        TagField::MusicBrainzTrack
                    } else {
                        *field
                    };
                    !skipped.contains(&group)
                });
                Some(values)
            })
            .collect();
        let set = |input: &Entity<InputState>, value: &str, window: &mut Window, cx: &mut App| {
            input.update(cx, |input, cx| {
                input.set_value(value.to_string(), window, cx)
            });
        };
        for (field, input) in &editor.inputs {
            let values: Vec<Option<&String>> = proposals
                .iter()
                .map(|values| values.as_ref().and_then(|values| values.get(field)))
                .collect();
            let shared = values
                .first()
                .copied()
                .flatten()
                .filter(|first| values.iter().all(|value| value == &Some(*first)));
            if let Some(value) = shared {
                set(input, value, window, cx);
                editor.cleared.remove(field);
                for extra in &mut editor.extra {
                    extra.remove(field);
                }
            } else {
                for (row, value) in values.iter().enumerate() {
                    if let Some(value) = value {
                        editor.extra[row].insert(*field, (*value).clone());
                    }
                }
            }
        }
        for (row, values) in proposals.iter().enumerate() {
            let Some(values) = values else {
                continue;
            };
            for (field, input) in &editor.cells[row] {
                if let Some(value) = values.get(field) {
                    set(input, value, window, cx);
                }
            }
            for field in TagField::IDS {
                if let Some(value) = values.get(&field) {
                    editor.extra[row].insert(field, value.clone());
                }
            }
        }
        cx.notify();
    }

    pub(super) fn tags_written(&mut self, report: TagReport) {
        if let Some(editor) = self.tag_editor.as_mut().filter(|_| !report.undo) {
            editor.saving = false;
            if report.failures.is_empty() {
                self.tag_editor = None;
            } else {
                editor.failures = report.failures.clone();
            }
        } else if report.written == 0
            && let Some((_, reason)) = report.failures.first()
        {
            self.error = Some(reason.clone());
        }
        if report.written > 0 {
            self.tag_notice = Some(report);
        }
    }

    fn lookup_panel(&self, editor: &TagEditor, cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = ink(cx);
        let status = |text: String| {
            div()
                .id("lookup-status")
                .test_support()
                .role(Role::Status)
                .aria_label(text.clone())
                .text_size(px(11.))
                .text_color(colors.muted)
                .child(text)
                .into_any_element()
        };
        let panel = div().flex().flex_col().gap_1();
        Some(match &editor.lookup {
            Lookup::Idle => return None,
            Lookup::Searching(query) => {
                status(format!("Searching MusicBrainz for {}…", query.subject()))
            }
            Lookup::Loading(id) => {
                let title = editor
                    .candidates
                    .iter()
                    .find(|candidate| &candidate.id == id)
                    .map_or("the release", |candidate| candidate.title.as_str());
                status(format!("Loading the tracks of {title}…"))
            }
            Lookup::Failed(error) => div()
                .id("lookup-error")
                .test_support()
                .role(Role::Alert)
                .aria_label(error.clone())
                .text_size(px(11.))
                .text_color(cx.theme().danger)
                .child(error.clone())
                .into_any_element(),
            Lookup::Choosing => {
                let mut list = panel.child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.muted)
                        .child("Pick the edition you have:"),
                );
                for (index, candidate) in editor.candidates.iter().enumerate() {
                    let label = format!(
                        "{} — {} · {} · {}% match",
                        candidate.title,
                        candidate.artist,
                        candidate.summary(),
                        candidate.score
                    );
                    let id = candidate.id.clone();
                    list =
                        list.child(
                            div()
                                .id(("release-candidate", index))
                                .test_support()
                                .role(Role::Button)
                                .aria_label(label)
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|row| row.bg(colors.panel))
                                .flex()
                                .gap_2()
                                .text_size(px(11.))
                                .child(
                                    div().flex_1().min_w_0().truncate().child(format!(
                                        "{} — {}",
                                        candidate.title, candidate.artist
                                    )),
                                )
                                .child(div().text_color(colors.muted).child(candidate.summary()))
                                .child(
                                    div()
                                        .w(px(36.))
                                        .text_right()
                                        .child(format!("{}%", candidate.score)),
                                )
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.choose_release(id.clone(), cx)
                                })),
                        );
                }
                list.into_any_element()
            }
            Lookup::Matched {
                release,
                matches,
                skipped,
            } => {
                let matched = matches.iter().filter(|found| found.is_some()).count();
                let mut list = panel.child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.muted)
                        .child(format!(
                            "{matched} of {} songs match {}. Untick what you want to keep:",
                            matches.len(),
                            release.title
                        )),
                );
                for (row, found) in matches.iter().enumerate() {
                    let song = &editor.form.tracks[row].title;
                    let label = match found {
                        Some((index, score)) => format!(
                            "{song} → {} · {:.0}%",
                            release.tracks[*index].title,
                            score * 100.0
                        ),
                        None => format!("{song} · no match, left as it is"),
                    };
                    list = list.child(
                        div()
                            .id(("track-match", row))
                            .test_support()
                            .role(Role::Note)
                            .aria_label(label.clone())
                            .text_size(px(11.))
                            .when(found.is_none(), |row| row.text_color(colors.muted))
                            .child(label),
                    );
                }
                let mut boxes = div().flex().flex_wrap().gap_x_3().gap_y_1();
                let groups = FILLED
                    .iter()
                    .map(|field| (*field, field.label(), field.slug()))
                    .chain([(TagField::MusicBrainzTrack, "MusicBrainz IDs", "ids")]);
                for (field, label, slug) in groups {
                    let app = cx.entity().downgrade();
                    boxes = boxes.child(
                        Checkbox::new(SharedString::from(format!("fill-{slug}")))
                            .label(label)
                            .checked(!skipped.contains(&field))
                            .on_click(move |_, _, cx| {
                                let _ = app.update(cx, |app, cx| app.toggle_filled(field, cx));
                            }),
                    );
                }
                list.child(boxes)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("fill-tags")
                                    .small()
                                    .primary()
                                    .label("Fill In")
                                    .disabled(matched == 0)
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.apply_match(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("other-releases")
                                    .small()
                                    .label("Other Editions")
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        if let Some(editor) = app.tag_editor.as_mut() {
                                            editor.lookup = Lookup::Choosing;
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    .into_any_element()
            }
        })
    }

    fn shared_fields(&self, editor: &TagEditor, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let mut fields = div().grid().grid_cols(2).gap_x_3().gap_y_1p5();
        for (field, input) in &editor.inputs {
            let field = *field;
            fields = fields.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .child(field.label()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(input)
                                        .small()
                                        .id(format!("tag-{}", field.slug())),
                                ),
                            )
                            .child(
                                short_button(
                                    SharedString::from(format!("clear-tag-{}", field.slug())),
                                    Glyph::X,
                                    format!("Clear {}", field.label()),
                                )
                                .on_click(cx.listener(
                                    move |app, _, window, cx| app.clear_tag(field, window, cx),
                                )),
                            ),
                    ),
            );
        }
        fields.into_any_element()
    }

    fn track_table(&self, editor: &TagEditor, cx: &mut Context<Self>) -> Option<AnyElement> {
        if editor.form.columns.is_empty() {
            return None;
        }
        let colors = ink(cx);
        let width = |field: TagField, cell: Div| match field {
            TagField::Track | TagField::Disc => cell.w(px(48.)).flex_shrink_0(),
            _ => cell.flex_1().min_w_0(),
        };
        let mut header = div()
            .flex()
            .gap_1()
            .text_size(px(11.))
            .text_color(colors.muted);
        for field in &editor.form.columns {
            header = header.child(width(*field, div()).child(field.label()));
        }
        let mut table = div()
            .id("tag-table")
            .test_support()
            .role(Role::Table)
            .aria_label("Songs")
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(260.))
            .overflow_y_scroll()
            .child(header);
        for (row, cells) in editor.cells.iter().enumerate() {
            let mut line = div().flex().gap_1();
            for (field, input) in cells {
                line = line.child(
                    width(*field, div()).child(
                        Input::new(input)
                            .small()
                            .id(format!("tag-cell-{row}-{}", field.slug())),
                    ),
                );
            }
            table = table.child(line);
        }
        Some(table.into_any_element())
    }

    fn review_panel(&self, changes: &[Change], cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let files = changes
            .iter()
            .map(|change| &change.track)
            .collect::<BTreeSet<_>>()
            .len();
        let mut list = div()
            .id("tag-review")
            .test_support()
            .role(Role::List)
            .aria_label(format!("{} changes", changes.len()))
            .flex()
            .flex_col()
            .gap_0p5()
            .max_h(px(260.))
            .overflow_y_scroll()
            .text_size(px(11.));
        for (index, change) in changes.iter().enumerate() {
            let before = if change.before.is_empty() {
                "(empty)"
            } else {
                &change.before
            };
            let after = if change.after.is_empty() {
                "(empty)"
            } else {
                &change.after
            };
            let label = format!(
                "{} · {}: {before} → {after}",
                change.track,
                change.field.label()
            );
            list = list.child(
                div()
                    .id(("tag-change", index))
                    .test_support()
                    .role(Role::ListItem)
                    .aria_label(label)
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(160.))
                            .flex_shrink_0()
                            .truncate()
                            .child(change.track.clone()),
                    )
                    .child(
                        div()
                            .w(px(120.))
                            .flex_shrink_0()
                            .text_color(colors.muted)
                            .child(change.field.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(colors.muted)
                            .child(before.to_string()),
                    )
                    .child(div().child("→"))
                    .child(div().flex_1().min_w_0().truncate().child(after.to_string())),
            );
        }
        let files = if files == 1 {
            "Write 1 File".to_string()
        } else {
            format!("Write {files} Files")
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child("Check the changes before they are written:"),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("confirm-tags")
                            .small()
                            .primary()
                            .label(files)
                            .on_click(cx.listener(|app, _, _, cx| app.write_tags(cx))),
                    )
                    .child(
                        Button::new("back-tags")
                            .small()
                            .label("Back")
                            .on_click(cx.listener(|app, _, _, cx| {
                                if let Some(editor) = app.tag_editor.as_mut() {
                                    editor.review = None;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn tag_editor_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = self.tag_editor.as_ref()?;
        let colors = ink(cx);
        let count = editor.form.tracks.len();
        let title = if count == 1 {
            format!("Edit tags of {}", editor.form.tracks[0].title)
        } else {
            format!("Edit tags of {count} songs")
        };
        let entries = editor.entries(cx);
        let warns = editor.form.warns_ids(&editor.form.edits(&entries));
        let searching = matches!(editor.lookup, Lookup::Searching(_) | Lookup::Loading(_));
        let mut panel = div()
            .id("tag-editor")
            .test_support()
            .role(Role::Dialog)
            .aria_label(title.clone())
            .flex_shrink_0()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .font_semibold()
                            .child(title),
                    )
                    .when(
                        editor.review.is_none() && !ReleaseQuery::spans_albums(&editor.form.tracks),
                        |row| {
                            row.child(
                                Button::new("find-musicbrainz")
                                    .small()
                                    .label("Find on MusicBrainz")
                                    .loading(searching)
                                    .on_click(cx.listener(|app, _, _, cx| app.find_releases(cx))),
                            )
                        },
                    ),
            );
        if let Some(changes) = &editor.review {
            panel = panel.child(self.review_panel(changes, cx));
        } else {
            panel = panel
                .children(self.lookup_panel(editor, cx))
                .child(div().text_size(px(11.)).text_color(colors.muted).child(
                    "Writes to the files in your music folder. Fields you leave alone keep their \
                     values; separate several artists or genres with ;.",
                ))
                .child(self.shared_fields(editor, cx))
                .children(self.track_table(editor, cx));
        }
        if warns {
            panel = panel.child(
                div()
                    .id("tag-id-warning")
                    .test_support()
                    .role(Role::Note)
                    .aria_label("Navidrome may give these songs new IDs")
                    .text_size(px(11.))
                    .child(
                        "Some of these files have no MusicBrainz IDs, so Navidrome may give them new \
                         IDs after the rescan. Sonora carries favorites and ratings over.",
                    ),
            );
        }
        if !editor.failures.is_empty() {
            let summary = editor
                .failures
                .iter()
                .map(|(title, reason)| format!("{title}: {reason}"))
                .collect::<Vec<_>>()
                .join("\n");
            panel = panel.child(
                div()
                    .id("tag-failures")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(summary.clone())
                    .text_size(px(11.))
                    .text_color(cx.theme().danger)
                    .whitespace_normal()
                    .child(summary),
            );
        }
        if editor.review.is_some() {
            return Some(panel.into_any_element());
        }
        let retry = !editor.failures.is_empty();
        let saving = editor.saving;
        Some(
            panel
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("save-tags")
                                .small()
                                .primary()
                                .loading(saving)
                                .label(if retry { "Retry" } else { "Review Changes" })
                                .on_click(cx.listener(|app, _, _, cx| app.save_tags(cx))),
                        )
                        .child(Button::new("cancel-tags").small().label("Cancel").on_click(
                            cx.listener(|app, _, _, cx| {
                                app.tag_editor = None;
                                cx.notify();
                            }),
                        )),
                )
                .into_any_element(),
        )
    }

    pub(super) fn tag_notice_banner(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let report = self.tag_notice.as_ref()?;
        let files = if report.written == 1 { "file" } else { "files" };
        let mut text = if report.undo {
            format!("Put back the tags in {} {files}", report.written)
        } else {
            format!("Wrote tags to {} {files}", report.written)
        };
        if !report.failures.is_empty() {
            text.push_str(&format!(" · {} not changed", report.failures.len()));
        }
        Some(
            div()
                .id("tag-notice")
                .test_support()
                .role(Role::Status)
                .aria_label(text.clone())
                .w_full()
                .px_2()
                .py_2()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(11.))
                .bg(cx.theme().secondary)
                .child(div().flex_1().min_w_0().child(text))
                .child(
                    Button::new("undo-tags")
                        .small()
                        .label("Undo")
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.tag_notice = None;
                            app.service.send(Command::UndoTags);
                            cx.notify();
                        })),
                )
                .child(
                    short_button("dismiss-tag-notice", Glyph::X, "Dismiss").on_click(cx.listener(
                        |app, _, _, cx| {
                            app.tag_notice = None;
                            cx.notify();
                        },
                    )),
                )
                .into_any_element(),
        )
    }
}
