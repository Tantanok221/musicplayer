//! Actual pointer handlers → real worker → loopback HTTP → visible feedback.
use crate::worker_fixture::{Fixture, Library, Request, WAIT};
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, TestAppContext, WindowBounds, WindowOptions, px, size,
    test::{TestQueryExt, TestWindowExt},
};
use sonora::{
    model::{FavoritesFilter, Playlist, Preferences, SavedQueue},
    playback::{PlayerCommand, test_support::ListeningPlayer},
    service::{Event, Mutation, test_support::Worker},
    ui::{MusicApp, tab_bindings},
};
use std::time::Instant;

#[gpui_kit::test]
fn deleted_playlist_ignores_late_detail_reply(cx: &mut TestAppContext) {
    let playlist = sonora::model::Playlist {
        id: "deleted".into(),
        name: "Deleted".into(),
        owner: "fixture".into(),
        ..Default::default()
    };
    let fixture = Fixture::new(Library {
        playlists: vec![playlist.clone()],
        ..Default::default()
    });
    fixture.seed();
    let (mut worker, tx, window, app) = mount_worker_with(&fixture, Preferences::default(), cx);
    deliver_until(
        &worker,
        &tx,
        window,
        cx,
        |e, _| matches!(e, Event::Status(s) if s.starts_with("Up to date")),
    );
    // Collapse Library so its contents do not overlap the playlist row.
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("library-disclosure", cx);
    })
    .unwrap();
    cx.run_until_parked();
    fixture.edit_library(|library| {
        library.hold_endpoint = Some("getPlaylist.view");
        library.playlist_response = Some(serde_json::json!({"status":"ok","playlist":playlist}));
    });
    cx.update_window(window, |_, window, cx| {
        assert!(window.find("nav-playlist-deleted").visible());
        window.click("nav-playlist-deleted", cx)
    })
    .unwrap();
    cx.run_until_parked();
    fixture.wait_request(&Request::new("getPlaylist.view", &[("id", "deleted")]));
    cx.update_window(window, |_, window, cx| window.click("playlist-options", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let mut menu = window.within("popup-menu");
        let target = menu.find_by_label("Delete playlist…");
        let id = target.path().last().unwrap().clone();
        menu.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
    fixture.expect_mutation(
        Request::new("deletePlaylist.view", &[("id", "deleted")]),
        true,
    );
    cx.update_window(window, |_, window, cx| window.click("confirm-delete", cx))
        .unwrap();
    cx.run_until_parked();
    deliver_until(
        &worker,
        &tx,
        window,
        cx,
        |e, _| matches!(e, Event::Playlists(p) if p.is_empty()),
    );
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("nav-playlist-deleted").is_none())
    })
    .unwrap();
    fixture.resume_held();
    let deadline = Instant::now() + std::time::Duration::from_secs(1);
    while let Ok(event) = worker.events().recv_deadline(deadline) {
        tx.send(event).unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("nav-playlist-deleted").is_none(),
                "deleted playlist must stay absent from sidebar"
            );
        })
        .unwrap();
    }
    assert!(fixture.catalog().playlist("deleted").unwrap().is_none());
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[gpui_kit::test]
fn remove_after_server_reorder_removes_clicked_song(cx: &mut TestAppContext) {
    let mut library = Library::default();
    library.playlists = vec![sonora::model::Playlist {
        id: "playlist".into(),
        name: "Roadtrip".into(),
        song_count: 2,
        entry: library
            .tracks
            .iter()
            .cloned()
            .map(std::sync::Arc::new)
            .collect::<Vec<_>>()
            .into(),
        ..Default::default()
    }];
    let cached = library.playlists[0].clone();
    let fixture = Fixture::new(library);
    fixture.seed();
    fixture.catalog().save_playlist(&cached).unwrap();
    let mut prefs = Preferences::default();
    prefs.tabs.active_mut().page = Some(sonora::browser::Page {
        workspace: sonora::browser::Workspace::Playlists,
        playlist: Some(("playlist".into(), "Roadtrip".into())),
        ..Default::default()
    });
    let (worker, tx, window, app) = mount_worker_with(&fixture, prefs, cx);
    wait_library(&worker, &tx, window, cx, "row-playlist-1");
    cx.run_until_parked();
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("row-playlist-0").label(),
            Some("Song song · Fixture Artist · Release album")
        );
    })
    .unwrap();
    fixture.edit_library(|library| {
        std::sync::Arc::make_mut(&mut library.playlists[0].entry).reverse()
    });
    fixture.expect_mutation(
        Request::new(
            "updatePlaylist.view",
            &[("playlistId", "playlist"), ("songIndexToRemove", "1")],
        ),
        true,
    );
    cx.update_window(window, |_, window, cx| {
        window.right_click("row-playlist-0", cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let mut menu = window.within("popup-menu");
        let target = menu.find_by_label("Remove from Playlist");
        let id = target.path().last().unwrap().clone();
        menu.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
    deliver_until(&worker, &tx, window, cx, |event, _| {
        assert!(
            !matches!(event, Event::MutationFailed(..)),
            "removing the clicked song must reach Navidrome"
        );
        matches!(event, Event::Playlist(p) if p.entry.len() == 1)
    });
    cx.update_window(window, |_, window, _| {
        assert!(window.find("row-playlist-0").visible());
        assert_eq!(
            window.find("row-playlist-0").label(),
            Some("Song other-song · Fixture Artist · Release other-album"),
            "wrong song removed by clicked Remove"
        );
        assert!(window.try_find("row-playlist-1").is_none());
    })
    .unwrap();
    assert_eq!(
        fixture
            .catalog()
            .playlist("playlist")
            .unwrap()
            .unwrap()
            .entry[0]
            .id,
        "other-song"
    );
    close_worker(worker, window, app, cx);
    fixture.finish();
}

/// Stage real events in FIFO order through MusicApp's normal receiver. This
/// lets the test inspect the optimistic frame before releasing HTTP results.
/// No event is synthesized, filtered, reordered or handled directly by the view.
fn deliver_until(
    worker: &Worker,
    ui_events: &flume::Sender<Event>,
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
    mut wanted: impl FnMut(&Event, &mut TestAppContext) -> bool,
) {
    let deadline = Instant::now() + WAIT;
    loop {
        let event = worker
            .events()
            .recv_deadline(deadline)
            .expect("real worker event");
        // Events/preferences can contain account settings: log only safe names.
        let name = match &event {
            Event::AlbumUpdated(_) => "AlbumUpdated",
            Event::MutationFailed(..) => "MutationFailed",
            Event::MutationDone(_) => "MutationDone",
            _ => "worker event",
        };
        eprintln!(
            "scenario={} event={name}",
            cx.test_function_name().unwrap_or("worker-ui")
        );
        ui_events.send(event.clone()).unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        if wanted(&event, cx) {
            return;
        }
    }
}

fn assert_favorite(
    window: AnyWindowHandle,
    favorite: bool,
    pending: bool,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, _| {
        let heart = window.find("album-favorite-album");
        assert!(heart.visible());
        assert_eq!(
            heart.label(),
            Some(if favorite {
                "Unfavorite album Release album"
            } else {
                "Favorite album Release album"
            })
        );
        assert_eq!(heart.disabled() == Some(true), pending);
    })
    .unwrap();
}

#[gpui_kit::test]
fn playlist_context_menu_fetches_tracks_and_updates_the_real_queue(cx: &mut TestAppContext) {
    let mut library = Library::default();
    let track = std::sync::Arc::new(library.tracks[1].clone());
    library.playlists = vec![Playlist {
        id: "roadtrip".into(),
        name: "Roadtrip".into(),
        song_count: 1,
        entry: vec![track.clone()].into(),
        ..Default::default()
    }];
    let fixture = Fixture::new(library);
    fixture.seed();
    let mut prefs = Preferences::default();
    prefs.layout.navigator_height = 300.;
    let (worker, tx, window, app) = mount_worker_with_player(&fixture, prefs, true, cx);
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    cx.update_window(window, |_, window, cx| {
        window.right_click("nav-playlist-roadtrip", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let mut menu = window.within("popup-menu");
        let item = menu.find_by_label("Add to End of Queue");
        menu.click(item.path().last().unwrap().clone(), cx);
    })
    .unwrap();
    fixture.wait_request(&Request::new("getPlaylist.view", &[("id", "roadtrip")]));
    deliver_until(
        &worker,
        &tx,
        window,
        cx,
        |event, _| matches!(event, Event::Playback(state) if state.queue.len() == 1 && state.queue[0].id == track.id),
    );
    cx.update_window(window, |_, window, cx| {
        assert!(window.find("explorer-album-album").visible());
        assert!(window.try_find("popup-menu").is_none());
        window.click("nav-queue", cx);
        assert!(
            window
                .find_by_label(&format!(
                    "Queue 1: {} · {}",
                    track.title,
                    track.artist_label()
                ))
                .visible()
        );
    })
    .unwrap();
    close_worker(worker, window, app, cx);
    fixture.finish();
}

#[gpui_kit::test]
fn create_smart_playlist_reaches_native_api_and_selects_saved_playlist(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(tab_bindings());
    });
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let mut worker = fixture.start();
    let (ui_events, received) = flume::unbounded();
    let (window, app) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1440.), px(900.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| MusicApp::new(worker.service().clone(), received, window, cx)),
        )
        .unwrap()
    });
    let mut native = false;
    deliver_until(&worker, &ui_events, window, cx, |e, _| {
        native |= matches!(e, Event::NativeAvailable(true));
        native && matches!(e,Event::Status(s) if s.starts_with("Up to date"))
    });
    cx.update_window(window, |_, window, cx| {
        window.click("new-playlist", cx);
        window.click("playlist-type-smart", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.click("smart-template-3", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.click("create-playlist", cx);
    })
    .unwrap();
    deliver_until(&worker, &ui_events, window, cx, |e, _| {
        matches!(e, Event::NativePasswordRequired)
    });
    cx.update_window(window, |_, window, cx| {
        window.click("native-password", cx);
        window.input("native-password", cx);
        window.press("enter", cx);
    })
    .unwrap();
    deliver_until(
        &worker,
        &ui_events,
        window,
        cx,
        |e, _| matches!(e,Event::Playlist(p) if p.id=="smart" && p.entry.len()==1),
    );
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("playlist-creation").is_none());
        assert!(window.find("smart-playlist-header").visible());
    })
    .unwrap();
    let requests = fixture.native_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, axum::http::Method::POST);
    assert_eq!(requests[0].2["name"], "Loved");
    assert_eq!(
        requests[0].2["rules"],
        serde_json::json!({"all":[{"is":{"loved":true}}],"sort":"dateloved","order":"desc","limit":100})
    );
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[gpui_kit::test]
fn password_sign_in_then_create_smart_playlist_needs_no_dialog(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Library::default());
    std::fs::remove_file(fixture.paths.data.join("account.json")).unwrap();
    let (worker, tx, window, app) = mount_worker(&fixture, cx);
    deliver_until(&worker, &tx, window, cx, |e, _| {
        matches!(e, Event::SignInRequired(_))
    });
    cx.update_window(window, |_, window, cx| {
        for (field, value) in [
            ("Server address", fixture.account.url.as_str()),
            ("Username", "fixture"),
            ("Password", "native-password"),
        ] {
            window.click(field, cx);
            window.input(value, cx);
        }
        window.click("connect-library", cx);
    })
    .unwrap();
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    for id in [
        "new-playlist",
        "playlist-type-smart",
        "smart-template-3",
        "create-playlist",
    ] {
        cx.update_window(window, |_, window, cx| window.click(id, cx))
            .unwrap();
        cx.run_until_parked();
    }
    deliver_until(&worker, &tx, window, cx, |e, _| {
        assert!(
            !matches!(e, Event::NativePasswordRequired),
            "sign-in must authorize smart playlist editing"
        );
        matches!(e,Event::Playlist(p) if p.id=="smart" && p.entry.len()==1)
    });
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("native-password").is_none());
        assert!(window.try_find("playlist-creation").is_none());
        assert!(window.find("smart-playlist-header").visible());
    })
    .unwrap();
    assert_eq!(fixture.native_login_count(), 1);
    assert_eq!(fixture.native_requests()[0].0, axum::http::Method::POST);
    close_worker(worker, window, app, cx);
    fixture.finish();
}

#[gpui_kit::test]
fn favorite_http_failure_rolls_back_in_the_real_view_and_retry_recovers(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(tab_bindings());
    });
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let mut prefs = Preferences::default();
    prefs.tabs.active = 0;
    prefs.layout.show_spectrum = false;
    fixture.catalog().set_state("preferences", &prefs).unwrap();
    let mut worker = fixture.start();
    let (ui_events, received_events) = flume::unbounded();
    let (window, app) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1440.), px(900.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| MusicApp::new(worker.service().clone(), received_events, window, cx))
            },
        )
        .unwrap()
    });
    let mut synced = false;
    deliver_until(&worker, &ui_events, window, cx, |event, cx| {
        synced |= matches!(event, Event::Status(message) if message.starts_with("Up to date"));
        synced
            && cx
                .update_window(window, |_, window, _| {
                    window
                        .try_find("explorer-album-album")
                        .is_some_and(|card| card.visible())
                })
                .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
        window.click("explorer-album-album", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_favorite(window, false, false, cx);

    let request = Request::new("star.view", &[("albumId", "album")]);
    fixture.expect_mutation(request.clone(), false);
    cx.update_window(window, |_, window, cx| {
        window.click("album-favorite-album", cx)
    })
    .unwrap();
    assert_favorite(window, true, true, cx);
    // Pending controls must reject actual repeated pointer input.
    cx.update_window(window, |_, window, cx| {
        window.click("album-favorite-album", cx)
    })
    .unwrap();
    deliver_until(
        &worker,
        &ui_events,
        window,
        cx,
        |event, _| matches!(event, Event::MutationFailed(Mutation::AlbumFavorite(id, true), _) if id == "album"),
    );
    assert_favorite(window, false, false, cx);
    assert!(
        fixture
            .catalog()
            .album("album")
            .unwrap()
            .unwrap()
            .starred
            .is_none()
    );
    cx.update_window(window, |_, window, _| {
        let error = window.find("error-banner");
        assert!(error.visible());
        assert_eq!(error.role(), Some(gpui_kit::Role::Alert));
        assert!(error.label().unwrap().contains("Fixture feedback rejected"));
        assert!(window.find("retry-error").visible());
    })
    .unwrap();
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|seen| **seen == request)
            .count(),
        1
    );

    fixture.expect_mutation(request.clone(), true);
    cx.update_window(window, |_, window, cx| window.click("retry-error", cx))
        .unwrap();
    assert_favorite(window, true, true, cx);
    deliver_until(
        &worker,
        &ui_events,
        window,
        cx,
        |event, _| matches!(event, Event::MutationDone(Mutation::AlbumFavorite(id, true)) if id == "album"),
    );
    assert_favorite(window, true, false, cx);
    assert!(
        fixture
            .catalog()
            .album("album")
            .unwrap()
            .unwrap()
            .starred
            .is_some()
    );
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("error-banner").is_none());
    })
    .unwrap();
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|seen| **seen == request)
            .count(),
        2
    );

    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    cx.run_until_parked();
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

fn mount_worker(
    fixture: &Fixture,
    cx: &mut TestAppContext,
) -> (
    Worker,
    flume::Sender<Event>,
    AnyWindowHandle,
    gpui_kit::Entity<MusicApp>,
) {
    mount_worker_with(fixture, Preferences::default(), cx)
}

fn mount_worker_with(
    fixture: &Fixture,
    prefs: Preferences,
    cx: &mut TestAppContext,
) -> (
    Worker,
    flume::Sender<Event>,
    AnyWindowHandle,
    gpui_kit::Entity<MusicApp>,
) {
    mount_worker_with_player(fixture, prefs, false, cx)
}

fn mount_worker_with_player(
    fixture: &Fixture,
    mut prefs: Preferences,
    paused: bool,
    cx: &mut TestAppContext,
) -> (
    Worker,
    flume::Sender<Event>,
    AnyWindowHandle,
    gpui_kit::Entity<MusicApp>,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(tab_bindings());
    });
    prefs.tabs.active = 0;
    prefs.layout.show_spectrum = false;
    let path = fixture
        .paths
        .data
        .join(if fixture.paths.account().unwrap().is_some() {
            format!("{}.sqlite", fixture.account.namespace())
        } else {
            "unconnected.sqlite".into()
        });
    sonora::catalog::Catalog::open(&path)
        .unwrap()
        .set_state("preferences", &prefs)
        .unwrap();
    let worker = if paused {
        fixture.start_paused()
    } else {
        fixture.start()
    };
    let (tx, rx) = flume::unbounded();
    let (window, app) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1440.), px(900.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| MusicApp::new(worker.service().clone(), rx, window, cx)),
        )
        .unwrap()
    });
    (worker, tx, window, app)
}

#[gpui_kit::test]
fn previous_after_restore_restarts_current_track(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    fixture
        .catalog()
        .set_state(
            "queue",
            &SavedQueue {
                ids: vec!["song".into(), "other-song".into()],
                index: Some(1),
                position: 60.,
                ..Default::default()
            },
        )
        .unwrap();
    let (worker, tx, window, app) =
        mount_worker_with_player(&fixture, Preferences::default(), true, cx);
    deliver_until(
        &worker,
        &tx,
        window,
        cx,
        |event, _| matches!(event, Event::Playback(state) if state.index == Some(1) && state.position == 60. && !state.playing),
    );
    cx.update_window(window, |_, window, cx| {
        assert_eq!(window.find("playback-elapsed").label(), Some("1:00"));
        window.click("previous", cx);
    })
    .unwrap();
    deliver_until(&worker, &tx, window, cx, |event, _| {
        if let Event::Playback(state) = event
            && state.position == 0.
        {
            assert_eq!(state.index, Some(1));
            assert_eq!(state.queue[1].id, "other-song");
            return true;
        }
        false
    });
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("playback-elapsed").label(), Some("0:00"));
    })
    .unwrap();
    close_worker(worker, window, app, cx);
    fixture.finish();
}

fn wait_library(
    worker: &Worker,
    tx: &flume::Sender<Event>,
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
    album: &'static str,
) {
    let mut synced = false;
    deliver_until(worker, tx, window, cx, |event, cx| {
        synced |= matches!(event, Event::Status(s) if s.starts_with("Up to date"));
        synced
            && cx
                .update_window(window, |_, window, _| {
                    window.try_find(album).is_some_and(|e| e.visible())
                })
                .unwrap()
    });
}

fn close_worker(
    mut worker: Worker,
    window: AnyWindowHandle,
    app: gpui_kit::Entity<MusicApp>,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    cx.run_until_parked();
    worker.shutdown(WAIT).unwrap();
}

#[gpui_kit::test]
fn switching_lastfm_user_during_import_updates_memory_lane(cx: &mut TestAppContext) {
    cx.update(|cx| cx.bind_keys(sonora::ui::settings_bindings()));
    let registered = chrono::Utc::now() - chrono::Duration::days(500);
    let spans = sonora::discover::seasons_between(registered, chrono::Utc::now());
    let fixture = Fixture::new(Library {
        hold_lastfm: true,
        lastfm_registered: registered.timestamp(),
        lastfm_charts: vec![(spans[0].from, vec![("Fixture Artist", "Release album", 30)])],
        ..Library::default()
    });
    let (worker, tx, window, app) = mount_worker_with(
        &fixture,
        Preferences {
            lastfm_user: "listener".into(),
            ..Default::default()
        },
        cx,
    );
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    cx.update_window(window, |_, window, cx| window.click("nav-discover", cx))
        .unwrap();
    deliver_until(&worker, &tx, window, cx, |e, _| {
        matches!(e, Event::Discover(..))
    });
    fixture.wait_request(&Request::new(
        "lastfm/chart",
        &[
            ("from", &spans[0].from.to_string()),
            ("to", &spans[0].to.to_string()),
        ],
    ));
    fixture.edit_library(|library| library.lastfm_charts = vec![]);
    cx.update_window(window, |_, window, cx| {
        window.press(sonora::platform::shortcut("cmd-,", "ctrl-,"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("Last.fm username", cx);
        window.press(sonora::platform::shortcut("cmd-a", "ctrl-a"), cx);
        window.input("listener-new", cx);
        window.click("save-lastfm", cx);
        window.press(sonora::platform::shortcut("cmd-,", "ctrl-,"), cx);
    })
    .unwrap();
    deliver_until(&worker, &tx, window, cx, |e, cx| {
        if let Event::Discover(_, found) = e {
            assert!(
                matches!(
                    found.history,
                    sonora::discover::HistoryStatus::Loading { done: 0, total: 0 }
                ),
                "new user's history must be Loading while the old import is in flight"
            );
            cx.update_window(window, |_, window, _| {
                assert_eq!(
                    window.find("discover-history").label(),
                    Some("Reading your Last.fm history…")
                )
            })
            .unwrap();
            true
        } else {
            false
        }
    });
    deliver_until(&worker, &tx, window, cx, |e, cx| {
        if matches!(e, Event::Discover(_, found) if found.history == sonora::discover::HistoryStatus::Ready)
        {
            cx.update_window(window, |_, window, _| {
                assert!(window.find("discover-empty").visible())
            })
            .unwrap();
            true
        } else {
            false
        }
    });
    fixture.resume_lastfm();
    deliver_until(&worker, &tx, window, cx, |e, cx| {
        if let Event::Discover(_, found) = e {
            assert_eq!(found.history, sonora::discover::HistoryStatus::Ready);
            assert!(
                found.lane.is_empty(),
                "old user's late charts must stay isolated"
            );
            cx.update_window(window, |_, window, _| {
                assert!(window.find("discover-empty").visible())
            })
            .unwrap();
            let old: Option<sonora::discover::History> = fixture
                .catalog()
                .state("lastfm-history:v1:listener")
                .unwrap();
            old.is_some_and(|h| h.seasons.len() == spans.len())
        } else {
            false
        }
    });
    close_worker(worker, window, app, cx);
    fixture.finish();
}

#[gpui_kit::test]
fn play_after_enqueue_empty_from_menu_starts_in_footer(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Library::default());
    let (worker, tx, window, app) = mount_worker_with(&fixture, Preferences::default(), cx);
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    let mut player = ListeningPlayer::start(WAIT).unwrap();
    // The worker owns enqueue resolution; the scripted decoder avoids native audio.
    for command in worker.player_commands().try_iter() {
        if matches!(command, PlayerCommand::Preferences(_)) {
            player.command(command).unwrap();
        }
    }
    cx.update_window(window, |_, window, cx| {
        window.right_click("explorer-album-album", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let mut menu = window.within("popup-menu");
        let target = menu.find_by_label("Play Next");
        let id = target.path().last().unwrap().clone();
        menu.click(id, cx);
    })
    .unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        let command = worker.player_commands().recv_deadline(deadline).unwrap();
        if matches!(command, PlayerCommand::Insert(..)) {
            player.command(command).unwrap();
            break;
        }
    }
    let queued = player.snapshot(WAIT).unwrap();
    assert_eq!(queued.queue.len(), 1);
    assert!(!queued.playing);
    tx.send(Event::Playback(queued)).unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.click("play-pause", cx))
        .unwrap();
    let command = worker.player_commands().recv_timeout(WAIT).unwrap();
    assert!(matches!(command, PlayerCommand::Toggle));
    player
        .restart(command, WAIT)
        .expect("footer Play must start queued song");
    let playing = player.snapshot(WAIT).unwrap();
    assert!(playing.playing);
    assert_eq!(playing.track.as_ref().unwrap().id, "song");
    tx.send(Event::Playback(playing)).unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("play-pause").label(), Some("Pause"));
        assert!(window.find("play-pause").visible());
    })
    .unwrap();
    close_worker(worker, window, app, cx);
    fixture.finish();
}

#[gpui_kit::test]
fn empty_playlist_play_preserves_queued_track(cx: &mut TestAppContext) {
    let mut library = Library::default();
    library.playlists.push(sonora::model::Playlist {
        id: "empty".into(),
        name: "Empty".into(),
        owner: "fixture".into(),
        ..Default::default()
    });
    let fixture = Fixture::new(library);
    fixture.seed();
    fixture
        .catalog()
        .set_state(
            "queue",
            &SavedQueue {
                ids: vec!["song".into()],
                index: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    let mut prefs = Preferences::default();
    prefs.tabs.tabs[0].page = Some(sonora::browser::Page {
        workspace: sonora::browser::Workspace::Playlists,
        playlist: Some(("empty".into(), "Empty".into())),
        ..Default::default()
    });
    let (worker, tx, window, app) = mount_worker_with(&fixture, prefs, cx);
    let mut loaded = false;
    let mut synced = false;
    deliver_until(&worker, &tx, window, cx, |event, _| {
        loaded |= matches!(event, Event::Playlist(p) if p.id == "empty");
        synced |= matches!(event, Event::Status(s) if s.starts_with("Up to date"));
        loaded && synced
    });
    let player = ListeningPlayer::start(WAIT).unwrap();
    for command in worker.player_commands().try_iter() {
        player.command(command).unwrap();
    }
    let before = player.snapshot(WAIT).unwrap();
    assert_eq!(before.queue[0].id, "song");
    tx.send(Event::Playback(before.clone())).unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.click("play-playlist", cx))
        .unwrap();
    cx.run_until_parked();
    for command in worker.player_commands().try_iter() {
        player.command(command).unwrap();
    }
    let after = player.snapshot(WAIT).unwrap();
    assert_eq!(
        after.queue.len(),
        1,
        "empty playlist Play must preserve the queue"
    );
    assert_eq!(after.queue[0].id, "song");
    assert_eq!(
        after.track.as_ref().map(|t| &t.id),
        before.track.as_ref().map(|t| &t.id)
    );
    assert_eq!(after.index, before.index);
    assert_eq!(after.position, before.position);
    assert_eq!(after.playing, before.playing);
    tx.send(Event::Playback(after)).unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, _| {
        assert!(window.find("play-pause").visible())
    })
    .unwrap();
    close_worker(worker, window, app, cx);
    fixture.finish();
}

#[gpui_kit::test]
fn status_bar_shows_library_totals_while_search_narrows_the_explorer(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Library::default());
    let (worker, tx, window, app) = mount_worker(&fixture, cx);
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    let totals = "Up to date · 2 tracks · 2 albums";
    cx.update_window(window, |_, window, _| {
        let label = window.find("library-status").label().unwrap().to_string();
        assert!(label.starts_with(totals), "{label}");
    })
    .unwrap();

    cx.update_window(window, |_, window, cx| {
        window.click("music-search", cx);
        window.input("other", cx);
    })
    .unwrap();
    deliver_until(&worker, &tx, window, cx, |event, cx| {
        matches!(event, Event::View(..))
            && cx
                .update_window(window, |_, window, _| {
                    window
                        .try_find("explorer-album-album")
                        .is_none_or(|card| !card.visible())
                })
                .unwrap()
    });
    cx.update_window(window, |_, window, _| {
        assert!(window.find("explorer-album-other-album").visible());
        let label = window.find("library-status").label().unwrap().to_string();
        // The filtered page has 1 track and 1 album; the bar keeps the totals.
        assert!(label.starts_with(totals), "{label}");
        assert!(!label.contains("1 tracks"), "{label}");
        assert!(!label.contains('|'), "{label}");
    })
    .unwrap();
    close_worker(worker, window, app, cx);
    fixture.finish();
}

#[gpui_kit::test]
fn refresh_click_updates_status_and_explorer_without_restart(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Library::default());
    let (worker, tx, window, app) = mount_worker(&fixture, cx);
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    fixture.edit_library(|l| {
        l.albums.push(crate::worker_fixture::album("new"));
        l.tracks
            .push(crate::worker_fixture::song("new-song", "new"));
    });
    cx.update_window(window, |_, window, cx| window.click("refresh-library", cx))
        .unwrap();
    deliver_until(
        &worker,
        &tx,
        window,
        cx,
        |e, _| matches!(e, Event::Status(s) if s.starts_with("Syncing")),
    );
    cx.update_window(window, |_, window, _| {
        assert!(
            window
                .find("library-status")
                .label()
                .unwrap()
                .contains("Syncing")
        )
    })
    .unwrap();
    wait_library(&worker, &tx, window, cx, "explorer-album-new");
    assert_eq!(fixture.catalog().count().unwrap(), 3);
    close_worker(worker, window, app, cx);
    fixture.finish();
}

#[gpui_kit::test]
fn offline_indicator_keeps_cached_music_browsable_and_playable_then_retry_resyncs(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new(Library::default());
    let (worker, tx, window, app) = mount_worker(&fixture, cx);
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    fixture.edit_library(|l| l.offline = true);
    worker
        .service()
        .send(sonora::service::Command::CheckConnection(true));
    deliver_until(&worker, &tx, window, cx, |e, _| {
        matches!(
            e,
            Event::Connection(sonora::service::Connection::Offline(_))
        )
    });
    cx.update_window(window, |_, window, cx| {
        assert!(window.find("server-offline").visible());
        assert!(window.find("explorer-album-album").visible());
        window.double_click("explorer-album-album", cx);
    })
    .unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        if let sonora::playback::PlayerCommand::Replace(tracks, _) =
            worker.player_commands().recv_deadline(deadline).unwrap()
        {
            assert_eq!(tracks[0].album_id, "album");
            break;
        }
    }
    fixture.edit_library(|l| {
        l.offline = false;
        l.albums.push(crate::worker_fixture::album("new"));
        l.tracks
            .push(crate::worker_fixture::song("new-song", "new"));
    });
    cx.update_window(window, |_, window, cx| window.click("server-offline", cx))
        .unwrap();
    wait_library(&worker, &tx, window, cx, "explorer-album-new");
    cx.update_window(window, |_, window, _| {
        assert!(window.find("server-online").visible());
        assert!(window.try_find("server-offline").is_none());
    })
    .unwrap();
    close_worker(worker, window, app, cx);
    fixture.finish();
}

#[gpui_kit::test]
fn sign_in_fields_and_connect_reach_the_real_worker(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Library::default());
    std::fs::remove_file(fixture.paths.data.join("account.json")).unwrap();
    let (worker, tx, window, app) = mount_worker(&fixture, cx);
    deliver_until(&worker, &tx, window, cx, |e, _| {
        matches!(e, Event::SignInRequired(_))
    });
    cx.update_window(window, |_, window, cx| {
        for (field, value) in [
            ("Server address", fixture.account.url.as_str()),
            ("Username", "fixture"),
            ("Password", "synthetic-password"),
        ] {
            window.click(field, cx);
            window.input(value, cx);
        }
        window.click("connect-library", cx);
        assert_eq!(window.find("connect-library").disabled(), Some(true));
    })
    .unwrap();
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    assert_eq!(
        fixture.paths.account().unwrap().unwrap().namespace(),
        fixture.account.namespace()
    );
    assert!(
        fixture
            .paths
            .credentials(&fixture.account)
            .unwrap()
            .is_some()
    );
    assert_eq!(fixture.catalog().count().unwrap(), 2);
    close_worker(worker, window, app, cx);
    fixture.finish();
}

#[gpui_kit::test]
fn invalid_sign_in_returns_to_an_enabled_form_and_valid_retry_connects(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Library::default());
    std::fs::remove_file(fixture.paths.data.join("account.json")).unwrap();
    let (worker, tx, window, app) = mount_worker(&fixture, cx);
    deliver_until(&worker, &tx, window, cx, |e, _| {
        matches!(e, Event::SignInRequired(_))
    });
    cx.update_window(window, |_, window, cx| window.click("connect-library", cx))
        .unwrap();
    deliver_until(&worker, &tx, window, cx, |e, _| {
        matches!(e, Event::SignInFailed(_))
    });
    cx.update_window(window, |_, window, cx| {
        assert_ne!(window.find("connect-library").disabled(), Some(true));
        assert!(window.find("error-banner").visible());
        assert!(
            fixture.requests().is_empty(),
            "invalid credentials must fail before HTTP"
        );
        for (field, value) in [
            ("Server address", fixture.account.url.as_str()),
            ("Username", "fixture"),
            ("Password", "synthetic-password"),
        ] {
            window.click(field, cx);
            window.input(value, cx);
        }
        window.click("retry-error", cx);
    })
    .unwrap();
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    assert!(fixture.paths.account().unwrap().is_some());
    close_worker(worker, window, app, cx);
    fixture.finish();
}

/// The Favorites page lists an album for its hearted songs, so a double-click
/// plays those songs rather than the whole album.
#[gpui_kit::test]
fn favorites_album_double_click_plays_only_the_favorite_songs(cx: &mut TestAppContext) {
    let mut library = Library::default();
    let mut hearted = crate::worker_fixture::song("hearted", "album");
    hearted.starred = Some("2026-01-01T00:00:00Z".into());
    library.tracks.push(hearted);
    let fixture = Fixture::new(library);
    let (worker, tx, window, app) = mount_worker(&fixture, cx);
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    cx.update_window(window, |_, window, cx| {
        if window.try_find("nav-favorites").is_none() {
            window.click("library-disclosure", cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.click("nav-favorites", cx))
        .unwrap();
    deliver_until(&worker, &tx, window, cx, |_, cx| {
        cx.update_window(window, |_, window, _| {
            window.try_find("explorer-album-other-album").is_none()
                && window
                    .try_find("explorer-album-album")
                    .is_some_and(|card| card.visible())
        })
        .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
        window.double_click("explorer-album-album", cx)
    })
    .unwrap();
    let deadline = Instant::now() + WAIT;
    let played = loop {
        cx.run_until_parked();
        if let sonora::playback::PlayerCommand::Replace(tracks, at) =
            worker.player_commands().recv_deadline(deadline).unwrap()
        {
            assert_eq!(at, 0);
            break tracks
                .iter()
                .map(|track| track.id.clone())
                .collect::<Vec<_>>();
        }
    };
    assert_eq!(played, ["hearted"]);
    close_worker(worker, window, app, cx);
    fixture.finish();
}

/// Rating an album below the Favorites minimum rating drops it from the page
/// even though the page is sorted by artist, not rating.
#[gpui_kit::test]
fn favorites_drop_an_album_rated_below_the_minimum_rating(cx: &mut TestAppContext) {
    let mut library = Library::default();
    library.albums[0].user_rating = Some(4);
    let fixture = Fixture::new(library);
    let prefs = Preferences {
        favorites: FavoritesFilter {
            hearted: false,
            min_rating: 4,
            match_all: false,
        },
        ..Default::default()
    };
    let (worker, tx, window, app) = mount_worker_with(&fixture, prefs, cx);
    wait_library(&worker, &tx, window, cx, "explorer-album-album");
    cx.update_window(window, |_, window, cx| {
        if window.try_find("nav-favorites").is_none() {
            window.click("library-disclosure", cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.click("nav-favorites", cx))
        .unwrap();
    deliver_until(&worker, &tx, window, cx, |_, cx| {
        cx.update_window(window, |_, window, _| {
            window.try_find("explorer-album-other-album").is_none()
                && window
                    .try_find("explorer-album-album")
                    .is_some_and(|card| card.visible())
        })
        .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
        window.click("explorer-album-album", cx)
    })
    .unwrap();
    cx.run_until_parked();

    fixture.expect_mutation(
        Request::new("setRating.view", &[("id", "album"), ("rating", "1")]),
        true,
    );
    cx.update_window(window, |_, window, cx| {
        window.click("album-rating-album-1", cx)
    })
    .unwrap();
    deliver_until(&worker, &tx, window, cx, |_, cx| {
        cx.update_window(window, |_, window, _| {
            window.try_find("explorer-album-album").is_none()
        })
        .unwrap()
    });
    close_worker(worker, window, app, cx);
    fixture.finish();
}
