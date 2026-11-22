use crate::common::ui::{ARTIST, ARTIST_TARGET, Fixture, start};
use gpui_kit::{AnyWindowHandle, AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    artists::{ArtistProfile, cache_key},
    model::Preferences,
    platform::{Account, shortcut},
    service::{Command, Event},
};
use std::sync::Arc;

pub(super) fn page(f: &mut Fixture, window: AnyWindowHandle, name: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        let id = match name {
            "Library" => "0-0",
            "Playback" => "0-1",
            "Downloads" => "0-2",
            "Appearance" => "0-3",
            _ => panic!("unknown settings page"),
        };
        window.within("settings-sidebar").click(id, cx);
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
    for attempt in 0..2 {
        f.expect("sign-in intent without logging credentials", |command, _| {
            assert!(matches!(command, Command::Connect(account, password) if account.url == "https://fixture.invalid/" && account.username == "音楽 listener" && password == "synthetic-nav-password"));
            vec![]
        });
        f.click(window, "connect-library", cx);
        cx.update_window(window, |_, window, _| {
            assert_eq!(window.find("connect-library").disabled(), Some(true))
        })
        .unwrap();
        // A disabled button must reject repeated actual pointer input.
        f.click(window, "connect-library", cx);
        f.assert_script_finished();
        if attempt == 0 {
            f.backend
                .emit(Event::Error("Fixture sign-in rejected".into()));
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
        assert!(matches!(command, Command::SlskdAuth(key, user, password) if key == "synthetic-slskd-key" && user.is_empty() && password.is_empty())); vec![]
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
    f.expect("keep saved slskd auth", |command, _| { assert!(matches!(command, Command::SlskdAuth(k, u, p) if k.is_empty() && u.is_empty() && p.is_empty())); vec![] });
    f.expect("retry saved slskd auth", |command, _| {
        assert!(matches!(command, Command::SlskdTest));
        vec![Event::SlskdStatus(Ok("Fixture linked".into()))]
    });
    f.click(window, "link-slskd", cx);
    fill(window, "Username", "fixture-admin", cx);
    fill(window, "Password", "synthetic-slskd-password", cx);
    f.expect("slskd username and password", |command, _| { assert!(matches!(command, Command::SlskdAuth(k,u,p) if k.is_empty() && u == "fixture-admin" && p == "synthetic-slskd-password")); vec![] });
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

#[gpui_kit::test]
fn lastfm_username_is_saved_trimmed_for_memory_lane(cx: &mut TestAppContext) {
    let (_dir, mut f, window, _) = start(cx);
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "Library", cx);
    fill(window, "Last.fm username", "  listener ", cx);
    f.click(window, "save-lastfm", cx);
    let prefs: sonora::model::Preferences = f.catalog.state("preferences").unwrap();
    assert_eq!(prefs.lastfm_user, "listener");
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("Last.fm username").value().unwrap_or_default(),
            "listener"
        );
    })
    .unwrap();

    fill(window, "Last.fm username", " ", cx);
    f.click(window, "save-lastfm", cx);
    let prefs: sonora::model::Preferences = f.catalog.state("preferences").unwrap();
    assert_eq!(
        prefs.lastfm_user, "",
        "clearing the name turns Last.fm history off"
    );
    f.assert_script_finished();
}

#[gpui_kit::test]
fn lastfm_api_key_is_sent_once_and_cleared_from_the_field(cx: &mut TestAppContext) {
    let (_dir, mut f, window, _) = start(cx);
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    page(&mut f, window, "Library", cx);
    fill(window, "Last.fm username", "listener", cx);
    fill(window, "Last.fm API key", " synthetic-lastfm-key ", cx);
    f.expect("Last.fm key", |command, _| {
        assert!(matches!(command, Command::LastFmKey(key) if key == "synthetic-lastfm-key"));
        vec![]
    });
    f.click(window, "save-lastfm", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("Last.fm API key").value().unwrap_or_default(),
            ""
        );
    })
    .unwrap();
    let prefs: sonora::model::Preferences = f.catalog.state("preferences").unwrap();
    assert_eq!(prefs.lastfm_user, "listener");
    let saved = serde_json::to_string(&prefs).unwrap();
    assert!(!saved.contains("synthetic-lastfm-key"));

    // An empty key field keeps the saved key and sends nothing.
    f.click(window, "save-lastfm", cx);
    f.assert_script_finished();
}
