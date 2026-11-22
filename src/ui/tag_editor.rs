//! The opt-in tag editor and the notice that offers to undo an edit.
use super::*;
use crate::tags::{EditorForm, MULTIPLE_VALUES, TagField, TagReport};

pub(super) struct TagEditor {
    form: EditorForm,
    inputs: Vec<(TagField, Entity<InputState>)>,
    saving: bool,
    failures: Vec<(String, String)>,
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
                let value = shared.clone().unwrap_or_default();
                let placeholder = if shared.is_some() {
                    ""
                } else {
                    MULTIPLE_VALUES
                };
                let input = cx.new(|cx| {
                    let mut input = InputState::new(window, cx).placeholder(placeholder);
                    input.set_value(value, window, cx);
                    input
                });
                (*field, input)
            })
            .collect();
        self.tag_editor = Some(TagEditor {
            form,
            inputs,
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

    fn save_tags(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.tag_editor.as_mut() else {
            return;
        };
        let entered: Vec<_> = editor
            .inputs
            .iter()
            .map(|(field, input)| (*field, input.read(cx).value().to_string()))
            .collect();
        let edit = editor.form.edit(&entered);
        if edit.fields.is_empty() {
            self.tag_editor = None;
        } else {
            editor.saving = true;
            editor.failures.clear();
            self.service.send(Command::WriteTags(edit));
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

    pub(super) fn tag_editor_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = self.tag_editor.as_ref()?;
        let colors = ink(cx);
        let count = editor.form.tracks.len();
        let title = if count == 1 {
            format!("Edit tags of {}", editor.form.tracks[0].title)
        } else {
            format!("Edit tags of {count} songs")
        };
        let entered: Vec<_> = editor
            .inputs
            .iter()
            .map(|(field, input)| (*field, input.read(cx).value().to_string()))
            .collect();
        let warns = editor.form.warns_ids(&editor.form.edit(&entered));
        let mut fields = div().grid().grid_cols(2).gap_x_3().gap_y_1p5();
        for (field, input) in &editor.inputs {
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
                        Input::new(input)
                            .small()
                            .id(format!("tag-{}", field.slug())),
                    ),
            );
        }
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
            .child(div().text_size(px(13.)).font_semibold().child(title))
            .child(div().text_size(px(11.)).text_color(colors.muted).child(
                "Writes to the files in your music folder. Fields you leave alone keep their \
                         values; separate several artists or genres with ;.",
            ))
            .child(fields);
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
                                .label(if retry { "Retry" } else { "Save to Files" })
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
