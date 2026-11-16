//! Actual panel drags/menu toggles persist; tab overflow stays navigable.
use crate::common::ui::{Fixture, start};
use gpui_kit::{
    AppContext, Bounds, Pixels, TestAppContext, Window, point, px, size, test::TestWindowExt,
};
use sonora::model::Preferences;

fn divider(window: &mut Window) -> Bounds<Pixels> {
    window
        .find_all(("resizable-handle", 0_usize))
        .into_iter()
        .filter(|handle| handle.bounds().size.height > handle.bounds().size.width * 5.)
        .min_by(|a, b| a.bounds().left().partial_cmp(&b.bounds().left()).unwrap())
        .expect("navigator divider")
        .bounds()
}

#[gpui_kit::test]
fn resized_navigator_and_information_visibility_survive_catalog_reload(cx: &mut TestAppContext) {
    let (dir, mut fixture, window, app) = start(cx);
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    let before: Preferences = fixture.catalog.state("preferences").unwrap();
    let start = cx
        .update_window(window, |_, window, _| divider(window).center())
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.drag(start, start + point(px(54.), px(0.)), cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    let resized: Preferences = fixture.catalog.state("preferences").unwrap();
    assert!(resized.layout.left_width > before.layout.left_width + 30.);
    cx.update_window(window, |_, window, _| {
        assert!(divider(window).center().x > start.x + px(30.));
    })
    .unwrap();
    fixture.click(window, "layout-menu", cx);
    fixture.menu_item(window, "Queue and information", cx);
    let saved: Preferences = fixture.catalog.state("preferences").unwrap();
    assert!(!saved.layout.show_information);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("info-Playing").is_none())
    })
    .unwrap();
    fixture.diagnose("resized and hidden panel persisted", window, &app, cx);
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    drop(fixture);
    cx.run_until_parked();

    let mut restarted = Fixture::open(&dir.path().join("catalog.sqlite"));
    let (window, app) = restarted.mount(cx);
    restarted.settle(window, cx);
    let restored: Preferences = restarted.catalog.state("preferences").unwrap();
    assert_eq!(restored.layout, saved.layout);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("info-Playing").is_none());
        assert!(divider(window).center().x > start.x + px(30.));
    })
    .unwrap();
    // Show it through the same menu: persistence must not make it unreachable.
    restarted.click(window, "layout-menu", cx);
    restarted.menu_item(window, "Queue and information", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("info-Playing").visible())
    })
    .unwrap();
    restarted.diagnose("restored panel is usable", window, &app, cx);
}

#[gpui_kit::test]
fn narrow_window_tab_arrows_reach_both_ends_and_keep_new_tab_usable(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    cx.simulate_window_resize(window, size(px(760.), px(650.)));
    fixture.settle(window, cx);
    for _ in 0..9 {
        fixture.click(window, "new-tab", cx);
    }
    let last = cx.update(|cx| app.read(cx).tab_state().active().id);
    cx.update_window(window, |_, window, _| {
        let active = window.find(("workspace-tab", last));
        assert!(active.visible());
        assert_eq!(active.selected(), Some(true));
        assert!(window.find("new-tab").visible());
        assert!(window.find("tabs-left").visible());
    })
    .unwrap();
    for _ in 0..16 {
        let first_visible = cx
            .update_window(window, |_, window, _| {
                let first = window.find(("workspace-tab", 1_u64));
                let bounds = first.bounds();
                first.visible() && bounds.left() >= px(0.)
            })
            .unwrap();
        if first_visible {
            break;
        }
        fixture.click(window, "tabs-left", cx);
    }
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find(("workspace-tab", 1_u64)).selected(), Some(true));
        assert!(window.find("music-search").visible());
        assert!(window.find("tabs-right").visible());
    })
    .unwrap();
    for _ in 0..16 {
        let visible = cx
            .update_window(window, |_, window, _| {
                window.find(("workspace-tab", last)).visible()
            })
            .unwrap();
        if visible {
            break;
        }
        fixture.click(window, "tabs-right", cx);
    }
    fixture.click(window, ("workspace-tab", last), cx);
    assert_eq!(cx.update(|cx| app.read(cx).tab_state().active().id), last);
    fixture.click(window, "new-tab", cx);
    let created = cx.update(|cx| app.read(cx).tab_state().active().id);
    assert!(created > last);
    cx.update_window(window, |_, window, _| {
        assert!(window.find(("workspace-tab", created)).visible());
        assert_eq!(
            window.find(("workspace-tab", created)).selected(),
            Some(true)
        );
    })
    .unwrap();
    fixture.diagnose("narrow tab strip navigated", window, &app, cx);
}

#[gpui_kit::test]
fn window_frame_debounces_resize_and_restores_size_and_position(cx: &mut TestAppContext) {
    use gpui_kit::WindowBounds;
    use std::time::Duration;
    cx.update(gpui_kit::init);
    let dir = tempfile::tempdir().unwrap();
    let mut f = Fixture::open(&dir.path().join("catalog.sqlite"));
    f.seed();
    let (window, app) = f.mount_at(
        Bounds::new(point(px(90.), px(70.)), size(px(1440.), px(900.))),
        cx,
    );
    f.settle(window, cx);
    cx.simulate_window_resize(window, size(px(1000.), px(700.)));
    f.settle(window, cx);
    cx.executor().advance_clock(Duration::from_millis(499));
    f.settle(window, cx);
    assert!(f.paths.window_frame().is_none());
    cx.simulate_window_resize(window, size(px(1100.), px(720.)));
    f.settle(window, cx);
    cx.executor().advance_clock(Duration::from_millis(499));
    f.settle(window, cx);
    assert!(
        f.paths.window_frame().is_none(),
        "the previous save must be cancelled by a newer resize"
    );
    cx.executor().advance_clock(Duration::from_millis(1));
    f.settle(window, cx);
    let saved = f
        .paths
        .window_frame()
        .expect("SaveWindow must persist the final debounced frame");
    assert_eq!(
        (saved.x, saved.y, saved.width, saved.height),
        (90., 70., 1100., 720.)
    );
    assert_eq!(f.window_saves, 1);
    assert!(!saved.fullscreen && !saved.maximized);
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    drop(f);
    cx.run_until_parked();
    let mut restarted = Fixture::open(&dir.path().join("catalog.sqlite"));
    let (window, _) = restarted.mount(cx);
    restarted.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.window_bounds(),
            WindowBounds::Windowed(Bounds::new(
                point(px(90.), px(70.)),
                size(px(1100.), px(720.))
            ))
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn unreachable_saved_window_falls_back_and_small_window_is_clamped(cx: &mut TestAppContext) {
    use gpui_kit::WindowBounds;
    use sonora::platform::WindowFrame;
    cx.update(gpui_kit::init);
    let dir = tempfile::tempdir().unwrap();
    let mut f = Fixture::open(&dir.path().join("catalog.sqlite"));
    f.seed();
    f.paths
        .save_window_frame(&WindowFrame {
            x: 100000.,
            y: 100000.,
            width: 900.,
            height: 700.,
            ..Default::default()
        })
        .unwrap();
    let (window, app) = f.mount(cx);
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        let WindowBounds::Windowed(bounds) = window.window_bounds() else {
            panic!("normal window")
        };
        assert!(bounds.origin.x < px(1920.) && bounds.origin.y < px(1080.));
    })
    .unwrap();
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    cx.run_until_parked();
    f.paths
        .save_window_frame(&WindowFrame {
            x: 80.,
            y: 60.,
            width: 200.,
            height: 100.,
            ..Default::default()
        })
        .unwrap();
    let (window, _) = f.mount(cx);
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.window_bounds(),
            WindowBounds::Windowed(Bounds::new(
                point(px(80.), px(60.)),
                size(px(680.), px(540.))
            ))
        );
    })
    .unwrap();
}
