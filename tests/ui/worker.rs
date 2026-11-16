//! Actual pointer handlers → real worker → loopback HTTP → visible feedback.
use crate::worker_fixture::{Fixture, Library, Request, WAIT};
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, TestAppContext, WindowBounds, WindowOptions, px, size,
    test::TestWindowExt,
};
use sonora::{
    model::Preferences,
    service::{Event, Mutation, test_support::Worker},
    ui::{MusicApp, tab_bindings},
};
use std::time::Instant;

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
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(tab_bindings());
    });
    let mut prefs = Preferences::default();
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
    let worker = fixture.start();
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
