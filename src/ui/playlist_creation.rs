use super::*;
use crate::playlists::{Creation, CreationSource};
use gpui_kit::base::Dialog;

pub(super) struct CreationDialog {
    draft: Creation,
    previous_focus: Option<FocusHandle>,
}

impl MusicApp {
    pub(super) fn new_playlist(
        &mut self,
        source: CreationSource,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.playlist_creation.is_some() || self.smart.editor.is_some() {
            return;
        }
        self.playlist_rename = None;
        self.playlist_creation = Some(CreationDialog {
            draft: Creation::new(source),
            previous_focus: window.focused(cx),
        });
        self.playlist_name
            .update(cx, |input, cx| input.set_value(name.to_owned(), window, cx));
        cx.defer_in(window, |app, window, cx| {
            if app.playlist_creation.is_some() {
                app.playlist_name
                    .update(cx, |input, cx| input.focus(window, cx));
            }
        });
        cx.notify();
    }

    pub(super) fn playlist_name_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.playlist_creation.is_some() {
            match event {
                InputEvent::Change => {
                    self.sync_smart_inputs(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } if !self.creating_smart() => {
                    self.save_playlist_creation(cx)
                }
                _ => {}
            }
        }
    }

    pub(super) fn close_playlist_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.playlist_creation.take() {
            dialog
                .previous_focus
                .unwrap_or_else(|| self.focus.clone())
                .focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn creating_smart(&self) -> bool {
        self.playlist_creation
            .as_ref()
            .is_some_and(|dialog| dialog.draft.is_smart())
    }

    fn choose_playlist_kind(&mut self, smart: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.playlist_creation else {
            return;
        };
        if self.smart.saving || self.smart.needs_password {
            return;
        }
        dialog.draft.choose_smart(smart, self.smart.available);
        if dialog.draft.is_smart() {
            self.start_smart_draft(window, cx);
        }
        cx.notify();
    }

    fn cancel_playlist_creation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.playlist_creation
            && !dialog.draft.saving()
            && (!self.smart.saving || self.smart.needs_password)
        {
            self.cancel_smart(window, cx);
        }
    }

    fn save_playlist_creation(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.playlist_creation else {
            return;
        };
        if dialog.draft.is_smart() {
            self.save_smart(cx);
        } else if let Some(command) = dialog
            .draft
            .submit_manual(self.playlist_name.read(cx).value().as_str())
        {
            self.service.send(command);
            cx.notify();
        }
    }

    pub(super) fn complete_playlist_creation(
        &mut self,
        mutation: &Mutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .playlist_creation
            .as_ref()
            .is_some_and(|dialog| dialog.draft.matches(mutation))
        {
            self.close_playlist_creation(window, cx);
            self.reset_smart();
            self.playlists_open = true;
        }
    }

    pub(super) fn fail_playlist_creation(&mut self, mutation: &Mutation, error: &str) -> bool {
        if let Some(dialog) = &mut self.playlist_creation
            && dialog.draft.matches(mutation)
        {
            dialog.draft.fail(error.into());
            return true;
        }
        false
    }

    pub(super) fn fail_album_playlist_creation(&mut self, error: &str) -> bool {
        if let Some(dialog) = &mut self.playlist_creation
            && dialog.draft.saving()
            && matches!(dialog.draft.source(), CreationSource::Album(_))
            && error == crate::service::EMPTY_ALBUM_PLAYLIST_ERROR
        {
            dialog.draft.fail(error.into());
            return true;
        }
        false
    }

    pub(super) fn playlist_creation_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let draft = &self.playlist_creation.as_ref()?.draft;
        let smart = draft.is_smart();
        let busy = draft.saving() || self.smart.saving;
        let colors = ink(cx);
        let disabled = busy
            || if smart {
                !self.smart.available
                    || self
                        .smart
                        .editor
                        .as_ref()
                        .is_none_or(|editor| editor.build().is_err())
            } else {
                self.playlist_name.read(cx).value().trim().is_empty()
            };
        let description = match draft.source() {
            CreationSource::Empty if smart && !self.smart.available => {
                Some("Smart playlists are unavailable on this server.".into())
            }
            CreationSource::Empty if smart => {
                Some("Tracks update automatically based on your rules.".into())
            }
            CreationSource::Empty if draft.can_choose_type(self.smart.available) => {
                Some("Add and arrange tracks yourself.".into())
            }
            CreationSource::Tracks(ids) if ids.len() == 1 => Some("Starts with 1 track.".into()),
            CreationSource::Tracks(ids) if !ids.is_empty() => {
                Some(format!("Starts with {} tracks.", ids.len()))
            }
            CreationSource::Album(_) => {
                Some("Starts with all tracks from the selected album.".into())
            }
            _ => None,
        };
        let header = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("New playlist"),
            )
            .when(
                smart || draft.can_choose_type(self.smart.available),
                |header| {
                    header.child(
                        div().flex().gap_1().children(
                            [
                                ("playlist-type-manual", "Manual", false),
                                ("playlist-type-smart", "Smart", true),
                            ]
                            .map(|(id, label, value)| {
                                Button::new(id)
                                    .small()
                                    .outline()
                                    .label(label)
                                    .selected(smart == value)
                                    .toggled(smart == value)
                                    .disabled(busy || value && !self.smart.available)
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        app.choose_playlist_kind(value, window, cx)
                                    }))
                            }),
                        ),
                    )
                },
            )
            .when_some(description, |header, text| {
                header.child(div().text_color(colors.muted).child(text))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Playlist name"))
                    .child(
                        Input::new(&self.playlist_name)
                            .id("playlist-name")
                            .disabled(busy),
                    ),
            );
        let header = header.when_some(
            self.smart
                .editor
                .as_ref()
                .filter(|_| smart)
                .and_then(|editor| editor.name_error()),
            |header, error| header.child(Self::smart_validation("smart-name-error", error, cx)),
        );
        let card = div()
            .id("playlist-creation")
            .test_support()
            .aria_label("New playlist")
            .relative()
            .occlude()
            .w_full()
            .max_w(px(if smart { 720. } else { 440. }))
            .max_h_full()
            .min_h_0()
            .bg(colors.background)
            .border_1()
            .border_color(colors.line)
            .rounded_xl()
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .id("playlist-creation-scroll")
                    .flex()
                    .flex_col()
                    .gap_4()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_5()
                    .child(header)
                    .when(smart, |form| form.child(self.smart_rules_form(cx)))
                    .when_some(draft.error(), |form, error| {
                        form.child(Self::smart_validation("playlist-create-error", error, cx))
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .px_5()
                    .py_3()
                    .border_t_1()
                    .border_color(colors.line)
                    .child(
                        Button::new("cancel-playlist-creation")
                            .ghost()
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.cancel_playlist_creation(window, cx)
                            })),
                    )
                    .child(
                        Button::new("create-playlist")
                            .primary()
                            .label(if busy {
                                "Creating…"
                            } else {
                                "Create playlist"
                            })
                            .disabled(disabled)
                            .on_click(cx.listener(|app, _, _, cx| app.save_playlist_creation(cx))),
                    ),
            );
        let popup = if smart && self.smart.needs_password {
            self.smart_password_form(cx).into_any_element()
        } else {
            card.into_any_element()
        };

        let weak = cx.weak_entity();
        Some(
            Dialog::new(cx)
                .focus_handle(self.playlist_creation_focus.clone())
                .p_5()
                .close_on_backdrop_press(false)
                .backdrop(div().absolute().inset_0().occlude().bg(cx.theme().overlay))
                .popup(popup)
                .on_ok(|_, _, _| false)
                .on_cancel(move |_, window, cx| {
                    let _ = weak.update(cx, |app, cx| app.cancel_playlist_creation(window, cx));
                    false
                })
                .into_any_element(),
        )
    }
}
