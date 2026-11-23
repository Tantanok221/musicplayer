use crate::common::ui::{Fixture, start};
use gpui_kit::{AppContext, TestAppContext, component::ActiveTheme, test::TestWindowExt};
use sonora::{model::Preferences, platform::shortcut, service::Event, sync::SharedPreferences};

#[gpui_kit::test]
fn received_shared_theme_applies_without_sending_preferences_back(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _) = start(cx);
    let dark = !cx.update(|cx| cx.theme().is_dark());
    fixture.backend.emit(Event::SharedPreferences(
        SharedPreferences::from_preferences(&Preferences {
            dark: Some(dark),
            ..Default::default()
        }),
    ));
    fixture.settle(window, cx);
    assert_eq!(cx.update(|cx| cx.theme().is_dark()), dark);
    assert!(
        fixture
            .catalog
            .state::<Preferences>("preferences")
            .unwrap()
            .dark
            .is_none()
    );
}

#[gpui_kit::test]
fn light_dark_and_system_theme_apply_immediately_and_survive_restart(cx: &mut TestAppContext) {
    let (dir, mut f, window, app) = start(cx);
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    crate::settings::page(&mut f, window, "Appearance", cx);
    for (id, expected) in [
        ("appearance-light", Some(false)),
        ("appearance-dark", Some(true)),
        ("appearance-light", Some(false)),
        ("appearance-system", None),
        ("appearance-dark", Some(true)),
    ] {
        cx.update_window(window, |_, window, cx| {
            window.within("appearance").click(
                match id {
                    "appearance-light" => 1_usize,
                    "appearance-dark" => 2,
                    _ => 0,
                },
                cx,
            )
        })
        .unwrap();
        f.settle(window, cx);
        assert_eq!(
            f.catalog.state::<Preferences>("preferences").unwrap().dark,
            expected
        );
        cx.update_window(window, |_, window, cx| {
            assert_eq!(
                window
                    .within("appearance")
                    .find(match id {
                        "appearance-light" => 1_usize,
                        "appearance-dark" => 2,
                        _ => 0,
                    })
                    .checked(),
                Some(true)
            );
            let dark = expected.unwrap_or(matches!(
                window.appearance(),
                gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark
            ));
            assert_eq!(cx.theme().is_dark(), dark);
        })
        .unwrap();
    }
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    drop(f);
    cx.run_until_parked();
    let mut restarted = Fixture::open(&dir.path().join("catalog.sqlite"));
    let (window, _) = restarted.mount(cx);
    restarted.settle(window, cx);
    assert!(cx.update(|cx| cx.theme().is_dark()));
    restarted.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    crate::settings::page(&mut restarted, window, "Appearance", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.within("appearance").find(2_usize).checked(),
            Some(true)
        )
    })
    .unwrap();
}
