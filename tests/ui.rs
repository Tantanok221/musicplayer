//! UI integration scenarios using the production library view.
//! Run with `scripts/test-ui.sh` or `cargo test --features test-support --test ui`.
mod common;
#[path = "ui/explorer.rs"]
mod explorer;
use common::ui::{
    ARTIST, ARTIST_TARGET, Fixture, MUTATION_ERROR, SECOND_ARTIST, SECOND_ARTIST_TARGET,
    assert_active_tab, assert_rightmost, start,
};
use gpui_kit::{AppContext, SharedString, TestAppContext, test::TestWindowExt};
use sonora::{
    model::Preferences, platform::shortcut, playback::PlayerCommand, service::Mutation,
    ui::tab_bindings,
};
use std::time::Duration;

#[gpui_kit::test]
fn new_tab_artist_stays_rightmost_and_survives_restart(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(tab_bindings());
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.sqlite");
    let mut fixture = Fixture::open(&path);
    fixture.seed();
    let (window, app) = fixture.mount(cx);
    fixture.settle(window, cx);
    fixture.diagnose("ready", window, &app, cx);

    cx.update_window(window, |_, window, cx| window.click("new-tab", cx))
        .unwrap();
    fixture.settle(window, cx);
    fixture.diagnose("new tab clicked", window, &app, cx);
    let tabs = cx.update(|cx| app.read(cx).tab_state().clone());
    let id = tabs.active().id;
    let ids = tabs.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
    assert_eq!(ids, [1, 2, 3, 4, 5]);
    assert_rightmost(window, &app, id, &ids, "New Tab", cx);

    cx.update_window(window, |_, window, cx| {
        assert!(window.find(SharedString::from(ARTIST_TARGET)).visible());
        window.click(SharedString::from(ARTIST_TARGET), cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.diagnose("artist selected", window, &app, cx);
    assert_rightmost(window, &app, id, &ids, ARTIST, cx);

    let before: Preferences = fixture.catalog.state("preferences").unwrap();
    assert_eq!(
        before.tabs.tabs.len(),
        4,
        "tab save should still be debounced"
    );
    cx.executor().advance_clock(Duration::from_millis(499));
    fixture.settle(window, cx);
    let before_deadline: Preferences = fixture.catalog.state("preferences").unwrap();
    assert_eq!(before_deadline.tabs, before.tabs);
    cx.executor().advance_clock(Duration::from_millis(1));
    fixture.settle(window, cx);
    let saved: Preferences = fixture.catalog.state("preferences").unwrap();
    assert_eq!(saved.tabs, cx.update(|cx| app.read(cx).tab_state().clone()));
    assert!(fixture.saves > 0, "UI must send a preference write");
    let player_updates = fixture
        .backend
        .player_commands()
        .try_iter()
        .collect::<Vec<_>>();
    assert!(
        !player_updates.is_empty(),
        "save must update the player handle"
    );
    for command in player_updates {
        match command {
            PlayerCommand::Preferences(prefs) => assert_eq!(prefs.tabs, saved.tabs),
            _ => panic!("tab navigation unexpectedly sent a playback command"),
        }
    }
    fixture.diagnose("preferences persisted", window, &app, cx);

    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    drop(fixture);
    cx.run_until_parked();

    let mut restarted = Fixture::open(&path);
    let (window, app) = restarted.mount(cx);
    restarted.settle(window, cx);
    restarted.diagnose("restarted from SQLite", window, &app, cx);
    assert_rightmost(window, &app, id, &ids, ARTIST, cx);

    // Keyboard creation reaches the production action handler too.
    cx.update_window(window, |_, window, cx| {
        window.press(shortcut("cmd-2", "ctrl-2"), cx);
    })
    .unwrap();
    restarted.settle(window, cx);
    assert_eq!(cx.update(|cx| app.read(cx).tab_state().active), 1);
    cx.update_window(window, |_, window, cx| {
        window.press(shortcut("cmd-t", "ctrl-t"), cx)
    })
    .unwrap();
    restarted.settle(window, cx);
    let mut keyboard_ids = ids;
    keyboard_ids.push(6);
    assert_rightmost(window, &app, 6, &keyboard_ids, "New Tab", cx);
    cx.update_window(window, |_, window, cx| {
        window.click(SharedString::from(ARTIST_TARGET), cx)
    })
    .unwrap();
    restarted.settle(window, cx);
    restarted.diagnose("keyboard tab artist selected", window, &app, cx);
    assert_rightmost(window, &app, 6, &keyboard_ids, ARTIST, cx);
}

#[gpui_kit::test]
fn closing_and_reopening_restores_the_artist_tab_in_place(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.click(window, "new-tab", cx);
    fixture.click(window, SharedString::from(ARTIST_TARGET), cx);
    fixture.click(window, "new-tab", cx);
    fixture.click(window, ("workspace-tab", 5_u64), cx);
    let before = cx.update(|cx| app.read(cx).tab_state().clone());
    assert_active_tab(window, &app, 5, &[1, 2, 3, 4, 5, 6], ARTIST, cx);

    cx.update_window(window, |_, window, cx| {
        window.hover(("workspace-tab", 5_u64), cx);
        window.click(("tab-close", 5_u64), cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.diagnose("artist tab closed", window, &app, cx);
    assert_active_tab(window, &app, 6, &[1, 2, 3, 4, 6], "New Tab", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find(("workspace-tab", 5_u64)).is_none())
    })
    .unwrap();

    fixture.press(window, shortcut("cmd-shift-t", "ctrl-shift-t"), cx);
    fixture.diagnose("artist tab reopened", window, &app, cx);
    assert_active_tab(window, &app, 5, &[1, 2, 3, 4, 5, 6], ARTIST, cx);
    assert_eq!(cx.update(|cx| app.read(cx).tab_state().clone()), before);
    cx.update_window(window, |_, window, _| {
        assert!(
            window.find(("workspace-tab", 5_u64)).bounds().right()
                <= window.find(("workspace-tab", 6_u64)).bounds().left()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn back_and_forward_histories_stay_with_each_tab(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.click(window, "new-tab", cx);
    fixture.click(window, SharedString::from(ARTIST_TARGET), cx);
    fixture.click(window, SharedString::from(SECOND_ARTIST_TARGET), cx);
    fixture.click(window, "history-back", cx);
    assert_active_tab(window, &app, 5, &[1, 2, 3, 4, 5], ARTIST, cx);
    fixture.click(window, "history-forward", cx);
    assert_active_tab(window, &app, 5, &[1, 2, 3, 4, 5], SECOND_ARTIST, cx);

    fixture.click(window, "new-tab", cx);
    fixture.click(window, SharedString::from(ARTIST_TARGET), cx);
    fixture.click(window, "history-back", cx);
    assert_active_tab(window, &app, 6, &[1, 2, 3, 4, 5, 6], "New Tab", cx);
    fixture.click(window, ("workspace-tab", 5_u64), cx);
    fixture.click(window, "history-back", cx);
    assert_active_tab(window, &app, 5, &[1, 2, 3, 4, 5, 6], ARTIST, cx);
    fixture.click(window, ("workspace-tab", 6_u64), cx);
    assert_active_tab(window, &app, 6, &[1, 2, 3, 4, 5, 6], "New Tab", cx);
    fixture.click(window, "history-forward", cx);
    assert_active_tab(window, &app, 6, &[1, 2, 3, 4, 5, 6], ARTIST, cx);
    fixture.click(window, ("workspace-tab", 5_u64), cx);
    fixture.click(window, "history-forward", cx);
    fixture.diagnose("independent tab histories restored", window, &app, cx);
    assert_active_tab(window, &app, 5, &[1, 2, 3, 4, 5, 6], SECOND_ARTIST, cx);
}

fn assert_favorite(
    window: gpui_kit::AnyWindowHandle,
    favorite: bool,
    pending: bool,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, _| {
        let button = window.find("album-favorite-album-1");
        assert!(button.visible());
        assert_eq!(
            button.label(),
            Some(if favorite {
                "Unfavorite album Fixture album"
            } else {
                "Favorite album Fixture album"
            })
        );
        assert_eq!(button.disabled() == Some(true), pending);
        assert_eq!(
            window.find("album-rating-album-1-4").disabled() == Some(true),
            pending
        );
    })
    .unwrap();
}

fn assert_song_and_other_album_unchanged(fixture: &Fixture) {
    let tracks = fixture.catalog.by_ids(&["track-1".into()]).unwrap();
    assert_eq!(tracks[0].user_rating, Some(5));
    assert_eq!(tracks[0].starred.as_deref(), Some("2026-01-01T00:00:00Z"));
    let other = fixture.catalog.album("album-2").unwrap().unwrap();
    assert_eq!(other.user_rating, Some(4));
    assert_eq!(other.starred.as_deref(), Some("2026-01-01T00:00:00Z"));
}

#[gpui_kit::test]
fn album_favorite_is_optimistic_blocks_duplicates_and_confirms(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.open_album(window, cx);
    assert_favorite(window, false, false, cx);
    fixture.click(window, "album-favorite-album-1", cx);
    fixture.diagnose("favorite pending", window, &app, cx);
    assert_favorite(window, true, true, cx);
    assert!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .starred
            .is_none()
    );
    fixture.click(window, "album-favorite-album-1", cx);
    assert_eq!(
        fixture.mutations(),
        &[Mutation::AlbumFavorite("album-1".into(), true)]
    );

    fixture.confirm_mutation();
    fixture.settle(window, cx);
    assert_favorite(window, true, false, cx);
    assert!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .starred
            .is_some()
    );
    // A real second click proves the confirmed control is enabled again.
    fixture.click(window, "album-favorite-album-1", cx);
    assert_favorite(window, false, true, cx);
    assert_eq!(
        fixture.mutations(),
        &[
            Mutation::AlbumFavorite("album-1".into(), true),
            Mutation::AlbumFavorite("album-1".into(), false)
        ]
    );
    fixture.confirm_mutation();
    fixture.settle(window, cx);
    assert_favorite(window, false, false, cx);
    assert!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .starred
            .is_none()
    );
    assert_song_and_other_album_unchanged(&fixture);
}

#[gpui_kit::test]
fn failed_album_favorite_rolls_back_and_retry_sends_the_same_intent(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.open_album(window, cx);
    fixture.click(window, "album-favorite-album-1", cx);
    assert_favorite(window, true, true, cx);
    fixture.fail_mutation();
    fixture.settle(window, cx);
    fixture.diagnose("favorite rolled back", window, &app, cx);
    assert_favorite(window, false, false, cx);
    assert!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .starred
            .is_none()
    );
    cx.update_window(window, |_, window, _| {
        let error = window.find("error-banner");
        assert_eq!(error.role(), Some(gpui_kit::Role::Alert));
        assert_eq!(error.label(), Some(MUTATION_ERROR));
        assert!(error.visible());
        assert!(window.find("retry-error").visible());
    })
    .unwrap();

    fixture.click(window, "retry-error", cx);
    assert_favorite(window, true, true, cx);
    assert_eq!(
        fixture.mutations(),
        &[
            Mutation::AlbumFavorite("album-1".into(), true),
            Mutation::AlbumFavorite("album-1".into(), true)
        ]
    );
    fixture.click(window, "album-favorite-album-1", cx);
    assert_eq!(fixture.mutations().len(), 2);
    fixture.confirm_mutation();
    fixture.settle(window, cx);
    fixture.diagnose("favorite retry confirmed", window, &app, cx);
    assert_favorite(window, true, false, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("error-banner").is_none());
        assert!(window.try_find("retry-error").is_none());
    })
    .unwrap();
    assert!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .starred
            .is_some()
    );
    assert_song_and_other_album_unchanged(&fixture);
}

#[gpui_kit::test]
fn album_rating_sets_and_clears_without_changing_song_rating(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.open_album(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("album-rating-album-1-2").label(),
            Some("Clear album Fixture album rating")
        );
    })
    .unwrap();
    fixture.click(window, "album-rating-album-1-4", cx);
    fixture.diagnose("album rating pending", window, &app, cx);
    assert_eq!(
        fixture.mutations(),
        &[Mutation::AlbumRating("album-1".into(), 4)]
    );
    assert_eq!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .user_rating,
        Some(2)
    );
    cx.update_window(window, |_, window, _| {
        let rating = window.find("album-rating-album-1-4");
        assert_eq!(rating.label(), Some("Clear album Fixture album rating"));
        assert_eq!(rating.disabled(), Some(true));
    })
    .unwrap();
    fixture.confirm_mutation();
    fixture.settle(window, cx);
    assert_eq!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .user_rating,
        Some(4)
    );
    fixture.click(window, "album-rating-album-1-4", cx);
    assert_eq!(
        fixture.mutations(),
        &[
            Mutation::AlbumRating("album-1".into(), 4),
            Mutation::AlbumRating("album-1".into(), 0)
        ]
    );
    fixture.confirm_mutation();
    fixture.settle(window, cx);
    fixture.diagnose("album rating cleared", window, &app, cx);
    assert_eq!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .user_rating,
        Some(0)
    );
    cx.update_window(window, |_, window, _| {
        let rating = window.find("album-rating-album-1-4");
        assert_eq!(rating.label(), Some("Rate album Fixture album 4 stars"));
        assert_ne!(rating.disabled(), Some(true));
    })
    .unwrap();
    assert_song_and_other_album_unchanged(&fixture);
}
