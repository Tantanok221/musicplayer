//! Update status in Settings and the status bar, with the real handlers.
use crate::common::ui::start;
use crate::settings::page;
use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    platform::shortcut,
    service::{Command, Event},
    update::{UpdateState, UpdateStatus},
};

fn status(state: UpdateState) -> Event {
    Event::Update(UpdateStatus {
        current: "a".repeat(40),
        state,
    })
}

#[gpui_kit::test]
fn settings_check_for_updates_and_restart_into_a_ready_one(cx: &mut TestAppContext) {
    let (_dir, mut f, window, _) = start(cx);
    f.backend.emit(status(UpdateState::Idle));
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "About", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("update-status").label(),
            Some("Version aaaaaaa · Updates install automatically")
        );
        assert!(window.try_find("update-restart").is_none());
    })
    .unwrap();

    f.expect("check now", |command, _| {
        assert!(matches!(command, Command::CheckForUpdates));
        vec![status(UpdateState::Ready("b".repeat(40)))]
    });
    f.click(window, "update-check", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("update-status").label(),
            Some("Version aaaaaaa · Update bbbbbbb is ready; restart to use it")
        );
        assert_eq!(
            window.find("update-restart").label(),
            Some("Restart to update")
        );
    })
    .unwrap();

    f.expect("restart into the update", |command, _| {
        assert!(matches!(command, Command::RestartToUpdate));
        vec![]
    });
    f.click(window, "update-restart", cx);
    f.assert_script_finished();
}

#[gpui_kit::test]
fn builds_that_cannot_update_say_so_and_offer_no_check(cx: &mut TestAppContext) {
    let (_dir, mut f, window, _) = start(cx);
    f.backend.emit(status(UpdateState::Unavailable));
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "About", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("update-status").label(),
            Some("Version aaaaaaa · Updates aren't available for this build")
        );
        assert_eq!(window.find("update-check").disabled(), Some(true));
        assert!(window.try_find("update-restart").is_none());
    })
    .unwrap();
}
