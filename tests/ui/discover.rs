use crate::common::ui::{Fixture, PlaybackIntent, start};
use gpui_kit::{AnyWindowHandle, AppContext, Entity, TestAppContext, test::TestWindowExt};
use sonora::{
    discover::{Dismissal, Pick, Reason, Rediscover, Shelf},
    model::Album,
    playback::Placement,
    service::{Command, Event},
    ui::MusicApp,
};
use std::sync::Arc;

const EXTRA: usize = 10;

fn album(fixture: &Fixture, id: &str) -> Arc<Album> {
    fixture
        .catalog
        .view(&Default::default())
        .unwrap()
        .albums
        .iter()
        .find(|album| album.id == id)
        .cloned()
        .unwrap_or_else(|| {
            Arc::new(Album {
                id: id.into(),
                name: format!("Extra {id}"),
                artist: "Extra Artist".into(),
                ..Default::default()
            })
        })
}

fn pick(fixture: &Fixture, id: &str) -> Pick {
    Pick {
        album: album(fixture, id),
        reason: Reason::Favorite {
            loved_songs: 3,
            played: None,
        },
    }
}

fn shelf(fixture: &Fixture, ranked: &[&str], today: &[&str]) -> Shelf {
    Shelf {
        ranked: ranked.iter().map(|id| pick(fixture, id)).collect(),
        today: today.iter().map(|id| pick(fixture, id)).collect(),
    }
}

fn reply(fixture: &mut Fixture, label: &'static str, shelves: Rediscover) {
    fixture.expect(label, move |command, _| {
        let Command::Discover(request) = command else {
            panic!("expected a Discover request")
        };
        vec![
            Event::Discover(request, Arc::new(shelves)),
            Event::Discover(request.wrapping_sub(1), Arc::default()),
        ]
    });
}

fn active_title(
    window: AnyWindowHandle,
    app: &Entity<MusicApp>,
    cx: &mut TestAppContext,
) -> String {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        app.read(cx).tab_state().active().title()
    })
    .unwrap()
}

fn shown_extras(window: AnyWindowHandle, cx: &mut TestAppContext) -> usize {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        (0..EXTRA)
            .filter(|ix| {
                window
                    .try_find(format!("discover-album-forgotten-favorites-extra-{ix}"))
                    .is_some()
            })
            .count()
    })
    .unwrap()
}

#[gpui_kit::test]
fn discover_grid_selects_like_explorer_explains_picks_and_expands(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.capture_playback();
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    let extras = (0..EXTRA)
        .map(|ix| format!("extra-{ix}"))
        .collect::<Vec<_>>();
    let mut ranked = vec!["album-3", "album-1"];
    ranked.extend(extras.iter().map(String::as_str));
    let mut today = ranked.clone();
    today.reverse();
    let shelves = Rediscover {
        old_rotation: Shelf {
            ranked: vec![Pick {
                album: album(&fixture, "album-2"),
                reason: Reason::Rotation {
                    plays: 44,
                    played: "2026-03-10T00:00:00Z".parse().unwrap(),
                },
            }],
            today: vec![],
        },
        forgotten_favorites: shelf(&fixture, &ranked, &today),
    };
    let rotation_reason = shelves.old_rotation.ranked[0].reason_text(chrono::Utc::now());
    let mut rotation = shelves;
    rotation.old_rotation.today = rotation.old_rotation.ranked.clone();
    reply(&mut fixture, "open Discover", rotation);
    fixture.click(window, "nav-discover", cx);
    fixture.diagnose("discover shelves", window, &app, cx);
    assert_eq!(active_title(window, &app, cx), "Discover");
    cx.update_window(window, |_, window, _| {
        assert!(window.find("discover-shelf-old-rotation").visible());
        assert_eq!(
            window.find("discover-reason-old-rotation-album-2").label(),
            Some(rotation_reason.as_str())
        );
        let first = window
            .find(format!(
                "discover-album-forgotten-favorites-extra-{}",
                EXTRA - 1
            ))
            .bounds();
        let second = window
            .find(format!(
                "discover-album-forgotten-favorites-extra-{}",
                EXTRA - 2
            ))
            .bounds();
        assert!(
            first.left() < second.left(),
            "the row follows today's order"
        );
        assert!(window.try_find("album-panel-album-2").is_none());
    })
    .unwrap();
    let collapsed = shown_extras(window, cx);
    assert!(collapsed < EXTRA, "one row shows only part of the shelf");

    fixture.click(window, "discover-album-old-rotation-album-2", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let tile = window.find("discover-album-old-rotation-album-2");
        assert_eq!(tile.selected(), Some(true));
        assert!(window.find("album-panel-album-2").visible());
        assert_eq!(
            window
                .find("discover-album-forgotten-favorites-extra-9")
                .selected(),
            Some(false)
        );
    })
    .unwrap();
    assert!(
        fixture.playback_intents().is_empty(),
        "one click only selects"
    );
    cx.update_window(window, |_, window, cx| {
        window.double_click("discover-album-old-rotation-album-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    assert_eq!(
        fixture.playback_intents(),
        [PlaybackIntent::PlayAlbum("album-2".into())]
    );

    fixture.click(window, "discover-see-all-forgotten-favorites", cx);
    assert_eq!(shown_extras(window, cx), EXTRA);
    cx.update_window(window, |_, window, _| {
        let first = window
            .find("discover-album-forgotten-favorites-album-3")
            .bounds();
        let second = window
            .find("discover-album-forgotten-favorites-album-1")
            .bounds();
        assert!(first.left() < second.left(), "See all shows the ranking");
    })
    .unwrap();
    fixture.click(window, "discover-see-all-forgotten-favorites", cx);
    assert_eq!(shown_extras(window, cx), collapsed);

    fixture.click(window, "discover-shuffle-forgotten-favorites", cx);
    let shuffled = &fixture.playback_intents()[1..];
    assert_eq!(shuffled.len(), ranked.len());
    assert!(matches!(shuffled[0], PlaybackIntent::PlayAlbum(_)));
    let mut ids = shuffled
        .iter()
        .enumerate()
        .map(|(ix, intent)| match intent {
            PlaybackIntent::PlayAlbum(id) if ix == 0 => id.clone(),
            PlaybackIntent::EnqueueAlbum(id, Placement::Last) if ix > 0 => id.clone(),
            other => panic!("unexpected shuffle intent {other:?}"),
        })
        .collect::<Vec<_>>();
    ids.sort();
    let mut expected = ranked.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    expected.sort();
    assert_eq!(ids, expected);
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn discover_dismissals_refresh_and_history_keep_the_page_current(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    fixture.hold_queries();
    fixture.search(window, "Fixture", cx);
    let both = &["album-3", "album-1"];
    let first = Rediscover {
        old_rotation: shelf(&fixture, &["album-2"], &["album-2"]),
        forgotten_favorites: shelf(&fixture, both, both),
    };
    reply(&mut fixture, "open Discover", first);
    fixture.click(window, "nav-discover", cx);

    fixture.expect("hide album-3 for now", |command, _| {
        assert!(matches!(
            command,
            Command::HideDiscover(id, Dismissal::NotNow) if id == "album-3"
        ));
        vec![]
    });
    let after_hide = Rediscover {
        old_rotation: shelf(&fixture, &["album-2"], &["album-2"]),
        forgotten_favorites: shelf(&fixture, &["album-1"], &["album-1"]),
    };
    reply(&mut fixture, "shelves without album-3", after_hide);
    cx.update_window(window, |_, window, cx| {
        window.right_click("discover-album-forgotten-favorites-album-3", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.menu_item(window, "Not now", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find("discover-album-forgotten-favorites-album-3")
                .is_none()
        );
        assert!(
            window
                .find("discover-album-forgotten-favorites-album-1")
                .visible()
        );
    })
    .unwrap();

    let refreshed = Rediscover {
        old_rotation: Shelf::default(),
        forgotten_favorites: shelf(&fixture, &["album-1"], &["album-1"]),
    };
    reply(&mut fixture, "refresh after library update", refreshed);
    fixture.deliver_query("Fixture");
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("discover-shelf-old-rotation").is_none());
        assert!(window.find("discover-shelf-forgotten-favorites").visible());
    })
    .unwrap();

    fixture.click(window, "history-back", cx);
    assert_eq!(active_title(window, &app, cx), "Music Explorer");
    reply(&mut fixture, "forward to Discover", Rediscover::default());
    fixture.click(window, "history-forward", cx);
    assert_eq!(active_title(window, &app, cx), "Discover");
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("discover-empty").visible());
    })
    .unwrap();
    fixture.assert_script_finished();
}
