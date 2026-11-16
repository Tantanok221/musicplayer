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
        eprintln!("scenario=worker-favorite event={name}");
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
