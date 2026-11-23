use crate::fixture::{Fixture, Library, Request, WAIT, event, synced};
use sonora::{
    catalog::Catalog,
    model::{Preferences, SavedQueue},
    playback::PlayerCommand,
    service::{Command, Event, test_support::Worker},
};

#[test]
fn delayed_lyrics_reply_cannot_reach_the_new_account() {
    let a = Fixture::new(Library {
        structured_lyrics: serde_json::json!([{"line":[{"value":"Account A lyrics"}]}]),
        ..Default::default()
    });
    let b = Fixture::new(Library::default());
    let mut worker = a.start_paused();
    synced(&worker);
    a.edit_library(|l| l.hold_endpoint = Some("getLyricsBySongId.view"));
    worker.service().send(Command::Lyrics(
        a.catalog().by_ids(&["song".into()]).unwrap()[0].clone(),
    ));
    a.wait_request(&Request::new("getLyricsBySongId.view", &[("id", "song")]));

    let catalog = switch_account(&worker, &a, &b);
    a.resume_held();
    assert_no_late_events(&worker);
    assert!(
        catalog
            .state::<Option<sonora::lyrics::Lyrics>>("lyrics:v2:song")
            .unwrap()
            .is_none()
    );

    worker.shutdown(WAIT).unwrap();
    a.finish();
    b.finish();
}

#[test]
fn delayed_artist_info_reply_cannot_reach_the_new_account() {
    let a = Fixture::new(Library::default());
    let b = Fixture::new(Library::default());
    let mut worker = a.start_paused();
    synced(&worker);
    a.edit_library(|l| l.hold_endpoint = Some("getArtistInfo2.view"));
    worker.service().send(Command::ArtistProfile(
        "Fixture Artist".into(),
        "artist".into(),
        true,
    ));
    a.wait_request(&Request::new(
        "getArtistInfo2.view",
        &[
            ("id", "artist"),
            ("count", "12"),
            ("includeNotPresent", "true"),
        ],
    ));

    let catalog = switch_account(&worker, &a, &b);
    a.resume_held();
    assert_no_late_events(&worker);
    assert!(
        catalog
            .artist_profile(&sonora::artists::cache_key("Fixture Artist", "artist"))
            .unwrap()
            .is_none()
    );

    worker.shutdown(WAIT).unwrap();
    a.finish();
    b.finish();
}

#[test]
fn delayed_download_rejection_cannot_reach_the_new_account() {
    let a = Fixture::new(Library::default());
    let b = Fixture::new(Library::default());
    let mut worker = a.start_paused();
    synced(&worker);
    let request = start_held_download(&worker, &a);
    a.wait_request(&request);

    let catalog = switch_account(&worker, &a, &b);
    a.resume_held();
    assert_no_late_events(&worker);
    assert!(
        catalog
            .state::<Vec<sonora::slskd::DownloadJob>>("downloads:v1")
            .unwrap()
            .is_empty()
    );

    worker.shutdown(WAIT).unwrap();
    a.finish();
    b.finish();
}

#[test]
fn download_rejection_still_reaches_its_own_account() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start_paused();
    synced(&worker);
    let request = start_held_download(&worker, &fixture);
    fixture.wait_request(&request);

    fixture.resume_held();
    event(
        &worker,
        |e| matches!(e, Event::DownloadNotStarted(id, message) if id == "a-release" && message.contains("did not accept")),
    );
    assert!(
        fixture
            .catalog()
            .state::<Vec<sonora::slskd::DownloadJob>>("downloads:v1")
            .unwrap()
            .is_empty()
    );

    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn delayed_scrobble_reply_cannot_reach_the_new_account() {
    let a = Fixture::new(Library::default());
    let b = Fixture::new(Library::default());
    let mut worker = a.start_paused();
    synced(&worker);
    a.edit_library(|l| l.hold_scrobble = true);
    let request = Request::new(
        "scrobble.view",
        &[
            ("id", "song"),
            ("submission", "true"),
            ("time", "1700000000001"),
        ],
    );
    a.expect_mutation(request.clone(), true);
    worker
        .service()
        .send(Command::Scrobble(sonora::listening::ListenReport {
            id: 1,
            account: a.account.namespace(),
            track_id: "song".into(),
            started_ms: 1700000000001,
            submission: true,
        }));
    a.wait_request(&request);

    let catalog = switch_account(&worker, &a, &b);
    let before = serde_json::to_value(catalog.by_ids(&["song".into()]).unwrap()).unwrap();
    a.resume_scrobble();
    a.wait_request(&Request::new("getSong.view", &[("id", "song")]));
    assert_no_late_events(&worker);
    assert_eq!(
        serde_json::to_value(catalog.by_ids(&["song".into()]).unwrap()).unwrap(),
        before
    );
    let submissions: i64 = rusqlite::Connection::open(
        a.paths
            .data
            .join(format!("{}.sqlite", b.account.namespace())),
    )
    .unwrap()
    .query_row("SELECT count(*) FROM listen_submissions", [], |r| r.get(0))
    .unwrap();
    assert_eq!(submissions, 0);

    worker.shutdown(WAIT).unwrap();
    a.finish();
    b.finish();
}

fn start_held_download(worker: &Worker, fixture: &Fixture) -> Request {
    fixture.edit_library(|l| l.hold_endpoint = Some("slskd/enqueue"));
    worker.service().send(Command::Preferences(Preferences {
        downloads: sonora::model::DownloadPreferences {
            enabled: true,
            url: fixture.account.url.clone(),
        },
        ..Default::default()
    }));
    let files = vec![sonora::slskd::RemoteFile {
        filename: "Fixture Album\\song.flac".into(),
        size: 100,
        ..Default::default()
    }];
    let candidate = sonora::slskd::candidates(
        &[sonora::slskd::SearchResponse {
            username: "fixture-peer".into(),
            files,
            ..Default::default()
        }],
        Some(1),
    )
    .remove(0);
    worker.service().send(Command::Download(Box::new(
        sonora::service::DownloadRequest {
            artist: "Fixture Artist".into(),
            artist_id: "artist".into(),
            release: "a-release".into(),
            title: "Account A".into(),
            expected: Some(1),
            candidate,
        },
    )));
    Request::new("slskd/enqueue", &[])
}

fn switch_account(worker: &Worker, a: &Fixture, b: &Fixture) -> Catalog {
    worker.service().send(Command::Connect(
        b.account.clone(),
        "synthetic-password".into(),
    ));
    event(
        worker,
        |e| matches!(e, Event::Ready(Some(account), _) if account.namespace() == b.account.namespace()),
    );
    synced(worker);
    Catalog::open(
        &a.paths
            .data
            .join(format!("{}.sqlite", b.account.namespace())),
    )
    .unwrap()
}

fn assert_no_late_events(worker: &Worker) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while let Ok(reply) = worker.events().recv_deadline(deadline) {
        assert!(
            !matches!(
                reply,
                Event::Lyrics(..)
                    | Event::ArtistProfile(..)
                    | Event::DownloadNotStarted(..)
                    | Event::TrackUpdated(..)
                    | Event::Error(..)
            ),
            "late background reply reached account B"
        );
        if let Event::Downloads(jobs) = reply {
            assert!(jobs.iter().all(|j| j.release != "a-release"));
        }
    }
    crate::fixture::view(worker);
}

#[test]
fn no_account_requests_sign_in_then_connect_uses_an_account_catalog() {
    let fixture = Fixture::new(Library::default());
    std::fs::remove_file(fixture.paths.data.join("account.json")).unwrap();
    let mut worker = fixture.start();
    assert!(worker.wait_ready(WAIT).unwrap().0.is_none());
    event(&worker, |e| matches!(e, Event::SignInRequired(_)));
    worker.service().send(Command::Connect(
        fixture.account.clone(),
        "synthetic-password".into(),
    ));
    event(&worker, |e| matches!(e, Event::Connected(..)));
    synced(&worker);
    assert_eq!(fixture.catalog().count().unwrap(), 2);
    assert_eq!(
        Catalog::open(&fixture.paths.data.join("unconnected.sqlite"))
            .unwrap()
            .count()
            .unwrap(),
        0
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn account_switch_saves_each_queue_and_preferences_and_refuses_during_sync() {
    let a = Fixture::new(Library::default());
    let b = Fixture::new(Library {
        tracks: vec![crate::fixture::song("b-song", "b-album")],
        albums: vec![crate::fixture::album("b-album")],
        ..Default::default()
    });
    let mut worker = a.start_paused();
    synced(&worker);
    let prefs = Preferences {
        volume: 0.23,
        ..Default::default()
    };
    worker.service().send(Command::Preferences(prefs));
    worker
        .service()
        .send(Command::Player(PlayerCommand::Restore(
            a.catalog().by_ids(&["song".into(), "song".into()]).unwrap(),
            1,
            23.,
            None,
        )));
    event(
        &worker,
        |e| matches!(e, Event::Playback(s) if s.position == 23.),
    );
    worker.service().send(Command::Connect(
        b.account.clone(),
        "synthetic-password".into(),
    ));
    event(
        &worker,
        |e| matches!(e, Event::Ready(Some(account), _) if account.namespace() == b.account.namespace()),
    );
    synced(&worker);
    let b_path = a
        .paths
        .data
        .join(format!("{}.sqlite", b.account.namespace()));
    let b_catalog = Catalog::open(&b_path).unwrap();
    assert_eq!(b_catalog.count().unwrap(), 1);
    worker.service().send(Command::Preferences(Preferences {
        volume: 0.71,
        ..Default::default()
    }));
    worker
        .service()
        .send(Command::Player(PlayerCommand::Restore(
            b_catalog.by_ids(&["b-song".into()]).unwrap(),
            0,
            9.,
            None,
        )));
    event(
        &worker,
        |e| matches!(e, Event::Playback(s) if s.position == 9.),
    );
    worker
        .service()
        .send(Command::Connect(a.account.clone(), "".into()));
    let Event::Ready(_, prefs) = event(&worker, |e| matches!(e, Event::Ready(..))) else {
        unreachable!()
    };
    assert_eq!(prefs.volume, 0.23);
    synced(&worker);
    worker
        .service()
        .send(Command::Player(PlayerCommand::ClearError));
    let Event::Playback(restored) = event(
        &worker,
        |e| matches!(e, Event::Playback(s) if s.queue.len() == 2),
    ) else {
        unreachable!()
    };
    assert_eq!(restored.index, Some(1));
    assert_eq!(restored.position, 23.);
    a.edit_library(|l| l.hold_sync = true);
    worker.service().send(Command::Refresh);
    a.wait_request(&Request::new("getScanStatus.view", &[]));
    worker
        .service()
        .send(Command::Connect(b.account.clone(), "".into()));
    event(
        &worker,
        |e| matches!(e, Event::SignInFailed(s) if s.contains("Wait for the current refresh")),
    );
    assert_eq!(
        a.paths.account().unwrap().unwrap().namespace(),
        a.account.namespace()
    );
    assert_eq!(a.catalog().count().unwrap(), 2);
    assert_eq!(b_catalog.count().unwrap(), 1);
    a.resume_sync();
    synced(&worker);
    worker.shutdown(WAIT).unwrap();
    assert_eq!(
        b_catalog
            .state::<Preferences>("preferences")
            .unwrap()
            .volume,
        0.71
    );
    assert_eq!(
        b_catalog.state::<SavedQueue>("queue").unwrap().ids,
        ["b-song"]
    );
    a.finish();
    b.finish();
}

#[test]
fn delayed_player_save_from_previous_account_cannot_replace_the_new_accounts_queue() {
    let a = Fixture::new(Library::default());
    let b = Fixture::new(Library::default());
    let mut worker = a.start_paused();
    synced(&worker);
    worker.service().send(Command::Connect(
        b.account.clone(),
        "synthetic-password".into(),
    ));
    event(
        &worker,
        |e| matches!(e, Event::Ready(Some(account), _) if account.namespace() == b.account.namespace()),
    );
    synced(&worker);
    let catalog = Catalog::open(
        &a.paths
            .data
            .join(format!("{}.sqlite", b.account.namespace())),
    )
    .unwrap();
    // Model the old player's already queued persistence message arriving after
    // Connected. The owning worker must reject this stale account's save.
    worker.service().send(Command::Persist(
        a.account.namespace(),
        SavedQueue {
            ids: vec!["song".into(), "song".into()],
            shuffle_positions: None,
            index: Some(1),
            position: 23.,
            listen: None,
        },
    ));
    crate::fixture::view(&worker);
    assert!(
        catalog.state::<SavedQueue>("queue").unwrap().ids.is_empty(),
        "late queue saves must remain scoped to their source account"
    );
    worker.shutdown(WAIT).unwrap();
    a.finish();
    b.finish();
}
