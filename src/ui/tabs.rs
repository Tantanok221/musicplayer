//! Browser-style top tabs: each tab keeps a page and its own Back/Forward
//! history. Tabs, their pages and locks are saved in preferences.
use super::*;

/// A tab being dragged to a new position.
#[derive(Clone)]
struct TabDrag {
    from: usize,
    title: String,
}
struct TabDragPreview(String);
impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded_md()
            .text_size(px(11.))
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .border_1()
            .border_color(cx.theme().border)
            .child(self.0.clone())
    }
}

impl MusicApp {
    /// Records what the active tab shows. An empty tab stays empty until the
    /// user picks a page, and nothing is saved before preferences have loaded.
    pub(super) fn sync_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blank || !self.prefs_loaded || self.settings {
            return;
        }
        let page = self.location();
        let tab = self.preferences.tabs.active_mut();
        if tab.page.as_ref() != Some(&page) {
            tab.page = Some(page);
            self.schedule_tab_save(window, cx);
        }
    }

    /// Saves once changes settle, so typing a search does not write on every key.
    fn schedule_tab_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.prefs_loaded {
            return;
        }
        self.tab_save = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |app, _| app.save_preferences()).ok();
        }));
    }

    /// Shows the active tab's page and history.
    pub(super) fn show_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.preferences.tabs.active().clone();
        (self.history_back, self.history_forward) =
            self.histories.remove(&tab.id).unwrap_or_default();
        self.settings = false;
        match tab.page {
            Some(page) => {
                self.blank = false;
                self.restore_location(page, window, cx);
            }
            None => self.show_empty_page(window, cx),
        }
        self.workspace_scroll
            .scroll_to_item(self.preferences.tabs.active);
        // The page that had focus may be gone; without focus, shortcuts and
        // menu commands would not reach the app.
        self.focus.focus(window, cx);
        // The strip scrolls during layout; redraw so the arrows match where it lands.
        cx.on_next_frame(window, |_, _, cx| cx.notify());
        cx.notify();
    }

    fn show_empty_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.blank = true;
        self.artist_name = None;
        self.search_text.clear();
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    /// Returns the active tab to its empty page, as Back does from its first page.
    pub(super) fn empty_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences.tabs.active_mut().page = None;
        self.show_empty_page(window, cx);
        self.schedule_tab_save(window, cx);
        cx.notify();
    }

    /// Keeps the outgoing tab's page and history before another tab takes over.
    fn park_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_active_tab(window, cx);
        let id = self.preferences.tabs.active().id;
        let history = (
            std::mem::take(&mut self.history_back),
            std::mem::take(&mut self.history_forward),
        );
        self.histories.insert(id, history);
    }

    pub(super) fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.preferences.tabs.tabs.len() {
            return;
        }
        if ix == self.preferences.tabs.active && !self.settings {
            // Clicking a locked tab that has wandered off takes it home.
            let home = self.preferences.tabs.active().home_page().cloned();
            if let Some(home) = home.filter(|home| self.blank || *home != self.location()) {
                self.remember_location();
                self.blank = false;
                self.restore_location(home, window, cx);
                cx.notify();
            }
            return;
        }
        self.park_active_tab(window, cx);
        self.preferences.tabs.active = ix;
        self.show_active_tab(window, cx);
        self.schedule_tab_save(window, cx);
    }

    pub(super) fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.park_active_tab(window, cx);
        self.preferences.tabs.new_tab();
        self.show_active_tab(window, cx);
        self.schedule_tab_save(window, cx);
    }

    pub(super) fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .preferences
            .tabs
            .tabs
            .get(ix)
            .is_none_or(|tab| tab.locked)
        {
            return;
        }
        let was_active = ix == self.preferences.tabs.active;
        if was_active {
            // Saves the page it shows now and its Back/Forward history.
            self.park_active_tab(window, cx);
        }
        let tab = self.preferences.tabs.tabs[ix].clone();
        let history = self.histories.remove(&tab.id).unwrap_or_default();
        let active = self.preferences.tabs.active().id;
        if !self.preferences.tabs.close(ix) {
            return;
        }
        // Keep what was closed so Cmd-Shift-T can bring it back.
        if tab.page.is_some() {
            self.closed_tabs.push((ix, tab, history));
            if self.closed_tabs.len() > 20 {
                self.closed_tabs.remove(0);
            }
        }
        // Closing a tab in the background only shifts indices; the page stays.
        if was_active || self.preferences.tabs.active().id != active {
            self.show_active_tab(window, cx);
        }
        self.schedule_tab_save(window, cx);
        cx.notify();
    }

    pub(super) fn reopen_closed_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ix, tab, history)) = self.closed_tabs.pop() else {
            return;
        };
        self.park_active_tab(window, cx);
        let id = self.preferences.tabs.reopen(ix, tab);
        self.histories.insert(id, history);
        self.show_active_tab(window, cx);
        self.schedule_tab_save(window, cx);
    }

    fn toggle_tab_lock(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences.tabs.toggle_lock(ix);
        self.schedule_tab_save(window, cx);
        cx.notify();
    }

    fn move_tab(&mut self, from: usize, to: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences.tabs.move_tab(from, to);
        self.schedule_tab_save(window, cx);
        cx.notify();
    }

    pub(super) fn reset_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences.tabs.reset();
        self.histories.clear();
        self.history_back.clear();
        self.history_forward.clear();
        self.show_active_tab(window, cx);
        self.schedule_tab_save(window, cx);
    }

    /// Cmd-1…8 pick that tab; Cmd-9 picks the last, as in browsers.
    pub(super) fn select_tab_number(
        &mut self,
        number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.preferences.tabs.tabs.len();
        let ix = if number >= 9 { count - 1 } else { number - 1 };
        self.activate_tab(ix, window, cx);
    }

    /// What a new tab shows until the user picks a page.
    pub(super) fn empty_tab_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .px_6()
            .text_center()
            .child(
                Icon::new(Glyph::PanelsTopLeft)
                    .size(px(28.))
                    .text_color(colors.muted),
            )
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Empty tab"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.muted)
                    .child("Pick a page from the navigator on the left to open it here."),
            )
            .child(
                div()
                    .mt_2()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child("⌘T new tab · ⌘W close tab · ⇧⌘T reopen closed tab · ⌘1–9 switch tabs"),
            )
            .into_any_element()
    }

    /// Lock, close, new and reset, for a tab's right-click menu.
    fn tab_menu(
        &self,
        ix: usize,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let weak = cx.weak_entity();
        let can_reopen = !self.closed_tabs.is_empty();
        move |menu, _, _| {
            let on = |action: fn(&mut MusicApp, usize, &mut Window, &mut Context<MusicApp>)| {
                let weak = weak.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |app, cx| action(app, ix, window, cx));
                }
            };
            menu.item(
                PopupMenuItem::new(if locked { "Unlock tab" } else { "Lock tab" }).on_click(on(
                    |app, ix, window, cx| app.toggle_tab_lock(ix, window, cx),
                )),
            )
            .item(
                PopupMenuItem::new("Close tab")
                    .disabled(locked)
                    .on_click(on(|app, ix, window, cx| app.close_tab(ix, window, cx))),
            )
            .separator()
            .item(
                PopupMenuItem::new("Reopen closed tab")
                    .disabled(!can_reopen)
                    .on_click(on(|app, _, window, cx| app.reopen_closed_tab(window, cx))),
            )
            .item(
                PopupMenuItem::new("New tab")
                    .on_click(on(|app, _, window, cx| app.new_tab(window, cx))),
            )
            .item(
                PopupMenuItem::new("Reset all tabs")
                    .on_click(on(|app, _, window, cx| app.reset_tabs(window, cx))),
            )
        }
    }

    /// The tab strip's contents: every tab, then the + button and an empty
    /// area whose right-click menu adds or resets tabs.
    pub(super) fn tab_items(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = ink(cx);
        let active_ix = self.preferences.tabs.active;
        let mut items = Vec::new();
        for (ix, tab) in self.preferences.tabs.tabs.iter().enumerate() {
            let active = ix == active_ix && !self.settings;
            let title = tab.title();
            let label = title.to_uppercase();
            let group = SharedString::from(format!("tab-{}", tab.id));
            // Shown over the label's right edge on hover, so it never widens the tab.
            let overlay = div()
                .absolute()
                .top_0()
                .bottom(px(2.))
                .right_0()
                .pl_1()
                .pr_1()
                .flex()
                .items_center()
                .bg(colors.panel)
                .opacity(0.)
                .group_hover(group.clone(), |s| s.opacity(1.));
            let end = if tab.locked {
                overlay
                    .child(
                        Icon::new(Glyph::Lock)
                            .size(px(10.))
                            .text_color(colors.muted),
                    )
                    .into_any_element()
            } else {
                overlay
                    .child(
                        div()
                            .id(("tab-close", tab.id))
                            .role(Role::Button)
                            .aria_label(format!("Close {title}"))
                            .size(px(16.))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.bg(colors.line))
                            .child(Icon::new(Glyph::X).size(px(10.)))
                            .on_click(cx.listener(move |app, _, window, cx| {
                                cx.stop_propagation();
                                app.close_tab(ix, window, cx);
                            })),
                    )
                    .into_any_element()
            };
            items.push(
                div()
                    .id(("workspace-tab", tab.id))
                    .group(group)
                    .role(Role::Tab)
                    .aria_label(title.clone())
                    .aria_selected(active)
                    .flex_shrink_0()
                    .relative()
                    .max_w(px(200.))
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_2()
                    .border_color(if active {
                        colors.accent
                    } else {
                        colors.background
                    })
                    .text_color(if active { colors.accent } else { colors.muted })
                    .text_size(px(11.))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.panel))
                    .child(div().min_w_0().truncate().whitespace_nowrap().child(label))
                    .child(end)
                    .on_click(
                        cx.listener(move |app, _, window, cx| app.activate_tab(ix, window, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |app, _, window, cx| app.close_tab(ix, window, cx)),
                    )
                    .on_drag(
                        TabDrag {
                            from: ix,
                            title: title.clone(),
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| TabDragPreview(drag.title.clone()))
                        },
                    )
                    .drag_over::<TabDrag>(move |s, _, _, _| s.bg(colors.selection))
                    .on_drop(cx.listener(move |app, drag: &TabDrag, window, cx| {
                        app.move_tab(drag.from, ix, window, cx)
                    }))
                    .context_menu(self.tab_menu(ix, tab.locked, cx))
                    .into_any_element(),
            );
        }
        items.push(
            short_button("new-tab", Glyph::Plus, "New tab (⌘T)")
                .on_click(cx.listener(|app, _, window, cx| app.new_tab(window, cx)))
                .into_any_element(),
        );
        let weak = cx.weak_entity();
        items.push(
            div()
                .id("tab-strip-space")
                .flex_1()
                .min_w(px(24.))
                .h_full()
                .context_menu(move |menu, _, _| {
                    let (add, reset) = (weak.clone(), weak.clone());
                    menu.item(
                        PopupMenuItem::new("New tab").on_click(move |_, window, cx| {
                            let _ = add.update(cx, |app, cx| app.new_tab(window, cx));
                        }),
                    )
                    .item(
                        PopupMenuItem::new("Reset all tabs").on_click(move |_, window, cx| {
                            let _ = reset.update(cx, |app, cx| app.reset_tabs(window, cx));
                        }),
                    )
                })
                .into_any_element(),
        );
        items
    }
}
