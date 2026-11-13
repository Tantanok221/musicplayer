//! Track and album context menus in the app's compact style: an icon on every
//! row, a muted second line where it helps, and actions grouped as play →
//! queue → playlists → navigation → feedback → copy.
use super::*;
use sonora::service::PlaylistTarget;

/// A plain menu row with a leading icon.
pub(super) fn menu_row(icon: Glyph, title: impl Into<SharedString>) -> PopupMenuItem {
    PopupMenuItem::new(title).icon(Icon::new(icon))
}

/// A menu row with a leading icon and a muted line naming what it acts on.
pub(super) fn menu_row_detail(
    icon: Glyph,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> PopupMenuItem {
    let (title, detail) = (title.into(), detail.into());
    PopupMenuItem::element(move |_, cx| {
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .py(px(3.))
            .child(title.clone())
            .child(
                div()
                    .text_size(px(10.))
                    .line_height(px(13.))
                    .text_color(ink(cx).muted)
                    .truncate()
                    .child(detail.clone()),
            )
    })
    .icon(Icon::new(icon))
}

/// The menu's first row, a palette in the manner of Finder's tag dots: five
/// star targets showing the current rating. The row itself is inert, so it
/// never highlights like a command or takes keyboard focus; each star has its
/// own hover pill, the stars preview what a click sets (clicking the current
/// rating clears it, as in the rating controls), and a caption says what the
/// click will do. A click rates and closes the menu.
fn rating_row(
    rating: u8,
    noun: &'static str,
    rate: impl Fn(u8, &mut App) + 'static,
) -> PopupMenuItem {
    let rate = Rc::new(rate);
    // Menu rows are rebuilt on every frame, so the hover lives here and a
    // refresh redraws the preview.
    let hovered = Rc::new(std::cell::Cell::new(None::<u8>));
    PopupMenuItem::element(move |_, cx| {
        let colors = ink(cx);
        let hover = hovered.get();
        let shown = hover.map_or(rating, |value| rating_after_click(rating, value));
        let mut stars = div().flex().items_center().ml(px(-4.));
        for value in 1..=5u8 {
            let filled = value <= shown;
            let (cell, rate) = (hovered.clone(), rate.clone());
            stars = stars.child(
                div()
                    .id(("menu-rating", usize::from(value)))
                    .role(Role::Button)
                    .aria_label(if value == rating {
                        format!("Clear {noun} rating")
                    } else {
                        format!("Rate {noun} {value} stars")
                    })
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(hover == Some(value), |star| {
                        star.bg(colors.line.opacity(0.7))
                    })
                    .text_size(px(16.))
                    .line_height(px(16.))
                    .text_color(if filled {
                        colors.accent
                    } else {
                        colors.muted.opacity(0.7)
                    })
                    .child(if filled { "★" } else { "☆" })
                    .on_hover(move |inside, window, _| {
                        if *inside {
                            cell.set(Some(value));
                        } else if cell.get() == Some(value) {
                            cell.set(None);
                        } else {
                            return;
                        }
                        window.refresh();
                    })
                    .on_click(move |_, window, cx| {
                        rate(rating_after_click(rating, value), cx);
                        // The row is inert, so close the menu as Escape does.
                        if let Ok(escape) = Keystroke::parse("escape") {
                            window.dispatch_keystroke(escape, cx);
                        }
                    }),
            );
        }
        let caption = match hover {
            Some(value) if value == rating => "Clear rating".to_string(),
            Some(1) => "Rate 1 star".to_string(),
            Some(value) => format!("Rate {value} stars"),
            None if rating == 0 => format!("Rate {noun}"),
            None => String::new(),
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .py(px(2.))
            .child(stars)
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child(caption),
            )
    })
    .disabled(true)
}

fn shorten(text: &str) -> String {
    if text.chars().count() > 40 {
        format!("{}…", text.chars().take(39).collect::<String>().trim_end())
    } else {
        text.to_string()
    }
}

/// What every track and album menu reads from the app, captured at render.
pub(super) struct MenuState {
    app: WeakEntity<MusicApp>,
    service: Service,
    places: Rc<QueuePlaces>,
    /// Playlists the signed-in user may edit: ID and name.
    playlists: Vec<(String, String)>,
}

/// Something a menu can add to a playlist.
#[derive(Clone)]
enum PlaylistItem {
    Track(Arc<Track>),
    Album(Arc<Album>),
}

impl MusicApp {
    pub(super) fn menu_state(&self, cx: &mut Context<Self>) -> Rc<MenuState> {
        let user = self
            .account
            .as_ref()
            .map(|account| account.username.as_str());
        Rc::new(MenuState {
            app: cx.weak_entity(),
            service: self.service.clone(),
            places: self.queue_places(),
            playlists: self
                .playlists
                .iter()
                .filter(|playlist| {
                    playlist.owner.is_empty() || Some(playlist.owner.as_str()) == user
                })
                .map(|playlist| (playlist.id.clone(), playlist.name.clone()))
                .collect(),
        })
    }

    /// Opens the playlist name editor for a new playlist holding `item`.
    fn new_playlist_for(
        &mut self,
        item: PlaylistItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = match &item {
            PlaylistItem::Track(_) => String::new(),
            PlaylistItem::Album(album) => album.name.clone(),
        };
        (self.playlist_editor_tracks, self.playlist_editor_album) = match item {
            PlaylistItem::Track(track) => (Some(vec![track.id.clone()]), None),
            PlaylistItem::Album(album) => (None, Some(album.id.clone())),
        };
        self.show_playlist_editor = true;
        self.playlist_editor_rename = false;
        self.playlist_name.update(cx, |input, cx| {
            input.set_value(name, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn add_to_playlist(&mut self, item: PlaylistItem, playlist: String, cx: &mut Context<Self>) {
        match item {
            PlaylistItem::Track(track) => self.mutate(
                Mutation::AddToPlaylist(playlist, vec![track.id.clone()]),
                cx,
            ),
            PlaylistItem::Album(album) => self.service.send(Command::AlbumToPlaylist(
                album.id.clone(),
                PlaylistTarget::Existing(playlist),
            )),
        }
    }

    /// The full menu for a song row; `queue_index` is set when the row is a
    /// queue entry, so Play Now jumps there and Remove is offered.
    pub(super) fn track_menu(
        &self,
        track: Arc<Track>,
        queue_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let state = self.menu_state(cx);
        move |menu, window, cx| {
            let app = state.app.clone();
            let play = track.clone();
            let menu = track_rating(menu.min_w(px(220.)), &state, &track)
                .item(menu_row(Glyph::Play, "Play Now").on_click(move |_, _, cx| {
                    let _ = app.update(cx, |app, _| match queue_index {
                        Some(index) => app.service.player.send(PlayerCommand::JumpTo(index)),
                        None => app.play_track(&play),
                    });
                }))
                .separator();
            let service = state.service.clone();
            let id = track.id.clone();
            let menu = queue_options(menu, &state.places, move |placement| {
                service.send(Command::Enqueue(vec![id.clone()], placement))
            });
            let menu = track_extras(menu, &state, &track, window, cx);
            match queue_index {
                Some(index) => {
                    let app = state.app.clone();
                    menu.separator()
                        .item(menu_row(Glyph::ListX, "Remove from Queue").on_click(
                            move |_, _, cx| {
                                let _ = app.update(cx, |app, _| {
                                    app.service.player.send(PlayerCommand::Remove(index))
                                });
                            },
                        ))
                }
                None => menu,
            }
        }
    }

    pub(super) fn album_menu(
        &self,
        album: &Album,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let state = self.menu_state(cx);
        let album = Arc::new(album.clone());
        move |menu, window, cx| {
            let app = state.app.clone();
            let id = album.id.clone();
            let menu = menu
                .min_w(px(220.))
                .item(rating_row(
                    album.user_rating.unwrap_or(0),
                    "album",
                    move |value, cx| {
                        let id = id.clone();
                        let _ = app.update(cx, |app, cx| {
                            app.mutate(Mutation::AlbumRating(id, value), cx)
                        });
                    },
                ))
                .separator();
            let service = state.service.clone();
            let id = album.id.clone();
            let menu = menu
                .item(
                    menu_row(Glyph::Play, "Play Album")
                        .on_click(move |_, _, _| service.send(Command::PlayAlbum(id.clone()))),
                )
                .separator();
            let service = state.service.clone();
            let id = album.id.clone();
            let menu = queue_options(menu, &state.places, move |placement| {
                service.send(Command::EnqueueAlbum(id.clone(), placement))
            });
            let menu =
                playlist_submenu(menu, &state, PlaylistItem::Album(album.clone()), window, cx);
            let menu = artist_items(menu.separator(), &state, album.artist_credits(), window, cx);

            let app = state.app.clone();
            let (id, favorite) = (album.id.clone(), album.starred.is_some());
            let menu = menu.separator().item(
                menu_row(
                    if favorite {
                        Glyph::HeartOff
                    } else {
                        Glyph::Heart
                    },
                    if favorite {
                        "Remove Album Favorite"
                    } else {
                        "Favorite Album"
                    },
                )
                .on_click(move |_, _, cx| {
                    let id = id.clone();
                    let _ = app.update(cx, |app, cx| {
                        app.mutate(Mutation::AlbumFavorite(id, !favorite), cx)
                    });
                }),
            );
            copy_row(
                menu,
                &state,
                format!(
                    "{} — {}",
                    if album.display_artist.is_empty() {
                        &album.artist
                    } else {
                        &album.display_artist
                    },
                    album.name
                ),
            )
        }
    }
}

/// The song's rating row and a separator, for the top of a song menu.
pub(super) fn track_rating(
    menu: PopupMenu,
    state: &Rc<MenuState>,
    track: &Arc<Track>,
) -> PopupMenu {
    let app = state.app.clone();
    let id = track.id.clone();
    menu.item(rating_row(
        track.user_rating.unwrap_or(0),
        "song",
        move |value, cx| {
            let id = id.clone();
            let _ = app.update(cx, |app, cx| app.mutate(Mutation::Rating(id, value), cx));
        },
    ))
    .separator()
}

/// Playlists, navigation, feedback and copying for a song, for menus that
/// supply their own play and queue actions.
pub(super) fn track_extras(
    menu: PopupMenu,
    state: &Rc<MenuState>,
    track: &Arc<Track>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let menu = playlist_submenu(menu, state, PlaylistItem::Track(track.clone()), window, cx);
    let menu = track_navigation(menu, state, track, window, cx);
    let menu = track_feedback(menu, state, track);
    copy_row(
        menu,
        state,
        format!("{} — {}", track.artist_label(), track.title),
    )
}

/// Add to Playlist ▸ New Playlist… and each playlist the user can edit.
fn playlist_submenu(
    menu: PopupMenu,
    state: &Rc<MenuState>,
    item: PlaylistItem,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let state = state.clone();
    menu.separator().submenu_with_icon(
        Some(Icon::new(Glyph::ListPlus)),
        "Add to Playlist",
        window,
        cx,
        move |menu, _, _| {
            let app = state.app.clone();
            let new = item.clone();
            let mut menu = menu.min_w(px(180.)).max_h(px(360.)).scrollable(true).item(
                menu_row(Glyph::Plus, "New Playlist…").on_click(move |_, window, cx| {
                    let new = new.clone();
                    let _ = app.update(cx, |app, cx| app.new_playlist_for(new, window, cx));
                }),
            );
            if state.playlists.is_empty() {
                return menu;
            }
            menu = menu.separator();
            for (id, name) in &state.playlists {
                let app = state.app.clone();
                let (id, item) = (id.clone(), item.clone());
                menu = menu.item(menu_row(Glyph::ListMusic, shorten(name)).on_click(
                    move |_, _, cx| {
                        let (id, item) = (id.clone(), item.clone());
                        let _ = app.update(cx, |app, cx| app.add_to_playlist(item, id, cx));
                    },
                ));
            }
            menu
        },
    )
}

/// Go to Album and Go to Artist, each naming its destination.
fn track_navigation(
    menu: PopupMenu,
    state: &Rc<MenuState>,
    track: &Arc<Track>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let mut menu = menu.separator();
    if !track.album_id.is_empty() {
        let app = state.app.clone();
        let album = track.clone();
        menu = menu.item(
            menu_row_detail(Glyph::DiscAlbum, "Go to Album", shorten(&track.album)).on_click(
                move |_, window, cx| {
                    let _ = app.update(cx, |app, cx| app.open_queue_album(&album, window, cx));
                },
            ),
        );
    }
    artist_items(menu, state, track.artist_credits(), window, cx)
}

/// Go to Artist for one credit, or a submenu naming each credited artist.
fn artist_items(
    menu: PopupMenu,
    state: &Rc<MenuState>,
    credits: Vec<NamedId>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let open = |state: &Rc<MenuState>, credit: NamedId| {
        let app = state.app.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            let (name, id) = (credit.name.clone(), credit.id.clone());
            let _ = app.update(cx, |app, cx| app.open_artist(name, id, window, cx));
        }
    };
    match credits.len() {
        0 => menu,
        1 => {
            let credit = credits[0].clone();
            menu.item(
                menu_row_detail(Glyph::User, "Go to Artist", shorten(&credit.name))
                    .on_click(open(state, credit)),
            )
        }
        _ => {
            let state = state.clone();
            menu.submenu_with_icon(
                Some(Icon::new(Glyph::User)),
                "Go to Artist",
                window,
                cx,
                move |mut menu, _, _| {
                    for credit in &credits {
                        menu = menu.item(
                            menu_row(Glyph::User, shorten(&credit.name))
                                .on_click(open(&state, credit.clone())),
                        );
                    }
                    menu
                },
            )
        }
    }
}

/// Favorite or unfavorite a song.
fn track_feedback(menu: PopupMenu, state: &Rc<MenuState>, track: &Arc<Track>) -> PopupMenu {
    let app = state.app.clone();
    let (id, favorite) = (track.id.clone(), track.starred.is_some());
    menu.separator().item(
        menu_row(
            if favorite {
                Glyph::HeartOff
            } else {
                Glyph::Heart
            },
            if favorite {
                "Remove Favorite"
            } else {
                "Favorite"
            },
        )
        .on_click(move |_, _, cx| {
            let id = id.clone();
            let _ = app.update(cx, |app, cx| {
                app.mutate(Mutation::Favorite(id, !favorite), cx)
            });
        }),
    )
}

fn copy_row(menu: PopupMenu, state: &Rc<MenuState>, text: String) -> PopupMenu {
    let app = state.app.clone();
    menu.separator().item(
        menu_row_detail(Glyph::Copy, "Copy Artist and Title", shorten(&text)).on_click(
            move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                let _ = app.update(cx, |app, cx| {
                    app.status = "Copied to the clipboard".into();
                    cx.notify();
                });
            },
        ),
    )
}
