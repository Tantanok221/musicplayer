use crate::common::ui::{ARTIST, ARTIST_TARGET, Fixture, start};
use gpui_kit::{AnyWindowHandle, AppContext, TestAppContext, px, test::TestWindowExt};
use sonora::{
    artists::{ArtistProfile, cache_key},
    model::Preferences,
    platform::{Account, shortcut},
    service::{Command, Event, SettingsCredentials},
    tags::MusicFolderCheck,
};
use std::{path::PathBuf, sync::Arc, time::Duration};

pub(super) fn page(f: &mut Fixture, window: AnyWindowHandle, name: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        let id = match name {
            "Library" => "0-0",
            "Playback" => "0-1",
            "Downloads" => "0-2",
            "Appearance" => "0-3",
            "Metadata" => "0-5",
            "Paired devices" => "0-7",
            _ => panic!("unknown settings page"),
        };
        window.within("settings-sidebar").click(id, cx);
    })
    .unwrap();
    f.settle(window, cx);
}

/// Picks a ReplayGain mode, a setting that saves as soon as it changes.
fn gain(f: &mut Fixture, window: AnyWindowHandle, index: usize, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.within("replay-gain").click(index, cx)
    })
    .unwrap();
    f.settle(window, cx);
}

fn fill(window: AnyWindowHandle, field: &'static str, value: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.click(field, cx);
        window.press(shortcut("cmd-a", "ctrl-a"), cx);
        window.input(value, cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn paired_devices_settings_shows_pairing_code_and_revokes_a_device(cx: &mut TestAppContext) {
    use sonora::sync::lan::{LanCode, LanOfferState, LanPeer, LanStatus};

    let (_dir, mut f, window, _) = start(cx);
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "Paired devices", cx);
    fill(window, "LAN listener port", "8088", cx);
    f.click(window, "lan-save-port", cx);
    assert_eq!(
        f.catalog
            .state::<Preferences>("preferences")
            .unwrap()
            .lan_port,
        Some(8088)
    );
    cx.update_window(window, |_, window, _| {
        assert!(
            window.try_find("settings-saved").is_some(),
            "saving the LAN port must show the settings confirmation"
        );
    })
    .unwrap();
    f.backend.emit(Event::LanStatus(LanStatus {
        port: 8088,
        peers: vec![LanPeer {
            id: "speaker".into(),
            name: "Kitchen".into(),
            address: "127.0.0.1:8089".parse().unwrap(),
        }],
        ..Default::default()
    }));
    f.settle(window, cx);
    f.expect("generate LAN pairing code", |command, _| {
        assert!(matches!(command, Command::LanShowCode));
        vec![Event::LanOffer(LanOfferState::Available(LanCode(
            "123456".into(),
        )))]
    });
    f.click(window, "lan-show-code", cx);
    f.assert_script_finished();
    cx.update_window(window, |_, window, _| {
        let code = window.find("lan-pairing-code");
        assert!(code.visible());
        assert_eq!(code.label(), Some("123456 · use within 2 minutes"));
    })
    .unwrap();
    f.backend
        .emit(Event::LanOffer(LanOfferState::UsedOrExpired));
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(
            window.try_find("lan-pairing-code").is_none(),
            "a used or expired code must not stay visible"
        );
        let notice = window.find("lan-pairing-code-unavailable");
        assert!(notice.visible());
        assert_eq!(
            notice.label(),
            Some("Code used or expired — generate a new code")
        );
    })
    .unwrap();
    f.expect("replace used LAN pairing code", |command, _| {
        assert!(matches!(command, Command::LanShowCode));
        vec![Event::LanOffer(LanOfferState::Available(LanCode(
            "654321".into(),
        )))]
    });
    f.click(window, "lan-show-code", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("lan-pairing-code").label(),
            Some("654321 · use within 2 minutes")
        );
        assert!(window.try_find("lan-pairing-code-unavailable").is_none());
    })
    .unwrap();
    f.expect("revoke LAN device", |command, _| {
        assert!(matches!(command, Command::LanRevoke(device) if device == "speaker"));
        vec![Event::LanStatus(LanStatus {
            port: 8088,
            peers: vec![],
            ..Default::default()
        })]
    });
    f.click(window, "lan-revoke-speaker", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("lan-revoke-speaker").is_none());
    })
    .unwrap();
    f.assert_script_finished();
}

#[gpui_kit::test]
fn metadata_folder_starts_with_a_picker_and_writes_disabled(cx: &mut TestAppContext) {
    let (_dir, mut f, window, _) = start(cx);
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "Metadata", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("choose-music-folder").visible());
        assert!(window.find("check-music-folder").visible());
        assert_eq!(window.find("write-tags-to-files").disabled(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn picked_folder_checks_automatically_and_only_its_pass_unlocks_tag_writing(
    cx: &mut TestAppContext,
) {
    let (_dir, mut f, window, _) = start(cx);
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "Metadata", cx);
    let assert_gate = |disabled, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            assert_eq!(
                window
                    .find("write-tags-to-files")
                    .disabled()
                    .unwrap_or(false),
                disabled
            );
        })
        .unwrap();
    };
    f.click(window, "choose-music-folder", cx);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| None);
    f.settle(window, cx);
    assert_gate(true, cx);

    let folder = PathBuf::from("/fixture/music");
    let checked = folder.clone();
    f.expect(
        "automatically check the picked folder",
        move |command, _| {
            let Command::CheckMusicFolder(path, reply) = command else {
                panic!("expected a music folder check");
            };
            assert_eq!(path, checked);
            reply
                .send(MusicFolderCheck::Passed {
                    found: 5,
                    server_path: "/music/Artist/Song.flac".into(),
                    local_path: path.join("Artist/Song.flac"),
                })
                .unwrap();
            vec![]
        },
    );
    f.click(window, "choose-music-folder", cx);
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && !options.multiple);
        Some(vec![folder])
    });
    f.settle(window, cx);
    assert_gate(false, cx);
    cx.update_window(window, |_, window, _| {
        let result = window.find("music-folder-check-result");
        assert!(result.visible());
        assert!(result.label().unwrap().contains("Found 5/5 sample tracks"));
    })
    .unwrap();
    assert!(
        !f.catalog
            .state::<Preferences>("preferences")
            .unwrap()
            .metadata
            .write_tags
    );
    f.click(window, "write-tags-to-files", cx);
    assert!(
        f.catalog
            .state::<Preferences>("preferences")
            .unwrap()
            .metadata
            .write_tags
    );

    fill(window, "Music folder", "/fixture/missing", cx);
    f.settle(window, cx);
    assert_gate(true, cx);
    f.expect("check a pasted folder", |command, _| {
        let Command::CheckMusicFolder(path, reply) = command else {
            panic!("expected a music folder check");
        };
        assert_eq!(path, PathBuf::from("/fixture/missing"));
        reply.send(MusicFolderCheck::MissingFolder).unwrap();
        vec![]
    });
    f.click(window, "check-music-folder", cx);
    assert_gate(true, cx);
    f.click(window, "save-music-folder", cx);
    assert!(
        !f.catalog
            .state::<Preferences>("preferences")
            .unwrap()
            .metadata
            .write_tags
    );
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("settings-saved").label(),
            Some("Music folder saved · not verified")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn settings_save_toast_expires_and_ignores_navigation_saves(cx: &mut TestAppContext) {
    let (_dir, mut f, window, _) = start(cx);
    let toast = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            window.try_find("settings-saved").map(|toast| {
                assert!(toast.visible());
                assert_eq!(toast.label(), Some("Settings saved"));
                let bounds = toast.bounds();
                let status = window.find("library-status").bounds();
                assert!(
                    bounds.bottom() < status.top(),
                    "leave playback controls clear"
                );
                assert!(status.right() - bounds.right() < px(40.));
            })
        })
        .unwrap()
        .is_some()
    };

    let saves = f.saves;
    f.click(window, "new-tab", cx);
    cx.executor().advance_clock(Duration::from_millis(500));
    f.settle(window, cx);
    assert!(f.saves > saves, "navigation must have persisted");
    assert!(!toast(cx), "navigation persistence must not show a toast");
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "Playback", cx);
    assert!(
        !toast(cx),
        "opening Settings without saving must not show a toast"
    );
    gain(&mut f, window, 1, cx);
    assert_eq!(
        f.catalog.state::<Preferences>("preferences").unwrap().gain,
        sonora::model::GainMode::Track
    );
    assert!(toast(cx), "a successful Settings save must show a toast");

    cx.executor().advance_clock(Duration::from_secs(1));
    f.settle(window, cx);
    gain(&mut f, window, 2, cx);
    cx.executor().advance_clock(Duration::from_millis(1500));
    f.settle(window, cx);
    assert!(toast(cx), "a newer save must renew the toast");
    cx.executor().advance_clock(Duration::from_secs(1));
    f.settle(window, cx);
    assert!(!toast(cx), "the toast must auto-dismiss");
}

#[gpui_kit::test]
fn settings_save_failure_does_not_show_success(cx: &mut TestAppContext) {
    let (dir, mut f, window, _) = start(cx);
    let db = rusqlite::Connection::open(dir.path().join("catalog.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_preferences BEFORE INSERT ON state
         WHEN NEW.key = 'preferences' BEGIN SELECT RAISE(FAIL, 'test write failed'); END;",
    )
    .unwrap();
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "Playback", cx);
    gain(&mut f, window, 1, cx);
    assert_eq!(
        f.catalog.state::<Preferences>("preferences").unwrap().gain,
        sonora::model::GainMode::Smart
    );
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("settings-saved").is_none());
        assert_eq!(
            window.find("settings-save-failed").label(),
            Some("Could not save settings")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn reconnect_keeps_artist_profile_but_account_switch_drops_it(cx: &mut TestAppContext) {
    let (_dir, mut f, window, app) = start(cx);
    f.click(window, "new-tab", cx);
    f.click(window, ARTIST_TARGET, cx);
    f.backend.emit(Event::ArtistProfile(
        cache_key(ARTIST, "artist-1"),
        Arc::new(ArtistProfile {
            name: ARTIST.into(),
            url: "https://fixture.invalid/artist".into(),
            complete: true,
            ..Default::default()
        }),
    ));
    f.settle(window, cx);

    let assert_profile = |expected, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            assert!(window.find("artist-release-album-1").visible());
            assert_eq!(
                window.try_find("artist-source").is_some(),
                expected,
                "the previous account's cached artist profile must not survive an account switch"
            );
        })
        .unwrap();
    };
    assert_profile(true, cx);

    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "Library", cx);
    f.expect("same-account reconnect", |command, _| {
        let Command::Connect(account, _) = command else {
            panic!("expected connect command");
        };
        assert_eq!(account.username, "fixture");
        vec![Event::Connected(account, "Connected".into())]
    });
    f.click(window, "connect-library", cx);
    assert_profile(true, cx);

    let prefs = Preferences {
        // Both accounts have a saved tab for the same artist ID.
        tabs: cx.update(|cx| app.read(cx).tab_state().clone()),
        ..f.catalog.state("preferences").unwrap()
    };
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "Library", cx);
    fill(window, "Username", "other-listener", cx);
    f.expect("switch to another account", move |command, _| {
        let Command::Connect(account, _) = command else {
            panic!("expected connect command");
        };
        assert_eq!(account.username, "other-listener");
        vec![
            Event::Ready(Some(account.clone()), prefs),
            Event::Connected(account, "Connected".into()),
        ]
    });
    f.click(window, "connect-library", cx);
    assert_profile(false, cx);
    f.assert_script_finished();
}

#[gpui_kit::test]
fn first_sign_in_failure_preserves_input_reenables_connect_and_retry_succeeds(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(sonora::ui::settings_bindings());
    });
    let dir = tempfile::tempdir().unwrap();
    let mut f = Fixture::open(&dir.path().join("catalog.sqlite"));
    f.seed();
    f.signed_out();
    let (window, _) = f.mount(cx);
    f.settle(window, cx);
    f.backend
        .emit(Event::SignInRequired("Sign in to your library".into()));
    f.settle(window, cx);
    fill(window, "Server address", "https://fixture.invalid/", cx);
    fill(window, "Username", "音楽 listener", cx);
    fill(window, "Password", "synthetic-nav-password", cx);
    for attempt in 0..3 {
        let (button, password) = match attempt {
            0 => ("connect-library", "synthetic-nav-password"),
            1 => ("retry-error", "synthetic-nav-password"),
            _ => {
                fill(window, "Password", "synthetic-replacement", cx);
                ("retry-error", "synthetic-replacement")
            }
        };
        f.expect("sign-in intent without logging credentials", move |command, _| {
            assert!(matches!(command, Command::Connect(account, submitted) if account.url == "https://fixture.invalid/" && account.username == "音楽 listener" && submitted == password), "Connect and sign-in Retry must send the current form values");
            vec![]
        });
        f.click(window, button, cx);
        cx.update_window(window, |_, window, _| {
            assert_eq!(window.find("connect-library").disabled(), Some(true));
            assert!(window.try_find("error-banner").is_none());
        })
        .unwrap();
        // A disabled button must reject repeated actual pointer input.
        f.click(window, "connect-library", cx);
        f.assert_script_finished();
        if attempt < 2 {
            f.backend
                .emit(Event::SignInFailed("Fixture sign-in rejected".into()));
            f.settle(window, cx);
            cx.update_window(window, |_, window, _| {
                assert!(window.find("error-banner").visible());
                assert_eq!(
                    window.find("Server address").value(),
                    Some("https://fixture.invalid/")
                );
                assert_eq!(window.find("Username").value(), Some("音楽 listener"));
                assert!(
                    !window
                        .find("Password")
                        .value()
                        .unwrap_or_default()
                        .contains("synthetic-nav-password")
                );
                assert_ne!(
                    window.find("connect-library").disabled(),
                    Some(true),
                    "failed sign-in must enable another attempt"
                );
            })
            .unwrap();
        }
    }
    f.backend.emit(Event::Connected(
        Account {
            url: "https://fixture.invalid/".into(),
            username: "音楽 listener".into(),
        },
        "Connected".into(),
    ));
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("connect-library").is_none());
        assert!(window.try_find("error-banner").is_none());
    })
    .unwrap();
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("connect-library").visible());
        assert_eq!(window.find("Password").value().unwrap_or_default(), "");
    })
    .unwrap();

    f.backend
        .emit(Event::Error("Fixture refresh failed".into()));
    f.settle(window, cx);
    f.expect("ordinary errors still refresh", |command, _| {
        assert!(matches!(command, Command::Refresh));
        vec![]
    });
    f.click(window, "retry-error", cx);
    f.assert_script_finished();
}

#[gpui_kit::test]
fn soulseek_key_and_login_submit_clear_secrets_and_keep_saved_auth_on_empty_retry(
    cx: &mut TestAppContext,
) {
    let (_dir, mut f, window, _) = start(cx);
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "Downloads", cx);
    fill(window, "slskd address", " http://127.0.0.1:5030/ ", cx);
    fill(window, "API key", "synthetic-slskd-key", cx);
    f.expect("slskd API-key auth", |command, _| {
        assert!(matches!(command, Command::SaveSettings(_, SettingsCredentials::Slskd(key, user, password), _) if key == "synthetic-slskd-key" && user.is_empty() && password.is_empty())); vec![]
    });
    f.expect("slskd test after credentials", |command, _| {
        assert!(matches!(command, Command::SlskdTest));
        vec![]
    });
    f.click(window, "link-slskd", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("API key").value().unwrap_or_default(), "");
        assert_eq!(window.find("Password").value().unwrap_or_default(), "");
    })
    .unwrap();
    let prefs: sonora::model::Preferences = f.catalog.state("preferences").unwrap();
    assert_eq!(prefs.downloads.url, "http://127.0.0.1:5030/");
    assert!(prefs.downloads.enabled);
    f.backend.emit(Event::SlskdStatus(Err(
        "Fixture slskd rejected login".into()
    )));
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(
            window
                .find("slskd-status")
                .label()
                .unwrap()
                .contains("Fixture slskd rejected login")
        )
    })
    .unwrap();
    // Blank fields mean reuse the saved auth, not erase it.
    f.expect("keep saved slskd auth", |command, _| { assert!(matches!(command, Command::SaveSettings(_, SettingsCredentials::Slskd(k, u, p), _) if k.is_empty() && u.is_empty() && p.is_empty())); vec![] });
    f.expect("retry saved slskd auth", |command, _| {
        assert!(matches!(command, Command::SlskdTest));
        vec![Event::SlskdStatus(Ok("Fixture linked".into()))]
    });
    f.click(window, "link-slskd", cx);
    fill(window, "Username", "fixture-admin", cx);
    fill(window, "Password", "synthetic-slskd-password", cx);
    f.expect("slskd username and password", |command, _| { assert!(matches!(command, Command::SaveSettings(_, SettingsCredentials::Slskd(k,u,p), _) if k.is_empty() && u == "fixture-admin" && p == "synthetic-slskd-password")); vec![] });
    f.expect("test replacement slskd login", |command, _| {
        assert!(matches!(command, Command::SlskdTest));
        vec![Event::SlskdStatus(Ok("Fixture linked".into()))]
    });
    f.click(window, "link-slskd", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("Username").value().unwrap_or_default(), "");
        assert_eq!(window.find("Password").value().unwrap_or_default(), "");
        assert!(
            window
                .find("slskd-status")
                .label()
                .unwrap()
                .contains("Fixture linked")
        );
    })
    .unwrap();
    f.assert_script_finished();
}
