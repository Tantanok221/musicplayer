use super::*;
use crate::feedback::{optimistic_album, optimistic_track};

impl MusicApp {
    /// Replaces a track everywhere it is shown, without reloading any list.
    pub(super) fn apply_track_update(&mut self, track: Arc<Track>) {
        if let Some(data) = &mut self.artist_data {
            for value in &mut data.most_played {
                if value.id == track.id {
                    *value = track.clone();
                }
            }
        }
        for profile in self.artist_profiles.values_mut() {
            if profile.popular.iter().any(|value| value.id == track.id) {
                for value in &mut Arc::make_mut(profile).popular {
                    if value.id == track.id {
                        *value = track.clone();
                    }
                }
            }
        }
        if self.selected.as_ref().is_some_and(|t| t.id == track.id) {
            self.selected = Some(track.clone());
        }
        for value in self
            .tracks
            .iter_mut()
            .chain(self.group_tracks.values_mut().flatten())
            .chain(
                self.highlight_album
                    .iter_mut()
                    .flat_map(|(_, tracks)| tracks.iter_mut()),
            )
        {
            if value.id == track.id {
                *value = track.clone();
            }
        }
        if let Some(playlist) = &mut self.playlist {
            for value in Arc::make_mut(&mut playlist.entry) {
                if value.id == track.id {
                    *value = track.clone();
                }
            }
        }
        if self.playback.queue.iter().any(|value| value.id == track.id) {
            for value in Arc::make_mut(&mut self.playback.queue) {
                if value.id == track.id {
                    *value = track.clone();
                }
            }
        }
        if self
            .playback
            .track
            .as_ref()
            .is_some_and(|t| t.id == track.id)
        {
            self.playback.track = Some(track);
        }
    }

    pub(super) fn find_track(&self, id: &str) -> Option<Arc<Track>> {
        self.selected
            .iter()
            .chain(&self.tracks)
            .chain(self.group_tracks.values().flatten())
            .chain(self.highlight_album.iter().flat_map(|(_, tracks)| tracks))
            .chain(self.playback.queue.iter())
            .chain(
                self.playlist
                    .iter()
                    .flat_map(|playlist| playlist.entry.iter()),
            )
            .chain(self.artist_data.iter().flat_map(|data| &data.most_played))
            .chain(
                self.artist_profiles
                    .values()
                    .flat_map(|profile| &profile.popular),
            )
            .find(|track| track.id == id)
            .cloned()
    }

    pub(super) fn find_album(&self, id: &str) -> Option<Arc<Album>> {
        self.selected_album
            .iter()
            .chain(&self.view.albums)
            .chain(self.artist_albums.iter().flatten())
            .chain(self.artist_data.iter().flat_map(|data| &data.albums))
            .find(|album| album.id == id)
            .cloned()
    }

    pub(super) fn apply_album_update(&mut self, album: Arc<Album>) {
        if self
            .selected_album
            .as_ref()
            .is_some_and(|selected| selected.id == album.id)
        {
            self.selected_album = Some(album.clone());
        }
        for value in self
            .view
            .albums
            .iter_mut()
            .chain(self.artist_albums.iter_mut().flatten())
            .chain(
                self.artist_data
                    .iter_mut()
                    .flat_map(|data| &mut data.albums),
            )
        {
            if value.id == album.id {
                *value = album.clone();
            }
        }
    }

    pub(super) fn mutate(&mut self, mutation: Mutation, cx: &mut Context<Self>) {
        if !self.feedback.begin(&mutation) {
            return;
        }
        let now = chrono::Utc::now().to_rfc3339();
        if let Some(album) = mutation
            .album_feedback_id()
            .and_then(|id| self.find_album(id))
            && let Some(edited) = optimistic_album(&album, &mutation, &now)
        {
            self.apply_album_update(Arc::new(edited));
        }
        // Show ratings and favorites at once; a failed save restores the
        // catalog value through the corresponding track/album update event.
        let edited = match &mutation {
            Mutation::Favorite(id, _) | Mutation::Rating(id, _) => self
                .find_track(id)
                .and_then(|track| optimistic_track(&track, &mutation, &now)),
            _ => None,
        };
        if let Some(track) = edited.map(Arc::new) {
            self.service
                .player
                .send(PlayerCommand::UpdateTrack(track.clone()));
            self.apply_track_update(track);
        }
        self.service.send(Command::Mutate(mutation));
        cx.notify();
    }

    pub(super) fn track_feedback(
        &self,
        track: Arc<Track>,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(format!("song-feedback-{key}")))
            .test_support()
            .role(Role::Group)
            .aria_label(format!(
                "{}: {}, rating {} of 5",
                track.title,
                if track.starred.is_some() {
                    "favorite"
                } else {
                    "not favorite"
                },
                track.user_rating.unwrap_or(0)
            ))
            .flex()
            .items_center()
            .flex_shrink_0()
            .gap(px(2.))
            .child(self.favorite_button(&track, &key, cx))
            .child(self.rating_stars(&track, &key, cx))
            .into_any_element()
    }

    pub(super) fn rating_stars(&self, track: &Track, key: &str, cx: &mut Context<Self>) -> Div {
        let colors = ink(cx);
        let rating = track.user_rating.unwrap_or(0);
        let preview = self
            .rating_hover
            .as_ref()
            .filter(|(hovered, _)| hovered == key)
            .map(|(_, value)| rating_after_click(rating, *value));
        let mut stars = div().flex().items_center().flex_shrink_0();
        for value in 1..=5u8 {
            let id = track.id.clone();
            let filled = value <= preview.unwrap_or(rating);
            let color = if filled { colors.accent } else { colors.muted };
            let hover_key = key.to_string();
            stars = stars.child(
                div()
                    .id(SharedString::from(format!("rating-hover-{key}-{value}")))
                    .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
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
                        Button::new(SharedString::from(format!("rating-{key}-{value}")))
                            .ghost()
                            .compact()
                            .w(px(15.))
                            .h(px(23.))
                            .label(if filled { "★" } else { "☆" })
                            .text_color(color)
                            .tooltip(if value == rating {
                                "Clear rating".to_string()
                            } else {
                                format!("Rate {value} / 5")
                            })
                            .accessibility_label(format!("Rate {} {value} stars", track.title))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.rating_hover = None;
                                app.mutate(
                                    Mutation::Rating(id.clone(), rating_after_click(rating, value)),
                                    cx,
                                );
                            })),
                    ),
            );
        }
        stars
    }

    /// One button that opens a rating menu, for widths where five stars don't fit.
    pub(super) fn rating_menu(
        &self,
        track: &Track,
        key: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = ink(cx);
        let rating = track.user_rating.unwrap_or(0);
        let id = track.id.clone();
        let weak = cx.weak_entity();
        Button::new(SharedString::from(format!("rating-menu-{key}")))
            .ghost()
            .compact()
            .h(px(24.))
            .label(if rating == 0 {
                "☆".to_string()
            } else {
                format!("★ {rating}")
            })
            .text_color(if rating == 0 {
                colors.muted
            } else {
                colors.accent
            })
            .tooltip("Rating")
            .accessibility_label(format!("Rate {}", track.title))
            .dropdown_menu(move |mut menu, _, _| {
                for value in (0..=5u8).rev() {
                    let id = id.clone();
                    let weak = weak.clone();
                    let label = if value == 0 {
                        "No rating".to_string()
                    } else {
                        "★".repeat(value.into())
                    };
                    menu = menu.item(PopupMenuItem::new(label).checked(value == rating).on_click(
                        move |_, _, cx| {
                            let _ = weak.update(cx, |app, cx| {
                                app.mutate(Mutation::Rating(id.clone(), value), cx)
                            });
                        },
                    ));
                }
                menu
            })
    }

    pub(super) fn favorite_button(
        &self,
        track: &Track,
        key: &str,
        cx: &mut Context<Self>,
    ) -> Button {
        let colors = ink(cx);
        let starred = track.starred.is_some();
        let id = track.id.clone();
        Button::new(SharedString::from(format!("love-{key}")))
            .ghost()
            .compact()
            .w(px(23.))
            .h(px(24.))
            .label(if starred { "♥" } else { "♡" })
            .text_color(if starred {
                colors.favorite
            } else {
                colors.muted
            })
            .tooltip(if starred {
                "Remove favorite"
            } else {
                "Favorite"
            })
            .accessibility_label(format!("Favorite {}", track.title))
            .on_click(cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                app.mutate(Mutation::Favorite(id.clone(), !starred), cx);
            }))
    }
}
