use crate::common::ui::{
    ARTIST, ARTIST_TARGET, Fixture, SECOND_ARTIST, SECOND_ARTIST_TARGET, assert_active_tab,
    assert_rightmost, start,
};
use gpui_kit::{AppContext, SharedString, TestAppContext, test::TestWindowExt};
use sonora::{model::Preferences, platform::shortcut, playback::PlayerCommand, ui::tab_bindings};
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

#[gpui_kit::test]
fn new_tab_button_follows_the_last_tab_without_extra_space(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    for expected_ids in [&[1_u64, 2, 3, 4][..], &[1, 2, 3, 4, 5][..]] {
        fixture.diagnose("tab strip", window, &app, cx);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let last = expected_ids[expected_ids.len() - 1];
            let last = window.find(("workspace-tab", last)).bounds();
            let previous = expected_ids[expected_ids.len() - 2];
            let previous = window.find(("workspace-tab", previous)).bounds();
            let new_tab = window.find("new-tab").bounds();
            let between_tabs = last.left() - previous.right();
            let before_new_tab = new_tab.left() - last.right();
            assert!(
                before_new_tab >= gpui_kit::px(0.) && before_new_tab <= between_tabs,
                "New Tab sits {before_new_tab:?} after the last tab; tabs are {between_tabs:?} apart"
            );
        })
        .unwrap();
        fixture.click(window, "new-tab", cx);
    }
}

#[gpui_kit::test]
fn discover_is_gone_and_a_saved_discover_tab_opens_music_explorer(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(tab_bindings());
    });
    let dir = tempfile::tempdir().unwrap();
    let mut fixture = Fixture::open(&dir.path().join("catalog.sqlite"));
    fixture.seed();
    // Tabs saved by a build that still had Discover.
    let mut saved = serde_json::to_value(Preferences::default()).unwrap();
    saved["tabs"]["tabs"][0]["page"]["workspace"] = serde_json::json!("Discover");
    saved["tabs"]["tabs"][1]["page"]["workspace"] = serde_json::json!("Discover");
    saved["tabs"]["tabs"][1]["locked"] = serde_json::json!(true);
    saved["tabs"]["tabs"][1]["home"] = saved["tabs"]["tabs"][1]["page"].clone();
    fixture.catalog.set_state("preferences", &saved).unwrap();
    let (window, app) = fixture.mount(cx);
    fixture.settle(window, cx);
    fixture.diagnose("saved discover tabs", window, &app, cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("nav-discover").is_none());
        for id in [1_u64, 2] {
            assert_eq!(
                window.find(("workspace-tab", id)).label(),
                Some("Music Explorer")
            );
        }
        assert!(window.find("nav-explorer").visible());
    })
    .unwrap();
    // Any request for Discover would be an unscripted command and fail here.
    fixture.click(window, ("workspace-tab", 2_u64), cx);
    fixture.assert_script_finished();
}
