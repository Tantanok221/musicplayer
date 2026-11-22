use super::*;
use crate::smart_editor::{Editor, FIELDS, Row, RowKind, Template, operators};

#[derive(Default)]
pub(super) struct SmartUi {
    pub available: bool,
    pub editor: Option<Editor>,
    name: Option<Entity<InputState>>,
    limit: Option<Entity<InputState>>,
    values: HashMap<u64, Entity<InputState>>,
    password: Option<Entity<InputState>>,
    pub needs_password: bool,
    pub password_busy: bool,
    pub loading: bool,
    pub saving: bool,
    pub error: Option<String>,
}

impl MusicApp {
    pub(super) fn new_smart_playlist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.smart.available || self.smart.editor.is_some() {
            return;
        }
        self.smart.editor = Some(Editor::new());
        self.settings = false;
        self.rebuild_smart_inputs(window, cx);
        cx.notify();
    }

    pub(super) fn edit_smart_playlist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.smart.available || self.smart.editor.is_some() {
            return;
        }
        let Some(playlist) = self.playlists.current() else {
            return;
        };
        if !crate::playlists::is_owner(playlist, self.account.as_ref().map(|a| a.username.as_str()))
            || !crate::playlists::is_smart(
                playlist,
                self.account.as_ref().map(|a| a.username.as_str()),
            )
        {
            return;
        }
        let id = playlist.id.clone();
        self.smart.editor = Some(Editor::load(
            Some(id.clone()),
            playlist.name.clone(),
            Default::default(),
        ));
        self.smart.loading = true;
        self.rebuild_smart_inputs(window, cx);
        self.service.send(Command::LoadSmartRules(id));
        cx.notify();
    }

    fn sync_smart_inputs(&mut self, cx: &App) {
        if let Some(editor) = &mut self.smart.editor {
            if let Some(input) = &self.smart.name {
                editor.name = input.read(cx).value().to_string();
            }
            if let Some(input) = &self.smart.limit {
                editor.limit = input.read(cx).value().to_string();
            }
            for (id, input) in &self.smart.values {
                if let Some(Row {
                    kind: RowKind::Condition { value, .. },
                    ..
                }) = editor.row_mut(*id)
                {
                    *value = input.read(cx).value().to_string();
                }
            }
        }
    }

    fn rebuild_smart_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        fn input(
            value: &str,
            window: &mut Window,
            cx: &mut Context<MusicApp>,
        ) -> Entity<InputState> {
            cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
        }
        fn values(
            rows: &[Row],
            out: &mut HashMap<u64, Entity<InputState>>,
            window: &mut Window,
            cx: &mut Context<MusicApp>,
        ) {
            for row in rows {
                match &row.kind {
                    RowKind::Condition { value, .. } => {
                        out.insert(row.id, input(value, window, cx));
                    }
                    RowKind::Group { rows, .. } => values(rows, out, window, cx),
                    _ => {}
                }
            }
        }
        self.smart.values.clear();
        if let Some(editor) = &self.smart.editor {
            self.smart.name = Some(input(&editor.name, window, cx));
            self.smart.limit = Some(input(&editor.limit, window, cx));
            values(&editor.rows, &mut self.smart.values, window, cx);
        }
        for input in self
            .smart
            .name
            .iter()
            .chain(self.smart.limit.iter())
            .chain(self.smart.values.values())
        {
            cx.subscribe_in(input, window, |app, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    app.sync_smart_inputs(cx);
                    cx.notify();
                }
            })
            .detach();
        }
    }

    fn change_smart(
        &mut self,
        change: impl FnOnce(&mut Editor),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.smart.loading || self.smart.saving || self.smart.needs_password {
            return;
        }
        self.sync_smart_inputs(cx);
        if let Some(editor) = &mut self.smart.editor {
            change(editor);
        }
        self.smart.error = None;
        self.rebuild_smart_inputs(window, cx);
        cx.notify();
    }

    pub(super) fn smart_rules_loaded(
        &mut self,
        id: String,
        rules: Option<crate::smart_rules::SmartRules>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.smart.loading
            || self.smart.editor.as_ref().and_then(|e| e.id.as_ref()) != Some(&id)
        {
            return;
        }
        self.sync_smart_inputs(cx);
        self.smart.loading = false;
        self.smart.needs_password = false;
        self.smart.password_busy = false;
        self.smart.password = None;
        if let Some(rules) = rules {
            self.error = None;
            let name = self.smart.editor.as_ref().unwrap().name.clone();
            self.smart.editor = Some(Editor::load(Some(id), name, rules));
            self.rebuild_smart_inputs(window, cx);
        } else {
            self.smart.error = Some("This playlist does not have smart rules".into());
        }
    }

    pub(super) fn smart_password_required(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.smart.editor.is_none() {
            return;
        }
        self.smart.needs_password = true;
        self.smart.password_busy = false;
        self.smart.password = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Navidrome password")
                .masked(true)
        }));
        cx.subscribe_in(
            self.smart.password.as_ref().unwrap(),
            window,
            |app, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    app.submit_native_password(window, cx);
                }
            },
        )
        .detach();
    }

    fn submit_native_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.smart.password_busy {
            return;
        }
        let Some(input) = &self.smart.password else {
            return;
        };
        let password = input.read(cx).value().to_string();
        if password.is_empty() {
            self.smart.error = Some("Enter your Navidrome password".into());
            cx.notify();
            return;
        }
        input.update(cx, |input, cx| input.set_value("", window, cx));
        self.smart.password_busy = true;
        self.smart.error = None;
        self.service.send(Command::NativeSignIn(password));
        cx.notify();
    }

    pub(super) fn smart_saved(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.smart.saving {
            return;
        }
        self.error = None;
        self.sync_smart_inputs(cx);
        let name = self
            .smart
            .editor
            .as_ref()
            .map(|e| e.name.clone())
            .unwrap_or_default();
        let available = self.smart.available;
        self.smart = SmartUi {
            available,
            ..Default::default()
        };
        self.playlists.open(Playlist {
            id,
            name,
            owner: self
                .account
                .as_ref()
                .map(|a| a.username.clone())
                .unwrap_or_default(),
            readonly: true,
            ..Default::default()
        });
        self.switch_workspace(Workspace::Playlists, window, cx);
    }

    fn cancel_smart(&mut self, cx: &mut Context<Self>) {
        if self.smart.loading || self.smart.saving || self.smart.needs_password {
            self.service.send(Command::CancelSmartPlaylist);
        }
        let available = self.smart.available;
        self.smart = SmartUi {
            available,
            ..Default::default()
        };
        cx.notify();
    }

    fn smart_choice(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        choices: Vec<(String, String)>,
        apply: impl Fn(&mut Editor, &str) + 'static,
        cx: &Context<Self>,
    ) -> AnyElement {
        let weak = cx.weak_entity();
        let apply = Rc::new(apply);
        Button::new(id)
            .small()
            .rounded_full()
            .label(format!("{} ▾", label.into()))
            .dropdown_menu(move |mut menu, _, _| {
                for (key, label) in &choices {
                    let weak = weak.clone();
                    let key = key.clone();
                    let apply = apply.clone();
                    menu = menu.item(PopupMenuItem::new(label.clone()).on_click(
                        move |_, window, cx| {
                            let _ = weak.update(cx, |app, cx| {
                                app.change_smart(|e| apply(e, &key), window, cx)
                            });
                        },
                    ));
                }
                menu
            })
            .into_any_element()
    }

    fn smart_match(&self, id: Option<u64>, any: bool, cx: &Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let key = id
            .map(|i| format!("smart-match-{i}"))
            .unwrap_or("smart-match-root".into());
        let mut segment = div()
            .flex()
            .gap_1()
            .p_1()
            .rounded_lg()
            .bg(colors.panel)
            .border_1()
            .border_color(colors.line);
        for (value, label) in [(false, "All"), (true, "Any")] {
            segment = segment.child(
                Button::new(SharedString::from(format!(
                    "{key}-{}",
                    if value { "any" } else { "all" }
                )))
                .small()
                .ghost()
                .label(label)
                .rounded_md()
                .when(any == value, |b| {
                    b.bg(colors.selection).text_color(colors.accent)
                })
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.change_smart(
                        |e| {
                            if let Some(id) = id {
                                if let Some(Row {
                                    kind: RowKind::Group { any, .. },
                                    ..
                                }) = e.row_mut(id)
                                {
                                    *any = value;
                                }
                            } else {
                                e.any = value;
                            }
                        },
                        window,
                        cx,
                    )
                })),
            );
        }
        div()
            .flex()
            .items_center()
            .gap_2()
            .child("Match")
            .child(segment)
            .child("of these rules")
            .into_any_element()
    }

    fn smart_rows(&self, rows: &[Row], cx: &Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let mut list = div().flex().flex_col().gap_2();
        for row in rows {
            let id = row.id;
            let remove = Button::new(SharedString::from(format!("smart-remove-{id}")))
                .small()
                .ghost()
                .label("×")
                .tooltip("Remove rule")
                .accessibility_label("Remove rule")
                .rounded_full()
                .w(px(26.))
                .h(px(26.))
                .text_color(colors.muted)
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.change_smart(|e| e.remove(id), window, cx)
                }));
            list = list.child(match &row.kind {
                RowKind::Advanced(node) => div()
                    .p_3()
                    .border_1()
                    .border_color(colors.line)
                    .rounded_lg()
                    .bg(colors.panel)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .id(SharedString::from(format!("smart-advanced-{id}")))
                            .test_support()
                            .aria_label("Advanced rule (kept as-is)")
                            .text_color(colors.muted)
                            .child("Advanced rule (kept as-is)"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_family("monospace")
                            .truncate()
                            .text_color(colors.muted)
                            .child(serde_json::to_string(node).unwrap_or_default()),
                    )
                    .into_any_element(),
                RowKind::Group { any, rows } => div()
                    .p_3()
                    .border_1()
                    .border_color(colors.line)
                    .rounded_lg()
                    .ml_3()
                    .bg(colors.panel)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.smart_match(Some(id), *any, cx))
                            .child(div().flex_1())
                            .when(!row.has_advanced(), |d| d.child(remove)),
                    )
                    .child(self.smart_rows(rows, cx))
                    .when(rows.is_empty(), |d| {
                        d.child(Self::smart_validation(
                            SharedString::from(format!("smart-group-error-{id}")),
                            "Add a condition to every group",
                            cx,
                        ))
                    })
                    .child(
                        Button::new(SharedString::from(format!("smart-add-to-{id}")))
                            .small()
                            .ghost()
                            .label("+ Add rule")
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.change_smart(|e| e.add_condition(Some(id)), window, cx)
                            })),
                    )
                    .into_any_element(),
                RowKind::Condition { field, op, value } => {
                    let field_label = FIELDS
                        .iter()
                        .find(|(key, _)| *key == field)
                        .map(|(_, label)| *label)
                        .unwrap_or(field);
                    let op_label = operators(field)
                        .iter()
                        .find(|(key, _)| *key == op)
                        .map(|(_, label)| *label)
                        .unwrap_or(op);
                    div()
                        .id(SharedString::from(format!("smart-row-{id}")))
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(colors.line)
                        .bg(colors.background)
                        .hover(|s| s.border_color(colors.accent))
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(
                            self.smart_choice(
                                SharedString::from(format!("smart-field-{id}")),
                                field_label.to_string(),
                                FIELDS
                                    .iter()
                                    .map(|(k, l)| (k.to_string(), l.to_string()))
                                    .collect(),
                                move |e, f| e.field(id, f),
                                cx,
                            ),
                        )
                        .child(
                            self.smart_choice(
                                SharedString::from(format!("smart-op-{id}")),
                                op_label.to_string(),
                                operators(field)
                                    .iter()
                                    .map(|(k, l)| (k.to_string(), l.to_string()))
                                    .collect(),
                                move |e, op| {
                                    if let Some(Row {
                                        kind: RowKind::Condition { op: o, .. },
                                        ..
                                    }) = e.row_mut(id)
                                    {
                                        *o = op.into();
                                    }
                                },
                                cx,
                            ),
                        )
                        .child(
                            div()
                                .w(px(
                                    (value.chars().count() as f32 * 7. + 32.).clamp(120., 210.)
                                ))
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    Input::new(&self.smart.values[&id])
                                        .id(SharedString::from(format!("smart-value-{id}")))
                                        .small(),
                                )
                                .when_some(row.hint(), |d, hint| {
                                    d.child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(colors.muted)
                                            .child(hint),
                                    )
                                })
                                .when_some(row.validation(), |d, error| {
                                    d.child(Self::smart_validation(
                                        format!("smart-value-error-{id}"),
                                        error,
                                        cx,
                                    ))
                                }),
                        )
                        .child(div().flex_1())
                        .child(remove)
                        .into_any_element()
                }
            });
        }
        list.into_any_element()
    }

    fn smart_validation(id: impl Into<ElementId>, message: &str, cx: &App) -> AnyElement {
        div()
            .id(id)
            .test_support()
            .aria_label(message.to_string())
            .text_size(px(11.))
            .text_color(ink(cx).favorite)
            .child(if message == "Enter a value for every condition" {
                "Value required".to_string()
            } else {
                message.to_string()
            })
            .into_any_element()
    }

    fn smart_primary(
        &self,
        id: &'static str,
        label: &'static str,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Button {
        let colors = ink(cx);
        Button::new(id)
            .label(label)
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(colors.accent)
                    .foreground(if colors.dark {
                        rgb(0x1b1c20).into()
                    } else {
                        rgb(0xffffff).into()
                    })
                    .hover(colors.flame)
                    .active(colors.flame),
            )
            .disabled(disabled)
            .when(!disabled, |b| b.bg(colors.accent))
            .rounded_lg()
    }

    fn smart_password_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let mut dialog =
            div()
                .w(px(420.))
                .max_w_full()
                .p_6()
                .rounded_xl()
                .border_1()
                .border_color(colors.line)
                .bg(colors.background)
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .size(px(40.))
                        .rounded_full()
                        .bg(colors.selection)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(colors.accent)
                        .child(Icon::new(Glyph::LockKeyhole).size(px(20.))),
                )
                .child(
                    div()
                        .id("native-password-title")
                        .test_support()
                        .aria_label("Sign in to Navidrome again")
                        .text_size(px(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Sign in to Navidrome again"),
                )
                .child({
                    let explanation="Your editing session is missing or has expired. Your password is used once to sign in again.";
                    div().id("native-session-explanation").test_support().aria_label(explanation).text_color(colors.muted).child(explanation)
                })
                .child(
                    div()
                        .id("native-password-explanation")
                        .test_support()
                        .aria_label("Your password is not stored.")
                        .text_color(colors.muted)
                        .child("Your password is not stored."),
                )
                .child(Input::new(self.smart.password.as_ref().unwrap()).id("native-password"))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.muted)
                        .child("Only a server-issued access token is saved on this device."),
                );
        if let Some(error) = &self.smart.error {
            dialog = dialog.child(Self::smart_validation("native-password-error", error, cx));
        }
        dialog = dialog
            .child(
                self.smart_primary(
                    "native-sign-in",
                    if self.smart.password_busy {
                        "Signing in…"
                    } else {
                        "Sign in and continue"
                    },
                    self.smart.password_busy,
                    cx,
                )
                .w_full()
                .on_click(cx.listener(|app, _, window, cx| app.submit_native_password(window, cx))),
            )
            .child(
                Button::new("cancel-smart-playlist")
                    .ghost()
                    .label("Cancel")
                    .on_click(cx.listener(|app, _, _, cx| app.cancel_smart(cx))),
            );
        div()
            .absolute()
            .occlude()
            .inset_0()
            .bg(gpui_kit::rgba(0x00000066))
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .child(dialog)
            .into_any_element()
    }

    pub(super) fn smart_editor_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let editor = self.smart.editor.as_ref().unwrap();
        let mut templates = div().flex().flex_wrap().gap_2();
        for (i, template) in Template::ALL.into_iter().enumerate() {
            templates = templates.child(
                Button::new(SharedString::from(format!("smart-template-{i}")))
                    .small()
                    .rounded_full()
                    .border_1()
                    .border_color(colors.line)
                    .label(template.label())
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(colors.panel)
                            .foreground(colors.muted)
                            .hover(colors.selection)
                            .active(colors.selection),
                    )
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.change_smart(|e| e.template(template), window, cx)
                    })),
            );
        }
        let name = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().font_weight(FontWeight::MEDIUM).child("Playlist name"))
            .child(
                Input::new(self.smart.name.as_ref().unwrap())
                    .id("smart-name")
                    .h(px(40.))
                    .disabled(self.smart.loading || self.smart.saving)
                    .text_size(px(15.)),
            )
            .when_some(editor.name_error(), |d, error| {
                d.child(Self::smart_validation("smart-name-error", error, cx))
            });
        let sort = editor
            .sort
            .as_deref()
            .and_then(|sort| {
                FIELDS
                    .iter()
                    .find(|(key, _)| *key == sort)
                    .map(|(_, label)| *label)
            })
            .unwrap_or_else(|| {
                if editor.sort.as_deref() == Some("random") {
                    "Random"
                } else {
                    editor.sort.as_deref().unwrap_or("Server default")
                }
            });
        let mut order = div().flex().p_1().gap_1().rounded_lg().bg(colors.panel);
        for (key, label) in [("asc", "↑ Asc"), ("desc", "↓ Desc")] {
            order = order.child(
                Button::new(SharedString::from(format!("smart-order-{key}")))
                    .ghost()
                    .small()
                    .label(label)
                    .when(editor.order.as_deref() == Some(key), |b| {
                        b.bg(colors.selection).text_color(colors.accent)
                    })
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.change_smart(|e| e.order = Some(key.into()), window, cx)
                    })),
            );
        }
        let options = div()
            .flex()
            .items_start()
            .flex_wrap()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Sort by"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                self.smart_choice(
                                    "smart-sort",
                                    sort.to_string(),
                                    std::iter::once((String::new(), "Server default".into()))
                                        .chain(std::iter::once(("random".into(), "Random".into())))
                                        .chain(
                                            FIELDS
                                                .iter()
                                                .map(|(k, l)| (k.to_string(), l.to_string())),
                                        )
                                        .collect(),
                                    |e, s| e.sort = (!s.is_empty()).then(|| s.into()),
                                    cx,
                                ),
                            )
                            .child(order),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Track limit"))
                    .child(
                        div().w(px(120.)).child(
                            Input::new(self.smart.limit.as_ref().unwrap())
                                .id("smart-limit")
                                .small(),
                        ),
                    )
                    .when_some(editor.limit_error(), |d, error| {
                        d.child(Self::smart_validation("smart-limit-error", error, cx))
                    }),
            );
        let content =
            div()
                .id("smart-editor-scroll")
                .overflow_y_scroll()
                .min_h_0()
                .flex_1()
                .p_6()
                .flex()
                .flex_col()
                .gap_5()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(24.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(if editor.id.is_some() {
                                    "Edit smart playlist"
                                } else {
                                    "New smart playlist"
                                }),
                        )
                        .child(div().text_color(colors.muted).text_size(px(12.)).child(
                            "Rules are evaluated by Navidrome. Tracks update automatically.",
                        )),
                )
                .child(name)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.muted)
                                .child("START WITH A TEMPLATE"),
                        )
                        .child(templates),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(self.smart_match(None, editor.any, cx))
                        .child(self.smart_rows(&editor.rows, cx))
                        .when(editor.rows.is_empty() && !self.smart.loading, |d| {
                            d.child(Self::smart_validation(
                                "smart-rules-error",
                                "Add at least one condition",
                                cx,
                            ))
                        })
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    Button::new("smart-add-condition")
                                        .ghost()
                                        .small()
                                        .label("+ Add rule")
                                        .text_color(colors.accent)
                                        .on_click(cx.listener(|app, _, window, cx| {
                                            app.change_smart(|e| e.add_condition(None), window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("smart-add-group")
                                        .ghost()
                                        .small()
                                        .label("+ Add group")
                                        .text_color(colors.accent)
                                        .on_click(cx.listener(|app, _, window, cx| {
                                            app.change_smart(Editor::add_group, window, cx)
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .id("smart-summary")
                                .test_support()
                                .aria_label(editor.summary())
                                .text_size(px(12.))
                                .text_color(colors.muted)
                                .p_3()
                                .rounded_lg()
                                .bg(colors.panel)
                                .child(editor.summary()),
                        ),
                )
                .child(
                    div()
                        .border_t_1()
                        .border_color(colors.line)
                        .pt_4()
                        .child(options),
                )
                .when_some(
                    self.smart
                        .error
                        .as_ref()
                        .filter(|_| !self.smart.needs_password),
                    |d, error| d.child(Self::smart_validation("smart-error", error, cx)),
                )
                .when(self.smart.loading, |d| d.child("Loading rules…"));
        let footer = div()
            .flex()
            .items_center()
            .justify_end()
            .flex_shrink_0()
            .gap_2()
            .px_6()
            .py_4()
            .border_t_1()
            .border_color(colors.line)
            .bg(colors.background)
            .child(
                Button::new("cancel-smart-playlist")
                    .label("Cancel")
                    .rounded_lg()
                    .disabled(self.smart.saving)
                    .on_click(cx.listener(|app, _, _, cx| app.cancel_smart(cx))),
            )
            .child(
                self.smart_primary(
                    "save-smart-playlist",
                    if self.smart.saving {
                        "Saving…"
                    } else {
                        "Save playlist"
                    },
                    self.smart.saving || self.smart.loading || editor.build().is_err(),
                    cx,
                )
                .on_click(cx.listener(|app, _, _, cx| {
                    app.sync_smart_inputs(cx);
                    let editor = app.smart.editor.as_ref().unwrap();
                    match editor.build() {
                        Ok(rules) => {
                            app.service.send(Command::SaveSmartPlaylist {
                                id: editor.id.clone(),
                                name: editor.name.trim().into(),
                                rules,
                            });
                            app.smart.saving = true;
                            app.smart.error = None;
                        }
                        Err(error) => app.smart.error = Some(error.into()),
                    }
                    cx.notify();
                })),
            );
        div()
            .id("smart-editor")
            .test_support()
            .relative()
            .size_full()
            .min_h_0()
            .p_6()
            .bg(colors.panel)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(720.))
                    .max_h_full()
                    .min_h_0()
                    .bg(colors.background)
                    .border_1()
                    .border_color(colors.line)
                    .rounded_xl()
                    .shadow_sm()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(content)
                    .child(footer),
            )
            .when(self.smart.needs_password, |d| {
                d.child(self.smart_password_dialog(cx))
            })
            .into_any_element()
    }
}
