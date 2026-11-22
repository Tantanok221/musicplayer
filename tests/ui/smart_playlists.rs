use crate::common::ui::{Fixture, start};
use gpui_kit::{
    AnyWindowHandle, AppContext, ScrollDelta, TestAppContext, point, px,
    test::{TestQueryExt, TestWindowExt},
};
use serde_json::json;
use sonora::{
    model::Playlist,
    platform::shortcut,
    service::{Command, Event},
};

fn edit(f: &mut Fixture, w: AnyWindowHandle, id: &str, text: &str, cx: &mut TestAppContext) {
    cx.update_window(w, |_, window, cx| {
        window.click(id.to_string(), cx);
        window.press(shortcut("cmd-a", "ctrl-a"), cx);
        window.press("backspace", cx);
        window.input(text, cx);
    })
    .unwrap();
    f.settle(w, cx);
}

fn reveal_playlists(f: &mut Fixture, w: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(w, |_, window, cx| {
        window.scroll(
            "playlist-disclosure",
            ScrollDelta::Pixels(point(px(0.), px(-150.))),
            cx,
        )
    })
    .unwrap();
    f.settle(w, cx);
}

#[gpui_kit::test]
fn create_validates_conditions_and_sends_rules(cx: &mut TestAppContext) {
    let (_dir, mut f, w, _app) = start(cx);
    f.backend.emit(Event::NativeAvailable(true));
    f.settle(w, cx);
    f.click(w, "new-smart-playlist", cx);
    f.click(w, "save-smart-playlist", cx);
    cx.update_window(w, |_, window, _| {
        assert!(window.find_by_label("Enter a playlist name").visible())
    })
    .unwrap();
    edit(&mut f, w, "smart-name", "Jazz discoveries", cx);
    f.click(w, "save-smart-playlist", cx);
    cx.update_window(w, |_, window, _| {
        assert!(
            window
                .find_by_label("Enter a value for every condition")
                .visible()
        )
    })
    .unwrap();
    f.click(w, "smart-field-0", cx);
    f.menu_item(w, "Genre", cx);
    edit(&mut f, w, "smart-value-0", "Jazz", cx);
    f.expect("save smart rules", |c, _| {
        let Command::SaveSmartPlaylist { id, name, rules } = c else {
            panic!("expected save")
        };
        assert!(id.is_none());
        assert_eq!(name, "Jazz discoveries");
        assert_eq!(
            serde_json::to_value(rules).unwrap(),
            json!({"all":[{"is":{"genre":"Jazz"}}]})
        );
        vec![Event::SmartPlaylistSaved("created".into())]
    });
    f.click(w, "save-smart-playlist", cx);
    cx.update_window(w, |_, window, _| {
        assert!(window.try_find("smart-editor").is_none())
    })
    .unwrap();
    f.assert_script_finished();
}

#[gpui_kit::test]
fn edit_preserves_advanced_rules_and_saves_update(cx: &mut TestAppContext) {
    let (_dir, mut f, w, _app) = start(cx);
    f.backend.emit(Event::NativeAvailable(true));
    let playlist = Playlist {
        id: "smart".into(),
        name: "Advanced Jazz".into(),
        owner: "fixture".into(),
        readonly: true,
        valid_until: Some("2999-01-01T00:00:00Z".into()),
        ..Default::default()
    };
    f.backend.emit(Event::Playlists(vec![playlist.clone()]));
    f.settle(w, cx);
    f.expect("open smart", move |c, _| {
        assert!(matches!(c,Command::Playlist(id) if id=="smart"));
        vec![Event::Playlist(playlist)]
    });
    reveal_playlists(&mut f, w, cx);
    f.click(w, "nav-playlist-smart", cx);
    f.click(w, "playlist-options", cx);
    let value = json!({"any":[{"is":{"genre":"Jazz"}},{"futureOp":{"x":[1,2]}}],"offset":4,"sort":"random"});
    let loaded = value.clone();
    f.expect("load rules", move |c, _| {
        assert!(matches!(c,Command::LoadSmartRules(id) if id=="smart"));
        vec![Event::SmartRules(
            "smart".into(),
            Some(serde_json::from_value(loaded).unwrap()),
        )]
    });
    f.menu_item(w, "Edit Rules…", cx);
    cx.update_window(w, |_, window, _| {
        assert!(window.find_by_label("Advanced rule — kept as is").visible());
        assert!(window.try_find("smart-remove-1").is_none());
    })
    .unwrap();
    edit(&mut f, w, "smart-name", "Advanced renamed", cx);
    f.expect("update rules", move |c, _| {
        let Command::SaveSmartPlaylist { id, name, rules } = c else {
            panic!("expected update")
        };
        assert_eq!(id.as_deref(), Some("smart"));
        assert_eq!(name, "Advanced renamed");
        assert_eq!(serde_json::to_value(rules).unwrap(), value);
        vec![Event::SmartPlaylistSaved("smart".into())]
    });
    f.click(w, "save-smart-playlist", cx);
    f.assert_script_finished();
}

#[gpui_kit::test]
fn password_prompt_submits_once_clears_password_and_can_cancel(cx: &mut TestAppContext) {
    let (_dir, mut f, w, _app) = start(cx);
    f.backend.emit(Event::NativeAvailable(true));
    f.settle(w, cx);
    f.click(w, "new-smart-playlist", cx);
    edit(&mut f, w, "smart-name", "Loved", cx);
    f.click(w, "smart-template", cx);
    f.menu_item(w, "Loved", cx);
    f.expect("password needed", |c, _| {
        assert!(matches!(c, Command::SaveSmartPlaylist { .. }));
        vec![Event::NativePasswordRequired]
    });
    f.click(w, "save-smart-playlist", cx);
    cx.update_window(w, |_, window, _| {
        assert!(
            window
                .find_by_label("Your password is not stored.")
                .visible()
        )
    })
    .unwrap();
    edit(&mut f, w, "native-password", "fixture-password", cx);
    f.expect("native sign in", |c, _| {
        assert!(matches!(c,Command::NativeSignIn(password) if password=="fixture-password"));
        vec![Event::Error(
            "Incorrect Navidrome username or password".into(),
        )]
    });
    f.click(w, "native-sign-in", cx);
    f.click(w, "native-sign-in", cx);
    cx.update_window(w, |_, window, _| {
        assert!(
            window
                .find_by_label("Enter your Navidrome password")
                .visible()
        )
    })
    .unwrap();
    f.expect("cancel pending save", |c, _| {
        assert!(matches!(c, Command::CancelSmartPlaylist));
        vec![]
    });
    f.click(w, "cancel-smart-playlist", cx);
    f.click(w, "new-smart-playlist", cx);
    cx.update_window(w, |_, window, _| {
        assert!(window.try_find("native-password").is_none())
    })
    .unwrap();
    f.assert_script_finished();
}

#[gpui_kit::test]
fn failed_sign_in_can_retry_the_pending_save(cx: &mut TestAppContext) {
    let (_dir, mut f, w, _app) = start(cx);
    f.backend.emit(Event::NativeAvailable(true));
    f.settle(w, cx);
    f.click(w, "new-smart-playlist", cx);
    f.click(w, "smart-template", cx);
    f.menu_item(w, "Loved", cx);
    f.expect("authorize save", |c, _| {
        assert!(matches!(c, Command::SaveSmartPlaylist { .. }));
        vec![Event::NativePasswordRequired]
    });
    f.click(w, "save-smart-playlist", cx);
    edit(&mut f, w, "native-password", "wrong", cx);
    f.expect("failed sign in", |c, _| {
        assert!(matches!(c, Command::NativeSignIn(_)));
        vec![Event::Error(
            "Incorrect Navidrome username or password".into(),
        )]
    });
    f.click(w, "native-sign-in", cx);
    edit(&mut f, w, "native-password", "correct", cx);
    f.expect("retry sign in", |c, _| {
        assert!(matches!(c,Command::NativeSignIn(p) if p=="correct"));
        vec![]
    });
    f.click(w, "native-sign-in", cx);
    f.click(w, "native-sign-in", cx);
    f.backend.emit(Event::SmartPlaylistSaved("saved".into()));
    f.settle(w, cx);
    cx.update_window(w, |_, window, _| {
        assert!(window.try_find("smart-editor").is_none());
        assert!(window.try_find("error-banner").is_none());
    })
    .unwrap();
    f.assert_script_finished();
}

#[gpui_kit::test]
fn templates_groups_and_condition_controls_are_editable(cx: &mut TestAppContext) {
    let (_dir, mut f, w, _app) = start(cx);
    f.backend.emit(Event::NativeAvailable(true));
    f.settle(w, cx);
    f.click(w, "new-smart-playlist", cx);
    f.click(w, "smart-template", cx);
    f.menu_item(w, "Most Played", cx);
    f.click(w, "smart-add-condition", cx);
    f.click(w, "smart-remove-1", cx);
    f.click(w, "smart-add-group", cx);
    f.click(w, "smart-match-2", cx);
    f.menu_item(w, "Match any", cx);
    f.click(w, "smart-remove-2", cx);
    f.expect("save template", |c, _| {
        let Command::SaveSmartPlaylist { rules, .. } = c else {
            panic!("expected save")
        };
        assert_eq!(
            serde_json::to_value(rules).unwrap(),
            json!({"all":[{"gt":{"playcount":0}}],"sort":"playcount","order":"desc","limit":100})
        );
        vec![]
    });
    f.click(w, "save-smart-playlist", cx);
    f.assert_script_finished();
}

#[gpui_kit::test]
fn entry_points_require_navidrome_and_edit_requires_owner(cx: &mut TestAppContext) {
    let (_dir, mut f, w, _app) = start(cx);
    cx.update_window(w, |_, window, _| {
        assert!(window.try_find("new-smart-playlist").is_none())
    })
    .unwrap();
    for (native, owner) in [(false, "fixture"), (true, "someone-else")] {
        f.backend.emit(Event::NativeAvailable(native));
        let p = Playlist {
            id: "smart".into(),
            name: "Read only".into(),
            owner: owner.into(),
            readonly: true,
            valid_until: Some("2999-01-01T00:00:00Z".into()),
            ..Default::default()
        };
        f.backend.emit(Event::Playlists(vec![p.clone()]));
        f.settle(w, cx);
        f.expect("open readonly", move |c, _| {
            assert!(matches!(c, Command::Playlist(_)));
            vec![Event::Playlist(p)]
        });
        reveal_playlists(&mut f, w, cx);
        f.click(w, "nav-playlist-smart", cx);
        if owner == "fixture" {
            f.click(w, "playlist-options", cx);
            cx.update_window(w, |_, window, cx| {
                assert!(
                    window
                        .within("popup-menu")
                        .try_find_by_label("Edit Rules…")
                        .is_none()
                );
                window.press("escape", cx);
            })
            .unwrap();
            f.settle(w, cx);
        } else {
            cx.update_window(w, |_, window, _| {
                assert!(window.try_find("playlist-options").is_none())
            })
            .unwrap();
        }
    }
    f.assert_script_finished();
}
