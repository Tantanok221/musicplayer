use crate::fixture::{Fixture, Library, Request, WAIT, event, mutate, synced};
use sonora::service::{Command, Event, Mutation};

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
