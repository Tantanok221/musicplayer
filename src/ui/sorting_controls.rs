use super::*;

impl MusicApp {
    pub(super) fn sort_scope(&self) -> SortScope {
        if self.main_view() == MainView::Tracks {
            SortScope::Tracks
        } else {
            SortScope::Albums
        }
    }
    fn active_sort(&self) -> SortSpec {
        self.query
            .ordering
            .clone()
            .unwrap_or_else(|| self.preferences.sorting.get(self.sort_scope()))
    }
    fn apply_sort(&mut self, spec: SortSpec, cx: &mut Context<Self>) {
        let spec = spec.validated(self.sort_scope());
        self.preferences.sorting.set(spec.clone());
        self.query.ordering = Some(spec);
        self.explorer_query.ordering = self.query.ordering.clone();
        self.alphabet_jump = None;
        self.request_query(cx);
        self.album_list.reset(self.album_row_count());
        self.cover_list.reset(self.cover_rows().row_count());
        self.track_scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.save_preferences();
    }
    pub(super) fn sort_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let scope = self.sort_scope();
        let current = self.active_sort();
        let available = self.view.sort_fields.clone();
        let name = current.label();
        let weak = cx.weak_entity();
        Button::new("sort-menu")
            .ghost()
            .compact()
            .icon(Glyph::ArrowDownWideNarrow)
            .when(self.library_width >= 620., |button| {
                button.label(format!("Sort: {name}"))
            })
            .tooltip(format!("Sort: {name}\n{}", current.summary()))
            .dropdown_menu(move |mut menu, _, _| {
                for preset in Preset::choices(scope) {
                    // Picking Random, even again, deals a fresh order.
                    let spec = if *preset == Preset::Random {
                        SortSpec::shuffled(scope)
                    } else {
                        preset.spec(scope)
                    };
                    let enabled = spec
                        .rules
                        .first()
                        .is_none_or(|rule| available.contains(&rule.field));
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(if enabled {
                            preset.label().to_string()
                        } else {
                            format!("{} · no metadata", preset.label())
                        })
                        .checked(current.preset() == Some(*preset))
                        .disabled(!enabled)
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |app, cx| app.apply_sort(spec.clone(), cx));
                        }),
                    );
                }
                if current.is_random() {
                    let weak = weak.clone();
                    menu = menu
                        .separator()
                        .item(
                            PopupMenuItem::new("Shuffle again").on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    app.apply_sort(SortSpec::shuffled(scope), cx)
                                });
                            }),
                        );
                } else if let Some(rule) = current.rules.first() {
                    menu = menu.separator();
                    for descending in [false, true] {
                        let mut spec = current.clone();
                        spec.rules[0].descending = descending;
                        let weak = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!(
                                "{}: {}",
                                rule.field.label(),
                                rule.field.direction_label(descending)
                            ))
                            .checked(rule.descending == descending)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |app, cx| app.apply_sort(spec.clone(), cx));
                            }),
                        );
                    }
                }
                menu
            })
            .into_any_element()
    }
}
