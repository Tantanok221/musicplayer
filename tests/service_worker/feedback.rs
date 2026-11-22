use crate::fixture::{Fixture, Library, Request, WAIT, event, mutate, synced};
use sonora::{
    model::{FavoritesFilter, LibraryQuery, Sort},
    service::{Command, Event, Mutation},
    sorting::{Scope, SortSpec},
};
use std::time::Duration;

#[test]
fn latest_track_rating_wins_when_the_first_write_is_held() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let request = |id, rating| Request::new("setRating.view", &[("id", id), ("rating", rating)]);
    let earlier = request("song", "2");
    let latest = request("song", "5");
    let release = fixture.hold_rating(earlier.clone(), true);
    worker
        .service()
        .send(Command::Mutate(Mutation::Rating("song".into(), 2)));
    fixture.wait_request(&earlier);

    // A held write on this song must not block feedback on a different song.
    fixture.expect_mutation(request("other-song", "4"), true);
    worker
        .service()
        .send(Command::Mutate(Mutation::Rating("other-song".into(), 4)));
    event(
        &worker,
        |event| matches!(event, Event::MutationDone(Mutation::Rating(id, 4)) if id == "other-song"),
    );

    fixture.expect_mutation(latest.clone(), true);
    worker
        .service()
        .send(Command::Mutate(Mutation::Rating("song".into(), 5)));
    let mut published = Vec::new();
    let mut record_rating = || {
        let Event::TrackUpdated(track) = event(
            &worker,
            |event| matches!(event, Event::TrackUpdated(track) if track.id == "song"),
        ) else {
            unreachable!()
        };
        published.push(track.user_rating.unwrap());
    };
    // If the requests race, let five stars finish before the server applies two.
    let overtook = fixture.request_arrives_within(&latest, Duration::from_secs(1));
    if overtook {
        record_rating();
    }
    release.send(()).unwrap();
    record_rating();
    if !overtook {
        record_rating();
    }
    let server = fixture.server_track_rating("song");
    let catalog = fixture.catalog().by_ids(&["song".into()]).unwrap()[0].user_rating;
    eprintln!("track ratings published={published:?}, server={server:?}, catalog={catalog:?}");
    assert_eq!(
        (published.last(), server, catalog),
        (Some(&5), Some(5), Some(5))
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn rejected_track_rating_does_not_block_or_offer_retry_over_a_newer_rating() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let earlier = Request::new("setRating.view", &[("id", "song"), ("rating", "2")]);
    let release = fixture.hold_rating(earlier.clone(), false);
    worker
        .service()
        .send(Command::Mutate(Mutation::Rating("song".into(), 2)));
    fixture.wait_request(&earlier);
    fixture.expect_mutation(
        Request::new("setRating.view", &[("id", "song"), ("rating", "5")]),
        true,
    );
    worker
        .service()
        .send(Command::Mutate(Mutation::Rating("song".into(), 5)));
    let epoch = worker.service().query(LibraryQuery::default());
    event(
        &worker,
        |event| matches!(event, Event::View(request, _) if *request == epoch),
    );

    release.send(()).unwrap();
    let result = event(&worker, |event| {
        matches!(event, Event::MutationDone(Mutation::Rating(id, 5))
            | Event::MutationFailed(Mutation::Rating(id, 2), _) if id == "song")
    });
    assert!(
        matches!(result, Event::MutationDone(Mutation::Rating(_, 5))),
        "an already superseded rating must not become retryable"
    );
    assert_eq!(fixture.server_track_rating("song"), Some(5));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn quitting_with_a_held_rating_keeps_the_latest_selection_for_reconnect() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let earlier = Request::new("setRating.view", &[("id", "song"), ("rating", "2")]);
    let release = fixture.hold_rating(earlier.clone(), false);
    worker
        .service()
        .send(Command::Mutate(Mutation::Rating("song".into(), 2)));
    fixture.wait_request(&earlier);
    let latest = Request::new("setRating.view", &[("id", "song"), ("rating", "5")]);
    fixture.expect_mutation(latest.clone(), true);
    worker
        .service()
        .send(Command::Mutate(Mutation::Rating("song".into(), 5)));
    worker.shutdown(WAIT).unwrap();
    assert!(!fixture.requests().contains(&latest));
    drop(release);

    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    fixture.wait_request(&latest);
    event(
        &worker,
        |event| matches!(event, Event::MutationDone(Mutation::Rating(id, 5)) if id == "song"),
    );
    assert_eq!(fixture.server_track_rating("song"), Some(5));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

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
fn rating_across_favorites_threshold_refreshes_view_sorted_by_another_field() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let rated_four = FavoritesFilter {
        hearted: false,
        min_rating: 4,
        match_all: false,
    };
    let favorites = |scope| LibraryQuery {
        favorites: true,
        favorite_filter: rated_four.clone(),
        ordering: Some(SortSpec::legacy(Sort::Album, false, scope)),
        ..Default::default()
    };
    // The rating change must publish a fresh view, not only the patched row.
    let rated = |mutation: Mutation, rating: &str| {
        let (Mutation::Rating(id, _) | Mutation::AlbumRating(id, _)) = &mutation else {
            unreachable!()
        };
        fixture.expect_mutation(
            Request::new("setRating.view", &[("id", id), ("rating", rating)]),
            true,
        );
        worker.service().send(Command::Mutate(mutation.clone()));
        event(
            &worker,
            |event| matches!(event, Event::MutationDone(done) if *done == mutation),
        );
        match worker.events().recv_timeout(WAIT) {
            Ok(Event::View(_, view)) => view,
            _ => panic!("no fresh view after {mutation:?}"),
        }
    };

    let epoch = worker.service().query(favorites(Scope::Tracks));
    let Event::View(_, view) = event(
        &worker,
        |event| matches!(event, Event::View(request, _) if *request == epoch),
    ) else {
        unreachable!()
    };
    assert_eq!(view.track_count, 0);
    let view = rated(Mutation::Rating("song".into(), 5), "5");
    assert_eq!(view.track_count, 1);
    assert_eq!(view.tracks[0].id, "song");
    let view = rated(Mutation::Rating("song".into(), 2), "2");
    assert_eq!(view.track_count, 0);

    let epoch = worker.service().query(favorites(Scope::Albums));
    let Event::View(_, view) = event(
        &worker,
        |event| matches!(event, Event::View(request, _) if *request == epoch),
    ) else {
        unreachable!()
    };
    assert_eq!((view.albums.len(), view.track_count), (0, 0));
    let view = rated(Mutation::AlbumRating("album".into(), 4), "4");
    assert_eq!((view.albums.len(), view.track_count), (1, 1));
    assert_eq!(view.albums[0].id, "album");
    let view = rated(Mutation::AlbumRating("album".into(), 1), "1");
    assert_eq!((view.albums.len(), view.track_count), (0, 0));

    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}
