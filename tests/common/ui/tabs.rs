use gpui_kit::{AnyWindowHandle, AppContext, Entity, TestAppContext, test::TestWindowExt};
use sonora::ui::MusicApp;

pub(crate) fn assert_active_tab(
    window: AnyWindowHandle,
    app: &Entity<MusicApp>,
    id: u64,
    expected_ids: &[u64],
    title: &str,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let tabs = app.read(cx).tab_state();
        assert_eq!(
            tabs.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            expected_ids
        );
        assert_eq!(tabs.active().id, id);
        let last = window.find(("workspace-tab", id));
        assert_eq!(last.label(), Some(title));
        assert_eq!(last.selected(), Some(true));
        assert!(last.visible());
        for other in expected_ids.iter().filter(|other| **other != id) {
            assert_eq!(
                window.find(("workspace-tab", *other)).selected(),
                Some(false)
            );
        }
    })
    .unwrap();
}

pub(crate) fn assert_rightmost(
    window: AnyWindowHandle,
    app: &Entity<MusicApp>,
    id: u64,
    expected_ids: &[u64],
    title: &str,
    cx: &mut TestAppContext,
) {
    assert_active_tab(window, app, id, expected_ids, title, cx);
    cx.update_window(window, |_, window, cx| {
        assert_eq!(app.read(cx).tab_state().active, expected_ids.len() - 1);
        let last = window.find(("workspace-tab", id));
        for previous in &expected_ids[..expected_ids.len() - 1] {
            assert!(
                window.find(("workspace-tab", *previous)).bounds().right() <= last.bounds().left()
            );
        }
    })
    .unwrap();
}
