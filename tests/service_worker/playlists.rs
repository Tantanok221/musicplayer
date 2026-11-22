use crate::fixture::{Fixture, Library, Request, WAIT, event, synced};
use serde_json::json;
use sonora::{
    model::Playlist,
    service::{Command, Event, Mutation},
};
use std::sync::Arc;

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
