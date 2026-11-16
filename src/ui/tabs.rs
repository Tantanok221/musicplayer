//! Browser-style top tabs: each tab keeps a page and its own Back/Forward
//! history. Tabs, their pages and locks are saved in preferences.
use super::*;

/// The tab shortcuts used by the desktop app and its integration tests.
pub fn tab_bindings() -> [KeyBinding; 12] {
    use crate::platform::shortcut;
    [
        KeyBinding::new(shortcut("cmd-t", "ctrl-t"), NewTab, Some("Sonora")),
        KeyBinding::new(shortcut("cmd-w", "ctrl-w"), CloseTab, Some("Sonora")),
        KeyBinding::new(
            shortcut("cmd-shift-t", "ctrl-shift-t"),
            ReopenClosedTab,
            Some("Sonora"),
        ),
        KeyBinding::new(shortcut("cmd-1", "ctrl-1"), SelectTab1, Some("Sonora")),
        KeyBinding::new(shortcut("cmd-2", "ctrl-2"), SelectTab2, Some("Sonora")),
        KeyBinding::new(shortcut("cmd-3", "ctrl-3"), SelectTab3, Some("Sonora")),
        KeyBinding::new(shortcut("cmd-4", "ctrl-4"), SelectTab4, Some("Sonora")),
        KeyBinding::new(shortcut("cmd-5", "ctrl-5"), SelectTab5, Some("Sonora")),
        KeyBinding::new(shortcut("cmd-6", "ctrl-6"), SelectTab6, Some("Sonora")),
        KeyBinding::new(shortcut("cmd-7", "ctrl-7"), SelectTab7, Some("Sonora")),
        KeyBinding::new(shortcut("cmd-8", "ctrl-8"), SelectTab8, Some("Sonora")),
        KeyBinding::new(shortcut("cmd-9", "ctrl-9"), SelectTab9, Some("Sonora")),
    ]
}

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
        self.history = self.histories.remove(&tab.id).unwrap_or_default();
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
        let history = std::mem::take(&mut self.history);
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
        self.history = History::default();
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

    /// The right-click menu for the top bar outside the tabs: the active tab's options.
    pub(super) fn bar_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let tabs = &self.preferences.tabs;
        self.tab_menu(tabs.active, tabs.active().locked, cx)
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
                            .test_support()
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
                    .test_support()
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
            div()
                .id("tab-strip-space")
                .flex_1()
                .min_w(px(24.))
                .h_full()
                .context_menu(self.bar_menu(cx))
                .into_any_element(),
        );
        items
    }
}

impl MusicApp {
    pub(super) fn tabs(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let width = f32::from(window.viewport_size().width);
        let search_width = (width * 0.22).clamp(120., 240.);
        let controls_inset = crate::platform::TITLEBAR_CONTROLS_INSET;
        // Reserve room for Back/Forward, New Tab, layout controls, drag space and search.
        let tabs_width = (width - search_width - controls_inset - 64. - 36. - 48. - 16.).max(100.);
        // Offer an arrow only toward tabs that are out of view, as of the last layout.
        let scrolled = -f32::from(self.workspace_scroll.offset().x);
        let max_scroll = f32::from(self.workspace_scroll.max_offset().x);
        let scroll_left = scrolled > 1.;
        let scroll_right = scrolled < max_scroll - 1.;
        let mut tabs = div()
            .id("workspace-tabs")
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .h_full()
            .gap_1()
            .overflow_x_scroll()
            .track_scroll(&self.workspace_scroll)
            .on_scroll(cx.listener(|_, _, _, cx| cx.notify()));
        tabs = tabs.children(self.tab_items(cx));
        let tabs = div()
            .max_w(px(tabs_width))
            .min_w_0()
            .h_full()
            .flex_shrink(1.)
            .relative()
            .flex()
            .items_center()
            .child(tabs)
            // The arrows float over the strip's ends so the tabs never shift
            // when one appears or disappears.
            .when(scroll_left, |row| {
                row.child(
                    edge("tabs-left-edge", colors.background, true)
                        .left_0()
                        .pr_1()
                        .child(
                            short_button("tabs-left", Glyph::ChevronLeft, "Scroll tabs left")
                                .on_click(cx.listener(|app, _, _, cx| {
                                    let offset = app.workspace_scroll.offset();
                                    let x = (offset.x + px(140.)).min(px(0.));
                                    app.workspace_scroll.set_offset(point(x, px(0.)));
                                    cx.notify();
                                })),
                        )
                        .context_menu(self.bar_menu(cx)),
                )
            })
            .when(scroll_right, |row| {
                row.child(
                    edge("tabs-right-edge", colors.background, false)
                        .right_0()
                        .pl_1()
                        .child(
                            short_button("tabs-right", Glyph::ChevronRight, "Scroll tabs right")
                                .on_click(cx.listener(|app, _, _, cx| {
                                    let offset = app.workspace_scroll.offset();
                                    let max = app.workspace_scroll.max_offset().x;
                                    let x = (offset.x - px(140.)).max(-max);
                                    app.workspace_scroll.set_offset(point(x, px(0.)));
                                    cx.notify();
                                })),
                        )
                        .context_menu(self.bar_menu(cx)),
                )
            });
        let navigator = navigator_key(&self.preferences.layout);
        let weak = cx.weak_entity();
        let info = self.preferences.layout.show_information;
        let browser = self.browser_visible();
        div()
            .w_full()
            .min_w_0()
            .h(px(36.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(colors.line)
            .when(controls_inset > 0., |bar| {
                bar.child(
                    // The transparent macOS title bar places traffic lights in this area.
                    div()
                        .id("titlebar-controls")
                        .w(px(controls_inset))
                        .h_full()
                        .flex_shrink_0()
                        .context_menu(self.bar_menu(cx)),
                )
            })
            .child(
                div()
                    .id("history-buttons")
                    .h_full()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(2.))
                    .pr_2()
                    .child(
                        short_button("history-back", Glyph::ChevronLeft, "Back")
                            .disabled(!self.history.can_go_back())
                            .on_click(cx.listener(|app, _, window, cx| app.go_back(window, cx))),
                    )
                    .child(
                        short_button("history-forward", Glyph::ChevronRight, "Forward")
                            .disabled(!self.history.can_go_forward())
                            .on_click(cx.listener(|app, _, window, cx| app.go_forward(window, cx))),
                    )
                    .context_menu(self.bar_menu(cx)),
            )
            .child(tabs)
            // Keep New Tab reachable while the tab strip scrolls.
            .child(
                short_button("new-tab", Glyph::Plus, "New tab (⌘T)")
                    .on_click(cx.listener(|app, _, window, cx| app.new_tab(window, cx))),
            )
            .child(
                div()
                    .id("titlebar-drag")
                    .flex_1()
                    .min_w(px(24.))
                    .h_full()
                    .on_double_click(|_, window, _| window.titlebar_double_click())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|app, _, _, _| app.titlebar_dragging = true),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|app, _, _, _| app.titlebar_dragging = false),
                    )
                    .on_mouse_down_out(cx.listener(|app, _, _, _| app.titlebar_dragging = false))
                    .on_mouse_move(cx.listener(|app, _, window, _| {
                        if app.titlebar_dragging {
                            app.titlebar_dragging = false;
                            window.start_window_move();
                        }
                    }))
                    .context_menu(self.bar_menu(cx)),
            )
            .child(
                short_button("layout-menu", Glyph::PanelsTopLeft, "Configure layout")
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.label("Navigator and artist browser");
                        for (key, label) in [
                            ("show", "Show"),
                            ("auto", "Auto-hide"),
                            ("disabled", "Disabled"),
                        ] {
                            let weak = weak.clone();
                            menu = menu.item(
                                PopupMenuItem::new(label)
                                    .checked(navigator == key)
                                    .on_click(move |_, _, cx| {
                                        let _ = weak.update(cx, |app, cx| {
                                            set_navigator(&mut app.preferences.layout, key);
                                            app.navigator_peek = None;
                                            app.save_preferences();
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        menu.separator()
                            .item(
                                PopupMenuItem::new("Queue and information")
                                    .checked(info)
                                    .action(Box::new(ToggleInformation)),
                            )
                            .item(
                                PopupMenuItem::new("Column browser")
                                    .checked(browser)
                                    .action(Box::new(ToggleBrowser)),
                            )
                            .separator()
                            .menu("Restore initial layout", Box::new(RestoreLayout))
                            .separator()
                            .menu("Settings…", Box::new(OpenSettings))
                    }),
            )
            .child(
                div().w(px(search_width)).flex_shrink_0().px_2().child(
                    Input::new(&self.search)
                        .id("music-search")
                        .aria_label("Search music")
                        .small()
                        .prefix(Icon::new(Glyph::Search).size_3()),
                ),
            )
            .into_any_element()
    }
}
