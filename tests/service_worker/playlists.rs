use crate::fixture::{
    ALBUM_ORDER, Fixture, Library, Request, WAIT, event, multi_disc_library, synced,
};
use serde_json::json;
use sonora::{
    catalog::Catalog,
    model::Playlist,
    service::{Command, Event, Mutation, PlaylistTarget},
};
use std::sync::Arc;

#[test]
fn album_details_load_from_the_catalog() {
    let mut library = Library::default();
    let album = &mut library.albums[0];
    album.year = Some(2024);
    album.genre = "Rock".into();
    let id = album.id.clone();
    let fixture = Fixture::new(library);
    fixture.seed();
    let mut worker = fixture.start_paused();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::AlbumDetails("missing-album".into()));
    worker.service().send(Command::AlbumDetails(id.clone()));
    let Event::AlbumDetails(album) =
        event(&worker, |event| matches!(event, Event::AlbumDetails(_)))
    else {
        unreachable!()
    };
    assert_eq!(album.id, id);
    assert_eq!(album.year, Some(2024));
    assert_eq!(album.genre, "Rock");
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn delayed_playlist_reply_after_account_switch_keeps_new_accounts_catalog() {
    delayed_playlist_reply_after_account_switch(false);
}

#[test]
fn delayed_post_mutation_playlists_reply_after_account_switch_keeps_new_accounts_catalog() {
    delayed_playlist_reply_after_account_switch(true);
}

fn delayed_playlist_reply_after_account_switch(post_mutation: bool) {
    let mut old_playlist = smart_playlist("2999-01-01T00:00:00Z");
    old_playlist.id = "a-playlist".into();
    old_playlist.entry = vec![Arc::new(crate::fixture::song("a-song", "album"))].into();
    let mut new_playlist = old_playlist.clone();
    new_playlist.id = "b-playlist".into();
    new_playlist.name = "Account B".into();
    let a = Fixture::new(Library {
        playlists: vec![old_playlist],
        ..Default::default()
    });
    let b = Fixture::new(Library {
        playlists: vec![new_playlist.clone()],
        ..Default::default()
    });
    let mut worker = a.start_paused();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let endpoint = if post_mutation {
        "getPlaylists.view"
    } else {
        "getPlaylist.view"
    };
    a.edit_library(|library| library.hold_endpoint = Some(endpoint));
    if post_mutation {
        a.expect_mutation(
            Request::new(
                "updatePlaylist.view",
                &[("playlistId", "a-playlist"), ("songIndexToRemove", "0")],
            ),
            true,
        );
        worker
            .service()
            .send(Command::Mutate(Mutation::RemoveFromPlaylist(
                "a-playlist".into(),
                0,
            )));
        event(&worker, |event| matches!(event, Event::MutationDone(_)));
    } else {
        worker
            .service()
            .send(Command::Playlist("a-playlist".into()));
    }
    a.wait_request(&Request::new(
        endpoint,
        if post_mutation {
            &[]
        } else {
            &[("id", "a-playlist")]
        },
    ));
    let catalog = Catalog::open(
        &a.paths
            .data
            .join(format!("{}.sqlite", b.account.namespace())),
    )
    .unwrap();
    catalog.save_playlist(&new_playlist).unwrap();
    worker.service().send(Command::Connect(
        b.account.clone(),
        "synthetic-password".into(),
    ));
    event(
        &worker,
        |event| matches!(event, Event::Ready(Some(account), _) if account.namespace() == b.account.namespace()),
    );
    synced(&worker);
    let before = serde_json::to_value(catalog.playlist_summaries().unwrap()).unwrap();
    a.resume_held();
    if post_mutation {
        a.wait_request(&Request::new("getPlaylist.view", &[("id", "a-playlist")]));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while worker.events().recv_deadline(deadline).is_ok() {}
    crate::fixture::view(&worker);
    assert_eq!(
        serde_json::to_value(catalog.playlist_summaries().unwrap()).unwrap(),
        before,
        "a delayed account A reply must not add playlists or delete account B playlists"
    );
    assert!(catalog.playlist("a-playlist").unwrap().is_none());
    assert_eq!(
        serde_json::to_value(catalog.playlist("b-playlist").unwrap().unwrap()).unwrap(),
        serde_json::to_value(new_playlist).unwrap()
    );
    worker.shutdown(WAIT).unwrap();
    a.finish();
    b.finish();
}

fn smart_playlist(until: &str) -> Playlist {
    serde_json::from_value(serde_json::json!({
        "id": "smart", "name": "Automatic", "owner": "fixture",
        "readonly": true, "validUntil": until, "songCount": 1
    }))
    .unwrap()
}

#[test]
fn sync_refreshes_only_expired_cached_smart_playlists() {
    let mut library = Library::default();
    let mut expired = smart_playlist("2000-01-01T00:00:00Z");
    expired.entry = vec![Arc::new(library.tracks[1].clone())].into();
    let mut fresh = smart_playlist("2999-01-01T00:00:00Z");
    fresh.id = "fresh".into();
    let mut uncached = expired.clone();
    uncached.id = "uncached".into();
    library.playlists = vec![expired.clone(), fresh.clone(), uncached];
    let fixture = Fixture::new(library);
    fixture.seed();
    expired.entry = vec![Arc::new(crate::fixture::song("cached", "album"))].into();
    fixture.catalog().save_playlist(&expired).unwrap();
    fixture.catalog().save_playlist(&fresh).unwrap();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    event(
        &worker,
        |event| matches!(event, Event::Playlist(p) if p.id == "smart" && p.entry[0].id == "other-song"),
    );
    assert_eq!(
        serde_json::to_value(fixture.catalog().playlist("smart").unwrap().unwrap()).unwrap()["validUntil"],
        "2000-01-01T00:00:00Z"
    );
    assert!(
        !fixture
            .requests()
            .iter()
            .any(|r| r.endpoint == "getPlaylist.view"
                && r.params.get("id").is_some_and(|id| id != "smart"))
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn expired_smart_refresh_failure_keeps_cache_then_empty_result_replaces_it() {
    let mut remote = smart_playlist("2000-01-01T00:00:00Z");
    remote.song_count = 0;
    let fixture = Fixture::new(Library {
        playlists: vec![remote.clone()],
        fail_playlist: true,
        ..Library::default()
    });
    fixture.seed();
    let mut cached = remote;
    cached.song_count = 1;
    cached.entry = vec![Arc::new(crate::fixture::song("cached", "album"))].into();
    fixture.catalog().save_playlist(&cached).unwrap();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    event(
        &worker,
        |event| matches!(event, Event::Error(error) if error.contains("Fixture playlist unavailable")),
    );
    assert_eq!(
        fixture.catalog().playlist("smart").unwrap().unwrap().entry[0].id,
        "cached"
    );
    fixture.edit_library(|library| library.fail_playlist = false);
    worker.service().send(Command::Refresh);
    synced(&worker);
    event(
        &worker,
        |event| matches!(event, Event::Playlist(p) if p.id == "smart" && p.entry.is_empty()),
    );
    assert!(
        fixture
            .catalog()
            .playlist("smart")
            .unwrap()
            .unwrap()
            .entry
            .is_empty()
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn incomplete_smart_refresh_preserves_cached_entries() {
    for playlist in [
        json!({"id":"smart","songCount":2,"entry":[{"id":"other-song"}]}),
        json!({"id":"smart","songCount":1}),
        json!({"id":"smart","songCount":1,"entry":[{}]}),
        json!({"id":"smart"}),
        json!({"id":"wrong","songCount":0}),
    ] {
        let mut cached = smart_playlist("2000-01-01T00:00:00Z");
        let summary = cached.clone();
        cached.entry = vec![Arc::new(crate::fixture::song("cached", "album"))].into();
        let fixture = Fixture::new(Library {
            playlists: vec![summary],
            playlist_response: Some(json!({"status":"ok","playlist":playlist})),
            ..Library::default()
        });
        fixture.seed();
        fixture.catalog().save_playlist(&cached).unwrap();
        let mut worker = fixture.start();
        worker.wait_ready(WAIT).unwrap();
        synced(&worker);
        let outcome = event(&worker, |event| match event {
            Event::Error(_) => true,
            Event::Playlist(list) => !list
                .entry
                .iter()
                .map(|track| track.id.as_str())
                .eq(["cached"]),
            _ => false,
        });
        let saved = fixture.catalog().playlist("smart").unwrap().unwrap();
        worker.shutdown(WAIT).unwrap();
        fixture.finish();
        assert!(
            saved
                .entry
                .iter()
                .map(|track| track.id.as_str())
                .eq(["cached"]),
            "partial getPlaylist {playlist} overwrote cached entries"
        );
        assert!(matches!(outcome, Event::Error(message) if message.contains("getPlaylist")));
    }
}

#[test]
fn opening_smart_playlist_replaces_cached_entries_and_failure_keeps_them() {
    let mut library = Library::default();
    let mut remote = smart_playlist("2999-01-01T00:00:00Z");
    remote.entry = vec![Arc::new(library.tracks[1].clone())].into();
    library.playlists = vec![remote.clone()];
    let fixture = Fixture::new(library);
    fixture.seed();
    let mut cached = remote;
    cached.entry = vec![Arc::new(crate::fixture::song("cached", "album"))].into();
    fixture.catalog().save_playlist(&cached).unwrap();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Playlist("smart".into()));
    event(
        &worker,
        |event| matches!(event, Event::Playlist(p) if p.entry[0].id == "cached"),
    );
    event(
        &worker,
        |event| matches!(event, Event::Playlist(p) if p.entry[0].id == "other-song"),
    );
    fixture.edit_library(|library| library.offline = true);
    worker.service().send(Command::Playlist("smart".into()));
    event(
        &worker,
        |event| matches!(event, Event::Playlist(p) if p.entry[0].id == "other-song"),
    );
    event(
        &worker,
        |event| matches!(event, Event::Error(error) if error.contains("Could not load playlist")),
    );
    assert_eq!(
        fixture.catalog().playlist("smart").unwrap().unwrap().entry[0].id,
        "other-song"
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn reorder_checks_server_snapshot_and_persists_duplicate_song_ids_in_order() {
    let mut library = Library::default();
    let first = Arc::new(library.tracks[0].clone());
    let original = ["song", "other-song", "song"];
    let reordered = ["other-song", "song", "song"];
    library.playlists = vec![Playlist {
        id: "playlist".into(),
        name: "Roadtrip".into(),
        comment: "Keep this description".into(),
        song_count: 3,
        entry: Arc::new(vec![
            first.clone(),
            Arc::new(library.tracks[1].clone()),
            first,
        ]),
        ..Default::default()
    }];
    let fixture = Fixture::new(library);
    fixture.seed();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Playlist("playlist".into()));
    event(
        &worker,
        |event| matches!(event, Event::Playlist(playlist) if playlist.entry.len() == 3),
    );

    let reorder = |old: [&str; 3]| Mutation::ReorderPlaylist {
        id: "playlist".into(),
        original: old.map(String::from).into(),
        reordered: reordered.map(String::from).into(),
    };
    let reject = |mutation: Mutation, reason: &str| {
        worker.service().send(Command::Mutate(mutation.clone()));
        event(
            &worker,
            |event| matches!(event, Event::MutationFailed(failed, error) if failed == &mutation && error.contains(reason)),
        );
        let saved = fixture.catalog().playlist("playlist").unwrap().unwrap();
        assert!(
            saved
                .entry
                .iter()
                .map(|track| track.id.as_str())
                .eq(original)
        );
    };
    reject(reorder(["song", "song", "other-song"]), "playlist changed");
    assert!(
        !fixture
            .requests()
            .iter()
            .any(|request| request.endpoint == "createPlaylist.view")
    );

    let request = Request::new(
        "createPlaylist.view",
        &[
            ("playlistId", "playlist"),
            ("songId", "other-song"),
            ("songId", "song"),
            ("songId", "song"),
        ],
    );
    fixture.expect_mutation(request.clone(), false);
    reject(reorder(original), "Fixture reorder rejected");
    fixture.expect_mutation(request, true);
    worker.service().send(Command::Mutate(reorder(original)));
    event(
        &worker,
        |event| matches!(event, Event::Playlist(playlist) if playlist.entry.iter().map(|track| track.id.as_str()).eq(reordered)),
    );
    let saved = fixture.catalog().playlist("playlist").unwrap().unwrap();
    assert!(
        saved
            .entry
            .iter()
            .map(|track| track.id.as_str())
            .eq(reordered)
    );
    assert_eq!(saved.comment, "Keep this description");
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn removal_uses_entry_index_and_refreshes_persisted_playlist_only_on_success() {
    let mut library = Library::default();
    let first = Arc::new(library.tracks[0].clone());
    library.playlists = vec![Playlist {
        id: "playlist".into(),
        name: "Roadtrip".into(),
        song_count: 3,
        entry: Arc::new(vec![
            first.clone(),
            Arc::new(library.tracks[1].clone()),
            first,
        ]),
        ..Default::default()
    }];
    let fixture = Fixture::new(library);
    fixture.seed();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Playlist("playlist".into()));
    event(
        &worker,
        |event| matches!(event, Event::Playlist(playlist) if playlist.entry.len() == 3),
    );
    let request = Request::new(
        "updatePlaylist.view",
        &[("playlistId", "playlist"), ("songIndexToRemove", "2")],
    );
    let mutation = Mutation::RemoveFromPlaylist("playlist".into(), 2);
    fixture.expect_mutation(request, false);
    worker.service().send(Command::Mutate(mutation.clone()));
    event(
        &worker,
        |event| matches!(event, Event::MutationFailed(failed, _) if failed == &mutation),
    );
    assert_eq!(
        fixture
            .catalog()
            .playlist("playlist")
            .unwrap()
            .unwrap()
            .entry
            .len(),
        3
    );
    for index in (0..3).rev() {
        let value = index.to_string();
        fixture.expect_mutation(
            Request::new(
                "updatePlaylist.view",
                &[("playlistId", "playlist"), ("songIndexToRemove", &value)],
            ),
            true,
        );
        worker
            .service()
            .send(Command::Mutate(Mutation::RemoveFromPlaylist(
                "playlist".into(),
                index,
            )));
        event(
            &worker,
            |event| matches!(event, Event::Playlist(playlist) if playlist.entry.len() == index),
        );
        let saved = fixture.catalog().playlist("playlist").unwrap().unwrap();
        assert_eq!(saved.entry.len(), index);
        assert_eq!(saved.song_count as usize, index);
        if index == 2 {
            assert_eq!(
                saved
                    .entry
                    .iter()
                    .map(|track| track.id.as_str())
                    .collect::<Vec<_>>(),
                ["song", "other-song"]
            );
        }
    }
    worker.shutdown(WAIT).unwrap();
    assert!(
        fixture
            .catalog()
            .playlist("playlist")
            .unwrap()
            .unwrap()
            .entry
            .is_empty()
    );
    fixture.finish();
}

fn album_ids() -> Vec<String> {
    ALBUM_ORDER.map(String::from).to_vec()
}

fn album_songs(endpoint: &str, key: &str, base: &[(&str, &str)]) -> Request {
    let mut params = base.to_vec();
    params.extend(ALBUM_ORDER.map(|id| (key, id)));
    Request::new(endpoint, &params)
}

#[test]
fn album_to_new_playlist_creates_it_with_songs_in_album_order() {
    let fixture = Fixture::new(multi_disc_library());
    let mut worker = fixture.start();
    synced(&worker);
    fixture.expect_mutation(
        album_songs("createPlaylist.view", "songId", &[("name", "Road trip")]),
        true,
    );
    worker.service().send(Command::AlbumToPlaylist(
        "album".into(),
        PlaylistTarget::New("Road trip".into()),
    ));
    let created = Mutation::CreatePlaylist("Road trip".into(), album_ids());
    event(
        &worker,
        |e| matches!(e, Event::MutationDone(m) if *m == created),
    );
    let Event::Playlists(playlists) = event(&worker, |e| matches!(e, Event::Playlists(_))) else {
        unreachable!()
    };
    assert!(
        playlists
            .iter()
            .any(|p| p.name == "Road trip" && p.song_count == 3)
    );
    assert!(
        fixture
            .catalog()
            .playlist_summaries()
            .unwrap()
            .iter()
            .any(|p| p.name == "Road trip")
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn album_to_existing_playlist_appends_in_album_order_and_reports_failure() {
    let mix = Playlist {
        id: "mix".into(),
        name: "Mix".into(),
        owner: "fixture".into(),
        song_count: 1,
        entry: vec![Arc::new(crate::fixture::song("other-song", "other-album"))].into(),
        ..Default::default()
    };
    let fixture = Fixture::new(Library {
        playlists: vec![mix],
        ..multi_disc_library()
    });
    let mut worker = fixture.start();
    synced(&worker);
    let add = album_songs(
        "updatePlaylist.view",
        "songIdToAdd",
        &[("playlistId", "mix")],
    );
    fixture.expect_mutation(add.clone(), true);
    worker.service().send(Command::AlbumToPlaylist(
        "album".into(),
        PlaylistTarget::Existing("mix".into()),
    ));
    let added = Mutation::AddToPlaylist("mix".into(), album_ids());
    event(
        &worker,
        |e| matches!(e, Event::MutationDone(m) if *m == added),
    );
    let Event::Playlist(refreshed) = event(
        &worker,
        |e| matches!(e, Event::Playlist(p) if p.id == "mix"),
    ) else {
        unreachable!()
    };
    let ids = |playlist: &Playlist| {
        playlist
            .entry
            .iter()
            .map(|t| t.id.clone())
            .collect::<Vec<_>>()
    };
    let expected = ["other-song", "opener", "song", "disc-two"].map(String::from);
    assert_eq!(ids(&refreshed), expected);
    assert_eq!(
        ids(&fixture.catalog().playlist("mix").unwrap().unwrap()),
        expected
    );

    fixture.expect_mutation(add, false);
    worker.service().send(Command::AlbumToPlaylist(
        "album".into(),
        PlaylistTarget::Existing("mix".into()),
    ));
    let Event::MutationFailed(failed, error) =
        event(&worker, |e| matches!(e, Event::MutationFailed(..)))
    else {
        unreachable!()
    };
    assert_eq!(failed, added);
    assert!(error.contains("Retry") && !error.contains("synthetic-password"));
    assert_eq!(
        ids(&fixture.catalog().playlist("mix").unwrap().unwrap()),
        expected
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn album_without_songs_in_the_library_is_refused_before_any_request() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    synced(&worker);
    worker.service().send(Command::AlbumToPlaylist(
        "missing-album".into(),
        PlaylistTarget::New("Empty".into()),
    ));
    event(
        &worker,
        |e| matches!(e, Event::Error(message) if message.contains("no songs in the library")),
    );
    crate::fixture::view(&worker);
    assert!(!fixture.requests().iter().any(|r| matches!(
        r.endpoint.as_str(),
        "createPlaylist.view" | "updatePlaylist.view"
    )));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}
