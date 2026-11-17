use crate::common::ui::{Fixture, start};
use gpui_kit::{AnyWindowHandle, AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    platform::{Account, shortcut},
    service::{Command, Event},
};

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
