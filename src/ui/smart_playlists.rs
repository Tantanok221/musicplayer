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
    }

    fn change_smart(
        &mut self,
        change: impl FnOnce(&mut Editor),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            .label(label)
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
        self.smart_choice(
            SharedString::from(
                id.map(|i| format!("smart-match-{i}"))
                    .unwrap_or("smart-match-root".into()),
            ),
            if any { "Match any" } else { "Match all" },
            vec![
                ("all".into(), "Match all".into()),
                ("any".into(), "Match any".into()),
            ],
            move |e, value| {
                if let Some(id) = id {
                    if let Some(Row {
                        kind: RowKind::Group { any, .. },
                        ..
                    }) = e.row_mut(id)
                    {
                        *any = value == "any";
                    }
                } else {
                    e.any = value == "any";
                }
            },
            cx,
        )
    }

    fn smart_rows(&self, rows: &[Row], cx: &Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let mut list = div().flex().flex_col().gap_2();
        for row in rows {
            let id = row.id;
            let remove = Button::new(SharedString::from(format!("smart-remove-{id}")))
                .small()
                .ghost()
                .label("Remove")
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.change_smart(|e| e.remove(id), window, cx)
                }));
            list = list.child(match &row.kind {
                RowKind::Advanced(node) => div()
                    .p_3()
                    .border_1()
                    .border_color(colors.line)
                    .rounded_md()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .id(SharedString::from(format!("smart-advanced-{id}")))
                            .test_support()
                            .aria_label("Advanced rule — kept as is")
                            .text_color(colors.muted)
                            .child("Advanced rule — kept as is"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .child(serde_json::to_string(node).unwrap_or_default()),
                    )
                    .into_any_element(),
                RowKind::Group { any, rows } => div()
                    .p_3()
                    .border_1()
                    .border_color(colors.line)
                    .rounded_md()
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
                    .child(
                        Button::new(SharedString::from(format!("smart-add-to-{id}")))
                            .small()
                            .label("Add condition")
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.change_smart(|e| e.add_condition(Some(id)), window, cx)
                            })),
                    )
                    .into_any_element(),
                RowKind::Condition { field, op, .. } => {
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
                        .flex()
                        .items_center()
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
                            div().flex_1().min_w_0().child(
                                Input::new(&self.smart.values[&id])
                                    .id(SharedString::from(format!("smart-value-{id}")))
                                    .small(),
                            ),
                        )
                        .child(remove)
                        .into_any_element()
                }
            });
        }
        list.into_any_element()
    }

    pub(super) fn smart_editor_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let editor = self.smart.editor.as_ref().unwrap();
        let mut view = div()
            .id("smart-editor")
            .test_support()
            .size_full()
            .min_h_0()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_size(px(18.)).child(if editor.id.is_some() {
                "Edit Smart Playlist"
            } else {
                "New Smart Playlist"
            }))
            .child(
                div()
                    .text_color(colors.muted)
                    .child("Rules are evaluated by Navidrome. Tracks update automatically."),
            );
        if let Some(error) = &self.smart.error {
            view = view.child(
                div()
                    .id("smart-error")
                    .test_support()
                    .aria_label(error.clone())
                    .child(error.clone()),
            );
        }
        if self.smart.needs_password {
            return view.child(div().flex_1().flex().flex_col().gap_3().p_4().border_1().border_color(colors.line).rounded_md()
                .child(div().text_size(px(16.)).child("Sign in to edit smart playlists"))
                .child("Navidrome needs your password once to authorize its native playlist API.")
                .child(div().id("native-password-explanation").test_support().aria_label("Your password is not stored.").child("Your password is not stored."))
                .child("Only a server-issued access token is saved locally.")
                .child(div().max_w(px(360.)).child(Input::new(self.smart.password.as_ref().unwrap()).id("native-password")))
                .child(Button::new("native-sign-in").primary().label(if self.smart.password_busy{"Signing in…"}else{"Sign in and continue"}).disabled(self.smart.password_busy).on_click(cx.listener(|app,_,window,cx|{
                    let input=app.smart.password.as_ref().unwrap();let password=input.read(cx).value().to_string();
                    if password.is_empty(){app.smart.error=Some("Enter your Navidrome password".into());cx.notify();return;}
                    input.update(cx,|input,cx|input.set_value("",window,cx));app.smart.password_busy=true;app.smart.error=None;
                    app.service.send(Command::NativeSignIn(password));cx.notify();
                })))
                .child(Button::new("cancel-smart-playlist").label("Cancel").on_click(cx.listener(|app,_,_,cx|app.cancel_smart(cx))))).into_any_element();
        }
        if self.smart.loading {
            return view
                .child("Loading rules…")
                .child(
                    Button::new("cancel-smart-playlist")
                        .label("Cancel")
                        .on_click(cx.listener(|app, _, _, cx| app.cancel_smart(cx))),
                )
                .into_any_element();
        }
        view=view.child(div().id("smart-editor-scroll").min_h_0().flex_1().overflow_y_scroll().flex().flex_col().gap_3()
            .child(div().child("Playlist name").child(Input::new(self.smart.name.as_ref().unwrap()).id("smart-name")))
            .child(self.smart_choice("smart-template","Apply template…",Template::ALL.iter().enumerate().map(|(i,t)|(i.to_string(),t.label().into())).collect(),|e,index|{if let Ok(index)=index.parse::<usize>() && let Some(t)=Template::ALL.get(index){e.template(*t);}},cx))
            .child(div().flex().items_center().gap_2().child(self.smart_match(None,editor.any,cx)).child("of the following conditions"))
            .child(self.smart_rows(&editor.rows,cx))
            .child(div().flex().gap_2()
                .child(Button::new("smart-add-condition").small().label("Add condition").on_click(cx.listener(|app,_,window,cx|app.change_smart(|e|e.add_condition(None),window,cx))))
                .child(Button::new("smart-add-group").small().label("Add group").on_click(cx.listener(|app,_,window,cx|app.change_smart(Editor::add_group,window,cx)))))
            .child(div().text_color(colors.muted).text_size(px(11.)).child("Ranges: [minimum, maximum] · Dates: YYYY-MM-DD · Relative dates: 30d, 90d · Loved: true or false"))
            .child(div().flex().items_center().gap_2().child("Sort")
                .child(self.smart_choice("smart-sort",editor.sort.clone().unwrap_or("Server default".into()),std::iter::once((String::new(),"Server default".into())).chain(std::iter::once(("random".into(),"Random".into()))).chain(FIELDS.iter().map(|(k,l)|(k.to_string(),l.to_string()))).collect(),|e,s|{e.sort=(!s.is_empty()).then(||s.into());},cx))
                .child(self.smart_choice("smart-order",editor.order.clone().unwrap_or("Server order".into()),vec![(String::new(),"Server order".into()),("asc".into(),"Ascending".into()),("desc".into(),"Descending".into())],|e,o|e.order=(!o.is_empty()).then(||o.into()),cx)))
            .child(div().flex().items_center().gap_2().child("Limit tracks (optional)").child(div().w(px(110.)).child(Input::new(self.smart.limit.as_ref().unwrap()).id("smart-limit").small()))));
        view.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("save-smart-playlist")
                        .primary()
                        .label(if self.smart.saving {
                            "Saving…"
                        } else {
                            "Save playlist"
                        })
                        .disabled(self.smart.saving)
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
                )
                .child(
                    Button::new("cancel-smart-playlist")
                        .label("Cancel")
                        .disabled(self.smart.saving)
                        .on_click(cx.listener(|app, _, _, cx| app.cancel_smart(cx))),
                ),
        )
        .into_any_element()
    }
}
