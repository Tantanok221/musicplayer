use super::*;

impl MusicApp {
    pub(super) fn album_favorite(&self, album: &Album, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let busy = self.album_pending.contains(&album.id);
        let favorite = album.starred.is_some();
        let id = album.id.clone();
        div()
            .flex_shrink_0()
            .child(
                Button::new(SharedString::from(format!("album-favorite-{}", album.id)))
                    .ghost()
                    .compact()
                    .w(px(23.))
                    .h(px(24.))
                    .label(if favorite { "♥" } else { "♡" })
                    .text_color(if favorite {
                        colors.favorite
                    } else {
                        colors.muted
                    })
                    .disabled(busy)
                    .tooltip(if favorite {
                        "Remove album favorite"
                    } else {
                        "Favorite album"
                    })
                    .accessibility_label(format!(
                        "{} album {}",
                        if favorite { "Unfavorite" } else { "Favorite" },
                        album.name
                    ))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.mutate(Mutation::AlbumFavorite(id.clone(), !favorite), cx);
                    })),
            )
            .into_any_element()
    }

    pub(super) fn album_rating(&self, album: &Album, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let busy = self.album_pending.contains(&album.id);
        let rating = album.user_rating.unwrap_or(0);
        let key = format!("album-feedback-{}", album.id);
        let preview = self
            .rating_hover
            .as_ref()
            .filter(|(hovered, _)| hovered == &key)
            .map(|(_, value)| rating_after_click(rating, *value));
        let feedback = div()
            .id(SharedString::from(key.clone()))
            .role(Role::Group)
            .aria_label(format!("Album rating: {}", album.name))
            .w_full()
            .h(px(24.))
            .flex()
            .items_center()
            .justify_center();
        let mut stars = div().flex().items_center().flex_shrink_0();
        for value in 1..=5u8 {
            let id = album.id.clone();
            let hover_key = key.clone();
            let filled = value <= preview.unwrap_or(rating);
            stars = stars.child(
                div()
                    .id(SharedString::from(format!(
                        "album-rating-hover-{}-{value}",
                        album.id
                    )))
                    .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
                        if busy {
                            return;
                        }
                        let entry = (hover_key.clone(), value);
                        if *hovered {
                            app.rating_hover = Some(entry);
                        } else if app.rating_hover.as_ref() == Some(&entry) {
                            app.rating_hover = None;
                        } else {
                            return;
                        }
                        cx.notify();
                    }))
                    .child(
                        Button::new(SharedString::from(format!(
                            "album-rating-{}-{value}",
                            album.id
                        )))
                        .ghost()
                        .compact()
                        .w(px(15.))
                        .h(px(23.))
                        .label(if filled { "★" } else { "☆" })
                        .text_color(if filled { colors.accent } else { colors.muted })
                        .disabled(busy)
                        .tooltip(if value == rating {
                            "Clear album rating".into()
                        } else {
                            format!("Rate album {value} / 5")
                        })
                        .accessibility_label(if value == rating {
                            format!("Clear album {} rating", album.name)
                        } else {
                            format!("Rate album {} {value} stars", album.name)
                        })
                        .on_click(cx.listener(move |app, _, _, cx| {
                            cx.stop_propagation();
                            app.rating_hover = None;
                            app.mutate(
                                Mutation::AlbumRating(
                                    id.clone(),
                                    rating_after_click(rating, value),
                                ),
                                cx,
                            );
                        })),
                    ),
            );
        }
        feedback.child(stars).into_any_element()
    }
}
