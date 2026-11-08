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
        self.sort_anchor = self.selected_album.as_ref().map(|album| album.id.clone());
        let selected = self.selected_album.clone();
        self.request_query(cx);
        self.selected_album = selected;
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
                    let spec = preset.spec(scope);
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
                // Discography direction changes the albums within each artist.
                // All other presets change their primary sort field.
                let index = if current.preset() == Some(Preset::Discography) {
                    1
                } else {
                    0
                };
                if let Some(rule) = current.rules.get(index) {
                    menu = menu.separator();
                    for descending in [false, true] {
                        let mut spec = current.clone();
                        spec.rules[index].descending = descending;
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
