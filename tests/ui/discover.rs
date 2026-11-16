use crate::common::ui::{Fixture, PlaybackIntent, start};
use gpui_kit::{AnyWindowHandle, AppContext, Entity, TestAppContext, test::TestWindowExt};
use sonora::{
    discover::Rediscover,
    model::Album,
    playback::Placement,
    service::{Command, Event},
    ui::MusicApp,
};
use std::sync::Arc;

fn shelf_album(fixture: &Fixture, id: &str) -> Arc<Album> {
    fixture
        .catalog
        .view(&Default::default())
        .unwrap()
        .albums
        .iter()
        .find(|album| album.id == id)
        .cloned()
        .unwrap()
}

fn shelves(fixture: &Fixture, old: &[&str], forgotten: &[&str]) -> Arc<Rediscover> {
    Arc::new(Rediscover {
        old_rotation: old.iter().map(|id| shelf_album(fixture, id)).collect(),
        forgotten_favorites: forgotten
            .iter()
            .map(|id| shelf_album(fixture, id))
            .collect(),
    })
}

fn reply(fixture: &mut Fixture, label: &'static str, shelves: Arc<Rediscover>) {
    fixture.expect(label, move |command, _| {
        let Command::Discover(request) = command else {
            panic!("expected a Discover request")
        };
        vec![
            Event::Discover(request, shelves),
            Event::Discover(request.wrapping_sub(1), Arc::new(Rediscover::default())),
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

#[gpui_kit::test]
fn discover_shelves_open_play_shuffle_and_refresh_after_a_library_update(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.capture_playback();
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    fixture.hold_queries();
    fixture.search(window, "Fixture", cx);
    let first = shelves(&fixture, &["album-2"], &["album-3", "album-1"]);
    reply(&mut fixture, "open Discover", first);
    fixture.click(window, "nav-discover", cx);
    fixture.diagnose("discover shelves", window, &app, cx);
    assert_eq!(active_title(window, &app, cx), "Discover");
    cx.update_window(window, |_, window, _| {
        assert!(window.find("discover-shelf-old-rotation").visible());
        let first = window
            .find("discover-album-forgotten-favorites-album-3")
            .bounds();
        let second = window
            .find("discover-album-forgotten-favorites-album-1")
            .bounds();
        assert!(
            first.left() < second.left(),
            "shelf keeps its ranking order"
        );
        let tile = window.find("discover-album-old-rotation-album-2");
        assert!(
            tile.label()
                .is_some_and(|label| label.starts_with("Open album Second release"))
        );
    })
    .unwrap();

    fixture.click(window, "discover-play-old-rotation-album-2", cx);
    fixture.click(window, "discover-shuffle-forgotten-favorites", cx);
    let intents = fixture.playback_intents();
    assert_eq!(intents[0], PlaybackIntent::PlayAlbum("album-2".into()));
    let mut shuffled = intents[1..]
        .iter()
        .map(|intent| match intent {
            PlaybackIntent::PlayAlbum(id) => (id.as_str(), None),
            PlaybackIntent::EnqueueAlbum(id, placement) => (id.as_str(), Some(*placement)),
        })
        .collect::<Vec<_>>();
    assert_eq!(shuffled.len(), 2);
    assert_eq!(shuffled[0].1, None, "shuffle starts by playing one album");
    assert_eq!(shuffled[1].1, Some(Placement::Last));
    shuffled.sort_by_key(|(id, _)| *id);
    assert_eq!(
        shuffled.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ["album-1", "album-3"]
    );

    let refreshed = shelves(&fixture, &[], &["album-1"]);
    reply(&mut fixture, "refresh after library update", refreshed);
    fixture.deliver_query("Fixture");
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("discover-shelf-old-rotation").is_none());
        assert!(
            window
                .find("discover-album-forgotten-favorites-album-1")
                .visible()
        );
        assert!(
            window
                .try_find("discover-album-forgotten-favorites-album-3")
                .is_none()
        );
    })
    .unwrap();

    fixture.click(window, "discover-album-forgotten-favorites-album-1", cx);
    assert_eq!(fixture.last_query().selection.albums, ["album-1"]);
    assert_eq!(active_title(window, &app, cx), "Music Explorer");

    let empty = shelves(&fixture, &[], &[]);
    reply(&mut fixture, "back to Discover", empty);
    fixture.click(window, "history-back", cx);
    assert_eq!(active_title(window, &app, cx), "Discover");
    cx.update_window(window, |_, window, _| {
        assert!(window.find("discover-empty").visible());
    })
    .unwrap();
    fixture.assert_script_finished();
}
