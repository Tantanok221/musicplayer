use crate::common::ui::{Fixture, PlaybackIntent, start};
use gpui_kit::{AnyWindowHandle, AppContext, Entity, TestAppContext, test::TestWindowExt};
use sonora::{
    discover::{Dismissal, Pick, Reason, Rediscover, Season, Shelf},
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

fn rotation(fixture: &Fixture, id: &str) -> Pick {
    Pick {
        album: album(fixture, id),
        reason: Reason {
            plays: 44,
            played: "2026-03-10T00:00:00Z".parse().unwrap(),
        },
    }
}

fn picks(fixture: &Fixture, ids: &[&str]) -> Vec<Pick> {
    ids.iter().map(|id| rotation(fixture, id)).collect()
}

fn season(fixture: &Fixture, key: &str, label: &str, ranked: &[&str], today: &[&str]) -> Season {
    Season {
        key: key.into(),
        label: label.into(),
        shelf: Shelf {
            ranked: picks(fixture, ranked),
            today: picks(fixture, today),
        },
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

fn label(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> String {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window
            .find(id.to_string())
            .label()
            .unwrap_or_default()
            .to_string()
    })
    .unwrap()
}

fn shown(window: AnyWindowHandle, prefix: &str, cx: &mut TestAppContext) -> usize {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        (0..EXTRA)
            .filter(|ix| window.try_find(format!("{prefix}extra-{ix}")).is_some())
            .count()
    })
    .unwrap()
}

fn open(
    fixture: &mut Fixture,
    window: AnyWindowHandle,
    shelves: Rediscover,
    cx: &mut TestAppContext,
) {
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    reply(fixture, "open Discover", shelves);
    fixture.click(window, "nav-discover", cx);
}

#[gpui_kit::test]
fn memory_lane_groups_seasons_selects_like_explorer_and_expands(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.capture_playback();
    let extras = (0..EXTRA)
        .map(|ix| format!("extra-{ix}"))
        .collect::<Vec<_>>();
    let mut ranked = vec!["album-3", "album-1"];
    ranked.extend(extras.iter().map(String::as_str));
    let mut today = ranked.clone();
    today.reverse();
    let spring = season(&fixture, "2026-spring", "Spring 2026", &ranked, &today);
    let older = season(
        &fixture,
        "older",
        "A year ago and longer",
        &["album-2"],
        &["album-2"],
    );
    let mut all = spring.shelf.ranked.clone();
    all.extend(older.shelf.ranked.clone());
    let reason = older.shelf.ranked[0].reason_text(chrono::Utc::now());
    let shelves = Rediscover {
        old_rotation: Shelf {
            ranked: all.clone(),
            today: all,
        },
        lane: vec![spring, older],
    };
    open(&mut fixture, window, shelves, cx);
    fixture.diagnose("memory lane", window, &app, cx);
    assert_eq!(
        label(window, "discover-season-2026-spring", cx),
        "Spring 2026"
    );
    assert_eq!(label(window, "discover-reason-older-album-2", cx), reason);
    cx.update_window(window, |_, window, _| {
        let spring = window.find("discover-season-2026-spring").bounds();
        let older = window.find("discover-season-older").bounds();
        assert!(spring.top() < older.top(), "seasons run newest first");
        let first = window
            .find(format!("discover-album-2026-spring-extra-{}", EXTRA - 1))
            .bounds();
        let second = window
            .find(format!("discover-album-2026-spring-extra-{}", EXTRA - 2))
            .bounds();
        assert!(
            first.left() < second.left(),
            "the row follows today's order"
        );
        assert!(window.try_find("album-panel-album-2").is_none());
    })
    .unwrap();
    let collapsed = shown(window, "discover-album-2026-spring-", cx);
    assert!(collapsed < EXTRA, "one row shows only part of the season");

    fixture.click(window, "discover-album-older-album-2", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let tile = window.find("discover-album-older-album-2");
        assert_eq!(tile.selected(), Some(true));
        assert!(window.find("album-panel-album-2").visible());
    })
    .unwrap();
    assert!(
        fixture.playback_intents().is_empty(),
        "one click only selects"
    );
    cx.update_window(window, |_, window, cx| {
        window.double_click("discover-album-older-album-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    assert_eq!(
        fixture.playback_intents(),
        [PlaybackIntent::PlayAlbum("album-2".into())]
    );

    fixture.click(window, "discover-see-all-2026-spring", cx);
    assert_eq!(shown(window, "discover-album-2026-spring-", cx), EXTRA);
    cx.update_window(window, |_, window, _| {
        let first = window.find("discover-album-2026-spring-album-3").bounds();
        let second = window.find("discover-album-2026-spring-album-1").bounds();
        assert!(first.left() < second.left(), "See all shows the ranking");
    })
    .unwrap();
    fixture.click(window, "discover-see-all-2026-spring", cx);
    assert_eq!(shown(window, "discover-album-2026-spring-", cx), collapsed);

    fixture.click(window, "discover-shuffle-lane", cx);
    let shuffled = &fixture.playback_intents()[1..];
    assert_eq!(shuffled.len(), ranked.len() + 1);
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
    expected.push("album-2".into());
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
    let lane = |fixture: &Fixture, ids: &[&str]| Rediscover {
        lane: vec![season(fixture, "older", "Spring 2024", ids, ids)],
        ..Default::default()
    };
    let first = lane(&fixture, &["album-3", "album-1", "album-2"]);
    reply(&mut fixture, "open Discover", first);
    fixture.click(window, "nav-discover", cx);

    fixture.expect("hide album-3 for now", |command, _| {
        assert!(matches!(
            command,
            Command::HideDiscover(id, Dismissal::NotNow) if id == "album-3"
        ));
        vec![]
    });
    let after_hide = lane(&fixture, &["album-1", "album-2"]);
    reply(&mut fixture, "lane without album-3", after_hide);
    cx.update_window(window, |_, window, cx| {
        window.right_click("discover-album-older-album-3", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.menu_item(window, "Not now", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("discover-album-older-album-3").is_none());
        assert!(window.find("discover-album-older-album-1").visible());
    })
    .unwrap();

    fixture.expect("never suggest album-2", |command, _| {
        assert!(matches!(
            command,
            Command::HideDiscover(id, Dismissal::Never) if id == "album-2"
        ));
        vec![]
    });
    let after_never = lane(&fixture, &["album-1"]);
    reply(&mut fixture, "lane without album-2", after_never);
    cx.update_window(window, |_, window, cx| {
        window.right_click("discover-album-older-album-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.menu_item(window, "Never suggest", cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("discover-album-older-album-2").is_none());
        assert!(window.find("discover-album-older-album-1").visible());
    })
    .unwrap();

    reply(
        &mut fixture,
        "refresh after library update",
        Rediscover::default(),
    );
    fixture.deliver_query("Fixture");
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("discover-lane").is_none());
        assert!(window.find("discover-empty").visible());
    })
    .unwrap();

    fixture.click(window, "history-back", cx);
    assert_eq!(active_title(window, &app, cx), "Music Explorer");
    reply(&mut fixture, "forward to Discover", Rediscover::default());
    fixture.click(window, "history-forward", cx);
    assert_eq!(active_title(window, &app, cx), "Discover");
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn discover_is_only_memory_lane(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    let older = season(&fixture, "older", "Spring 2024", &["album-2"], &["album-2"]);
    let shelves = Rediscover {
        old_rotation: older.shelf.clone(),
        lane: vec![older],
    };
    open(&mut fixture, window, shelves, cx);
    fixture.diagnose("memory lane only", window, &app, cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("discover-lane").visible());
        for gone in ["discover-spotlight", "discover-loved", "discover-closing"] {
            assert!(window.try_find(gone).is_none(), "{gone} should be gone");
        }
    })
    .unwrap();
    fixture.assert_script_finished();
}
