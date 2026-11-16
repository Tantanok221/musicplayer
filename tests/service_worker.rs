//! Real actor → loopback HTTP → SQLite → production events.
#[path = "service_worker/fixture.rs"]
mod fixture;

use fixture::{Fixture, Library, Request, WAIT, album, event, mutate, song, synced, view};
use sonora::{
    model::{GainMode, Playlist, Preferences, RepeatMode},
    playback::PlayerCommand,
    service::{Command, Event, Mutation},
};
use std::time::Instant;

#[test]
fn album_feedback_reaches_http_catalog_and_real_events_without_editing_tracks() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let catalog = fixture.catalog();
    let original_song =
        serde_json::to_value(&*catalog.by_ids(&["song".into()]).unwrap()[0]).unwrap();
    let original_other =
        serde_json::to_value(&*catalog.album("other-album").unwrap().unwrap()).unwrap();
    for (mutation, request, starred, rating) in [
        (
            Mutation::AlbumFavorite("album".into(), true),
            Request::new("star.view", &[("albumId", "album")]),
            true,
            2,
        ),
        (
            Mutation::AlbumFavorite("album".into(), false),
            Request::new("unstar.view", &[("albumId", "album")]),
            false,
            2,
        ),
        (
            Mutation::AlbumRating("album".into(), 5),
            Request::new("setRating.view", &[("id", "album"), ("rating", "5")]),
            false,
            5,
        ),
        (
            Mutation::AlbumRating("album".into(), 0),
            Request::new("setRating.view", &[("id", "album"), ("rating", "0")]),
            false,
            0,
        ),
    ] {
        fixture.expect_mutation(request, true);
        let updated = mutate(&worker, mutation, true);
        assert_eq!(updated.id, "album");
        assert_eq!(updated.starred.is_some(), starred);
        assert_eq!(updated.user_rating, Some(rating));
        let saved = catalog.album("album").unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&*saved).unwrap(),
            serde_json::to_value(&*updated).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&*catalog.by_ids(&["song".into()]).unwrap()[0]).unwrap(),
            original_song
        );
        assert_eq!(
            serde_json::to_value(&*catalog.album("other-album").unwrap().unwrap()).unwrap(),
            original_other
        );
    }
    // Track feedback uses id, not albumId, and never edits the parent album.
    fixture.expect_mutation(Request::new("star.view", &[("id", "song")]), true);
    worker
        .service()
        .send(Command::Mutate(Mutation::Favorite("song".into(), true)));
    let Event::TrackUpdated(track) =
        event(&worker, |event| matches!(event, Event::TrackUpdated(_)))
    else {
        unreachable!()
    };
    assert!(track.starred.is_some());
    event(
        &worker,
        |event| matches!(event, Event::MutationDone(Mutation::Favorite(id, true)) if id == "song"),
    );
    fixture.expect_mutation(
        Request::new("setRating.view", &[("id", "song"), ("rating", "3")]),
        true,
    );
    worker
        .service()
        .send(Command::Mutate(Mutation::Rating("song".into(), 3)));
    let Event::TrackUpdated(track) =
        event(&worker, |event| matches!(event, Event::TrackUpdated(_)))
    else {
        unreachable!()
    };
    assert_eq!(track.user_rating, Some(3));
    assert!(track.starred.is_some());
    event(
        &worker,
        |event| matches!(event, Event::MutationDone(Mutation::Rating(id, 3)) if id == "song"),
    );
    assert_eq!(
        catalog.by_ids(&["song".into()]).unwrap()[0].user_rating,
        Some(3)
    );
    assert!(catalog.album("album").unwrap().unwrap().starred.is_none());
    assert_eq!(
        catalog.album("album").unwrap().unwrap().user_rating,
        Some(0)
    );
    worker.shutdown(WAIT).unwrap();
    assert_eq!(
        fixture
            .catalog()
            .album("album")
            .unwrap()
            .unwrap()
            .user_rating,
        Some(0)
    );
    fixture.finish();
}

#[test]
fn failed_album_feedback_publishes_rollback_before_failure_and_retry_repeats_http_intent() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    for (mutation, request) in [
        (
            Mutation::AlbumFavorite("album".into(), true),
            Request::new("star.view", &[("albumId", "album")]),
        ),
        (
            Mutation::AlbumRating("album".into(), 4),
            Request::new("setRating.view", &[("id", "album"), ("rating", "4")]),
        ),
    ] {
        let before =
            serde_json::to_value(&*fixture.catalog().album("album").unwrap().unwrap()).unwrap();
        fixture.expect_mutation(request.clone(), false);
        let rollback = mutate(&worker, mutation.clone(), false);
        assert_eq!(serde_json::to_value(&*rollback).unwrap(), before);
        assert_eq!(
            serde_json::to_value(&*fixture.catalog().album("album").unwrap().unwrap()).unwrap(),
            before
        );
        // Retry is the same command the UI's Retry handler emits (covered in batch 1).
        fixture.expect_mutation(request.clone(), true);
        let updated = mutate(&worker, mutation, true);
        assert_ne!(serde_json::to_value(&*updated).unwrap(), before);
        assert_eq!(
            fixture
                .requests()
                .iter()
                .filter(|actual| **actual == request)
                .count(),
            2
        );
    }
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn interrupted_sync_keeps_cached_music_then_a_complete_refresh_reconciles() {
    let fixture = Fixture::new(Library {
        tracks: (0..501)
            .map(|index| song(&format!("fresh-{index}"), "album"))
            .collect(),
        albums: vec![album("album")],
        fail_song_offset: Some(500),
        ..Library::default()
    });
    {
        let mut catalog = fixture.catalog();
        catalog
            .put_tracks(&[song("cached", "cached-album")], 1)
            .unwrap();
        catalog.put_albums(&[album("cached-album")], 1).unwrap();
        catalog
            .save_playlist_summaries(
                &[Playlist {
                    id: "cached-playlist".into(),
                    name: "Offline playlist".into(),
                    ..Default::default()
                }],
                true,
            )
            .unwrap();
    }
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    event(
        &worker,
        |event| matches!(event, Event::Error(message) if message.contains("Fixture later page failed") && message.contains("Cached music is still available")),
    );
    let partial = view(&worker);
    assert_eq!(
        partial.track_count, 501,
        "500 imported songs plus the cached song"
    );
    let catalog = fixture.catalog();
    assert_eq!(
        catalog
            .by_ids(&["fresh-0".into(), "fresh-499".into()])
            .unwrap()
            .len(),
        2
    );
    assert_eq!(catalog.by_ids(&["cached".into()]).unwrap().len(), 1);
    assert!(catalog.album("cached-album").unwrap().is_some());
    assert!(catalog.playlist("cached-playlist").unwrap().is_some());

    // A successful pass during a server scan still must not remove cached data.
    fixture.edit_library(|library| {
        library.fail_song_offset = None;
        library.scanning = true;
    });
    worker.service().send(Command::Refresh);
    event(
        &worker,
        |event| matches!(event, Event::Status(message) if message.starts_with("Server scan in progress")),
    );
    assert_eq!(view(&worker).track_count, 502);
    assert!(catalog.album("cached-album").unwrap().is_some());
    assert!(catalog.playlist("cached-playlist").unwrap().is_some());

    fixture.edit_library(|library| library.scanning = false);
    worker.service().send(Command::Refresh);
    synced(&worker);
    let complete = view(&worker);
    assert_eq!(complete.track_count, 501);
    assert_eq!(complete.album_count, 1);
    assert!(catalog.by_ids(&["cached".into()]).unwrap().is_empty());
    assert!(catalog.album("cached-album").unwrap().is_none());
    assert!(catalog.playlist("cached-playlist").unwrap().is_none());
    assert_eq!(catalog.by_ids(&["fresh-500".into()]).unwrap().len(), 1);
    worker.shutdown(WAIT).unwrap();
    assert_eq!(fixture.catalog().count().unwrap(), 501);
    fixture.finish();
}

#[test]
fn preferences_save_through_the_worker_and_restore_after_clean_restart() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    let (_, initial) = worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let preferences = Preferences {
        volume: 0.23,
        gain: GainMode::Album,
        shuffle: true,
        repeat: RepeatMode::All,
        dark: Some(true),
        all_missing_releases: true,
        ..initial
    };
    let expected = serde_json::to_value(&preferences).unwrap();
    worker.service().send(Command::Preferences(preferences));
    let deadline = Instant::now() + WAIT;
    loop {
        let command = worker.player_commands().recv_deadline(deadline).unwrap();
        if let PlayerCommand::Preferences(actual) = command
            && serde_json::to_value(actual).unwrap() == expected
        {
            break;
        }
    }
    assert_eq!(
        serde_json::to_value(
            fixture
                .catalog()
                .state::<Preferences>("preferences")
                .unwrap()
        )
        .unwrap(),
        expected
    );
    worker.shutdown(WAIT).unwrap();
    drop(worker);
    let mut restarted = fixture.start();
    let (account, restored) = restarted.wait_ready(WAIT).unwrap();
    assert_eq!(account.unwrap().namespace(), fixture.account.namespace());
    assert_eq!(serde_json::to_value(restored).unwrap(), expected);
    let PlayerCommand::Preferences(restored) =
        restarted.player_commands().recv_timeout(WAIT).unwrap()
    else {
        panic!("preferences must reach the player first")
    };
    assert_eq!(serde_json::to_value(restored).unwrap(), expected);
    synced(&restarted);
    restarted.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn worker_metadata_uses_injected_musicbrainz_and_listenbrainz_endpoints() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Discography(
        "Fixture Artist".into(),
        "artist".into(),
    ));
    let Event::Discography(_, result) = event(
        &worker,
        |event| matches!(event, Event::Discography(_, result) if !result.loading),
    ) else {
        unreachable!()
    };
    assert_eq!(result.missing.len(), 1);
    assert_eq!(result.missing[0].id, "missing-release");
    fixture.wait_request(&Request::new("metadata/release", &[]));
    fixture.wait_request(&Request::new("listens/mb-artist", &[]));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn shutdown_finishes_the_worker_and_runtime_with_metadata_still_pending() {
    let fixture = Fixture::new(Library {
        hold_metadata: true,
        ..Library::default()
    });
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Discography(
        "Fixture Artist".into(),
        "artist".into(),
    ));
    fixture.wait_request(&Request::new("metadata/release", &[]));
    worker.shutdown(WAIT).unwrap();
    assert!(
        worker.events().is_disconnected(),
        "no timer/request may retain the worker's event sender"
    );
    fixture.finish();
}
