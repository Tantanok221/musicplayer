use crate::fixture::{Fixture, Library, WAIT, event, synced};
use serde_json::{Value, json};
use sonora::{
    service::{Command, Event, test_support::Worker},
    smart_rules::{RuleGroup, RuleNode, SmartRules},
};

fn rules() -> SmartRules {
    SmartRules {
        root: RuleGroup {
            any: false,
            items: vec![RuleNode::Condition {
                op: "is".into(),
                field: "genre".into(),
                value: json!("Jazz"),
            }],
        },
        sort: Some("random".into()),
        ..SmartRules::default()
    }
}

#[test]
fn password_sign_in_reuses_native_session_without_blocking_connection() {
    let (release, gate) = flume::bounded(1);
    let mut library = Library::default();
    library.native.login_gate = Some(gate);
    let fixture = Fixture::new(library);
    std::fs::remove_file(fixture.paths.data.join("account.json")).unwrap();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    worker.service().send(Command::Connect(
        fixture.account.clone(),
        "native-password".into(),
    ));
    event(&worker, |e| matches!(e, Event::Connected(..)));
    synced(&worker);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    release.send(()).unwrap();
    event(&worker, |e| {
        assert!(
            !matches!(e, Event::NativePasswordRequired),
            "sign-in should supply the native session"
        );
        matches!(e, Event::SmartRules(_, Some(_)))
    });
    assert!(
        fixture
            .paths
            .native_token(&fixture.account)
            .unwrap()
            .is_some()
    );
    worker.service().send(Command::SaveSmartPlaylist {
        id: None,
        name: "Jazz".into(),
        rules: rules(),
    });
    event(&worker, |e| {
        assert!(!matches!(e, Event::NativePasswordRequired));
        matches!(e, Event::SmartPlaylistSaved(_))
    });
    assert_eq!(fixture.native_login_count(), 1);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn native_sign_in_failure_is_silent_and_editor_falls_back() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Connect(
        fixture.account.clone(),
        "subsonic-only-password".into(),
    ));
    event(&worker, |e| matches!(e, Event::Connected(..)));
    synced(&worker);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    password(&worker);
    assert_eq!(fixture.native_login_count(), 1);
    assert!(
        fixture
            .paths
            .native_token(&fixture.account)
            .unwrap()
            .is_none()
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn password_account_switch_stores_the_session_only_for_the_new_account() {
    let a = Fixture::new(Library::default());
    let b = Fixture::new(Library::default());
    let mut worker = a.start_paused();
    event(&worker, |e| matches!(e, Event::Ready(..)));
    synced(&worker);
    worker.service().send(Command::Connect(
        b.account.clone(),
        "native-password".into(),
    ));
    event(
        &worker,
        |e| matches!(e,Event::Connected(account,_) if account.namespace()==b.account.namespace()),
    );
    synced(&worker);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    event(&worker, |e| {
        assert!(!matches!(e, Event::NativePasswordRequired));
        matches!(e, Event::SmartRules(..))
    });
    assert!(a.paths.native_token(&a.account).unwrap().is_none());
    assert!(a.paths.native_token(&b.account).unwrap().is_some());
    assert_eq!(a.native_login_count(), 0);
    assert_eq!(b.native_login_count(), 1);
    worker.shutdown(WAIT).unwrap();
    a.finish();
    b.finish();
}

#[test]
fn non_navidrome_password_sign_in_skips_native_login() {
    let library = Library {
        server_type: "Subsonic",
        ..Library::default()
    };
    let fixture = Fixture::new(library);
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Connect(
        fixture.account.clone(),
        "native-password".into(),
    ));
    event(&worker, |e| matches!(e, Event::Connected(..)));
    synced(&worker);
    assert_eq!(fixture.native_login_count(), 0);
    assert!(
        fixture
            .paths
            .native_token(&fixture.account)
            .unwrap()
            .is_none()
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn cancel_pending_save_does_not_retry_it_after_sign_in() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::SaveSmartPlaylist {
        id: None,
        name: "Cancelled".into(),
        rules: rules(),
    });
    password(&worker);
    worker.service().send(Command::CancelSmartPlaylist);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    password(&worker);
    sign_in(&worker);
    loaded(&worker, Some(rules()));
    let requests = fixture.native_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, axum::http::Method::GET);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn initialize_publishes_capability_after_account_state() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Initialize);
    assert!(matches!(
        worker.events().recv_timeout(WAIT).unwrap(),
        Event::Ready(..)
    ));
    event(&worker, |e| matches!(e, Event::NativeAvailable(true)));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

fn password(worker: &Worker) {
    event(worker, |e| matches!(e, Event::NativePasswordRequired));
}

fn sign_in(worker: &Worker) {
    worker
        .service()
        .send(Command::NativeSignIn("native-password".into()));
}

fn loaded(worker: &Worker, expected: Option<SmartRules>) {
    let Event::SmartRules(id, rules) = event(worker, |e| matches!(e, Event::SmartRules(..))) else {
        unreachable!()
    };
    assert_eq!(id, "smart");
    assert_eq!(rules, expected);
}

fn token(fixture: &Fixture) -> Option<Value> {
    let path = fixture
        .paths
        .data
        .join("credentials")
        .join(format!("{}.native.json", fixture.account.namespace()));
    std::fs::read(path)
        .ok()
        .map(|b| serde_json::from_slice(&b).unwrap())
}

#[test]
fn password_required_sign_in_retries_load_and_persists_refreshed_token() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    password(&worker);
    assert!(fixture.native_requests().is_empty());
    fixture.edit_library(|l| l.native.refresh = Some("native-refreshed".into()));
    sign_in(&worker);
    loaded(&worker, Some(rules()));
    assert_eq!(token(&fixture).unwrap()["token"], "native-refreshed");
    worker.shutdown(WAIT).unwrap();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    loaded(&worker, Some(rules()));
    assert_eq!(fixture.native_login_count(), 1);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn create_and_update_refresh_subsonic_summaries_and_detail() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::SaveSmartPlaylist {
        id: None,
        name: "Jazz".into(),
        rules: rules(),
    });
    password(&worker);
    sign_in(&worker);
    event(
        &worker,
        |e| matches!(e, Event::SmartPlaylistSaved(id) if id == "smart"),
    );
    event(
        &worker,
        |e| matches!(e, Event::Playlists(ps) if ps.iter().any(|p| p.id == "smart" && p.name == "Jazz")),
    );
    event(
        &worker,
        |e| matches!(e, Event::Playlist(p) if p.id == "smart" && p.entry.len() == 1),
    );
    worker.service().send(Command::SaveSmartPlaylist {
        id: Some("smart".into()),
        name: "More Jazz".into(),
        rules: rules(),
    });
    event(
        &worker,
        |e| matches!(e, Event::SmartPlaylistSaved(id) if id == "smart"),
    );
    event(
        &worker,
        |e| matches!(e, Event::Playlists(ps) if ps.iter().any(|p| p.name == "More Jazz")),
    );
    event(
        &worker,
        |e| matches!(e, Event::Playlist(p) if p.name == "More Jazz"),
    );
    let requests = fixture.native_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0],
        (
            axum::http::Method::POST,
            "/api/playlist".into(),
            json!({"name":"Jazz","comment":"","public":false,"rules":{"all":[{"is":{"genre":"Jazz"}}],"sort":"random"}})
        )
    );
    assert_eq!(
        requests[1],
        (
            axum::http::Method::PUT,
            "/api/playlist/smart".into(),
            json!({"name":"More Jazz","rules":{"all":[{"is":{"genre":"Jazz"}}],"sort":"random"}})
        )
    );
    let saved = fixture.catalog().playlist("smart").unwrap().unwrap();
    assert_eq!(saved.entry[0].id, "song");
    assert!(saved.readonly);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn wrong_password_keeps_pending_request_and_expired_token_prompts_again() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    password(&worker);
    worker
        .service()
        .send(Command::NativeSignIn("wrong-password".into()));
    event(
        &worker,
        |e| matches!(e, Event::Error(s) if s == "Incorrect Navidrome username or password"),
    );
    assert!(token(&fixture).is_none());
    sign_in(&worker);
    loaded(&worker, Some(rules()));
    fixture.edit_library(|l| l.native.reject_token = true);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    password(&worker);
    assert!(token(&fixture).is_none());
    sign_in(&worker);
    loaded(&worker, Some(rules()));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn null_rules_and_server_errors_are_safe_and_do_not_retry_mutations() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    fixture.edit_library(|l| l.native.rules = None);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    password(&worker);
    sign_in(&worker);
    loaded(&worker, None);
    fixture.edit_library(|l| l.native.failure = Some(axum::http::StatusCode::FORBIDDEN));
    worker.service().send(Command::SaveSmartPlaylist {
        id: None,
        name: "Denied".into(),
        rules: rules(),
    });
    event(
        &worker,
        |e| matches!(e, Event::Error(s) if s == "Navidrome denied this playlist operation"),
    );
    assert_eq!(fixture.native_requests().len(), 2);
    assert!(fixture.catalog().playlist("smart").unwrap().is_none());
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn non_navidrome_server_refuses_native_commands() {
    let fixture = Fixture::new(Library {
        server_type: "Gonic",
        ..Library::default()
    });
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    for command in [
        Command::LoadSmartRules("smart".into()),
        Command::SaveSmartPlaylist {
            id: None,
            name: "No".into(),
            rules: rules(),
        },
        Command::NativeSignIn("native-password".into()),
    ] {
        worker.service().send(command);
        event(
            &worker,
            |e| matches!(e, Event::Error(s) if s == "Smart playlist editing requires Navidrome"),
        );
    }
    assert!(fixture.native_requests().is_empty());
    assert_eq!(fixture.native_login_count(), 0);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn advanced_rules_survive_load_update_and_empty_rules_load() {
    let value = json!({"any":[{"futureOp":{"genre":["Jazz","Blues"]}},{"all":[{"gt":{"playcount":5}},{"is":{"year":1990,"genre":"Jazz"}}]}],"offset":7,"sort":"-year,title","order":"asc","limitPercent":25,"refreshDelay":"1d"});
    let mut library = Library::default();
    library.native.rules = Some(value.clone());
    let fixture = Fixture::new(library);
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    password(&worker);
    sign_in(&worker);
    let Event::SmartRules(_, Some(rules)) = event(&worker, |e| matches!(e, Event::SmartRules(..)))
    else {
        panic!("expected rules")
    };
    assert_eq!(serde_json::to_value(&rules).unwrap(), value);
    worker.service().send(Command::SaveSmartPlaylist {
        id: Some("smart".into()),
        name: "Advanced".into(),
        rules,
    });
    event(&worker, |e| matches!(e, Event::SmartPlaylistSaved(_)));
    event(&worker, |e| matches!(e, Event::Playlist(_)));
    assert_eq!(fixture.native_requests()[1].2["rules"], value);
    fixture.edit_library(|l| l.native.rules = Some(json!({"all":[]})));
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    loaded(&worker, Some(SmartRules::default()));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn switching_accounts_discards_pending_edits_and_scopes_tokens() {
    let a = Fixture::new(Library::default());
    let b = Fixture::new(Library::default());
    let mut worker = a.start_paused();
    event(&worker, |e| matches!(e, Event::Ready(..)));
    synced(&worker);
    worker.service().send(Command::SaveSmartPlaylist {
        id: None,
        name: "Never send to B".into(),
        rules: rules(),
    });
    password(&worker);
    worker.service().send(Command::Connect(
        b.account.clone(),
        "synthetic-password".into(),
    ));
    event(
        &worker,
        |e| matches!(e, Event::Connected(account, _) if account.namespace() == b.account.namespace()),
    );
    synced(&worker);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    password(&worker);
    sign_in(&worker);
    loaded(&worker, Some(rules()));
    assert!(a.native_requests().is_empty());
    assert_eq!(b.native_requests().len(), 1);
    assert_eq!(b.native_requests()[0].0, axum::http::Method::GET);
    assert!(a.paths.native_token(&a.account).unwrap().is_none());
    assert!(a.paths.native_token(&b.account).unwrap().is_some());
    worker.shutdown(WAIT).unwrap();
    a.finish();
    b.finish();
}

#[test]
fn malformed_rules_are_user_safe_and_do_not_replace_cached_playlist() {
    let mut library = Library::default();
    library.native.rules = Some(json!({"all":[],"any":[{"is":{"secret-field":"secret-value"}}]}));
    let fixture = Fixture::new(library);
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    password(&worker);
    sign_in(&worker);
    event(
        &worker,
        |e| matches!(e, Event::Error(s) if s == "Navidrome returned invalid smart playlist rules"),
    );
    assert!(fixture.catalog().playlist("smart").unwrap().is_none());
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn native_redirects_do_not_forward_tokens_to_another_server() {
    let a = Fixture::new(Library::default());
    let b = Fixture::new(Library::default());
    let mut worker = a.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    password(&worker);
    sign_in(&worker);
    loaded(&worker, Some(rules()));
    a.edit_library(|l| l.native.redirect = Some(format!("{}api/playlist/smart", b.account.url)));
    worker
        .service()
        .send(Command::LoadSmartRules("smart".into()));
    event(
        &worker,
        |e| matches!(e, Event::Error(s) if s == "Navidrome could not complete the playlist operation"),
    );
    assert!(b.native_requests().is_empty());
    worker.shutdown(WAIT).unwrap();
    a.finish();
    b.finish();
}

#[test]
fn saved_playlist_with_failed_subsonic_refresh_keeps_cached_detail() {
    let mut library = Library::default();
    let cached = sonora::model::Playlist {
        id: "smart".into(),
        name: "Cached".into(),
        owner: "fixture".into(),
        readonly: true,
        entry: vec![std::sync::Arc::new(library.tracks[1].clone())].into(),
        ..Default::default()
    };
    library.playlists = vec![cached.clone()];
    library.fail_playlist = true;
    let fixture = Fixture::new(library);
    fixture.seed();
    fixture.catalog().save_playlist(&cached).unwrap();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::SaveSmartPlaylist {
        id: Some("smart".into()),
        name: "New name".into(),
        rules: rules(),
    });
    password(&worker);
    sign_in(&worker);
    event(
        &worker,
        |e| matches!(e, Event::SmartPlaylistSaved(id) if id == "smart"),
    );
    event(
        &worker,
        |e| matches!(e, Event::Error(s) if s == "Saved, but could not refresh playlist tracks"),
    );
    let saved = fixture.catalog().playlist("smart").unwrap().unwrap();
    assert_eq!(saved.name, "Cached");
    assert_eq!(saved.entry[0].id, "other-song");
    assert_eq!(fixture.native_requests().len(), 1);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}
