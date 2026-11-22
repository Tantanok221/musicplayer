use crate::fixture::{Fixture, Library, WAIT, event, synced};
use sonora::{
    listening::ListenSession,
    model::{PlaybackSnapshot, Preferences, SavedQueue},
    playback::PlayerCommand,
    service::{Command, Event, test_support::Worker},
};

fn snapshot(worker: &Worker, ids: &[&str]) -> PlaybackSnapshot {
    let Event::Playback(snapshot) = event(
        worker,
        |event| matches!(event, Event::Playback(s) if s.queue.iter().map(|t| t.id.as_str()).collect::<Vec<_>>() == ids),
    ) else {
        unreachable!()
    };
    snapshot
}

#[test]
fn disabling_shuffle_after_restart_restores_order_and_current_duplicate() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let original = ["other-song", "song", "song"];
    let mut worker = fixture.start_paused();
    synced(&worker);
    worker.service().send(Command::Preferences(Preferences {
        shuffle: true,
        ..Preferences::default()
    }));
    worker
        .service()
        .send(Command::Player(PlayerCommand::Replace(
            fixture
                .catalog()
                .by_ids(&original.map(str::to_owned))
                .unwrap(),
            2,
        )));
    worker.shutdown(WAIT).unwrap();

    let shuffled: SavedQueue = fixture.catalog().state("queue").unwrap();
    assert_eq!(shuffled.index, Some(0));
    assert_eq!(shuffled.ids[0], "song");
    let mut restarted = fixture.start_paused();
    synced(&restarted);
    restarted
        .service()
        .send(Command::Player(PlayerCommand::ClearError));
    let ids: Vec<_> = shuffled.ids.iter().map(String::as_str).collect();
    let restored = snapshot(&restarted, &ids);
    assert_eq!(restored.index, Some(0));
    restarted
        .service()
        .send(Command::Preferences(Preferences::default()));
    restarted.shutdown(WAIT).unwrap();

    let saved: SavedQueue = fixture.catalog().state("queue").unwrap();
    assert_eq!(
        (
            saved.ids.iter().map(String::as_str).collect::<Vec<_>>(),
            saved.index
        ),
        (original.to_vec(), Some(2)),
        "shuffle-off must recover the order and the later copy"
    );
    fixture.finish();
}

#[test]
fn restart_recovers_when_shuffle_off_was_saved_before_the_queue() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let catalog = fixture.catalog();
    catalog
        .set_state("preferences", &Preferences::default())
        .unwrap();
    catalog
        .set_state(
            "queue",
            &SavedQueue {
                ids: vec!["song".into(), "other-song".into(), "song".into()],
                shuffle_positions: Some(vec![2, 0, 1]),
                index: Some(0),
                position: 23.,
                ..SavedQueue::default()
            },
        )
        .unwrap();
    drop(catalog);

    let mut worker = fixture.start_paused();
    synced(&worker);
    worker.shutdown(WAIT).unwrap();

    let saved: SavedQueue = fixture.catalog().state("queue").unwrap();
    assert_eq!(saved.ids, ["other-song", "song", "song"]);
    assert_eq!(saved.index, Some(2));
    assert_eq!(saved.position, 23.);
    assert!(saved.shuffle_positions.is_none());
    fixture.finish();
}

#[test]
fn legacy_shuffled_queue_loads_without_an_original_order() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let ids = ["other-song", "song"];
    let catalog = fixture.catalog();
    catalog
        .set_state(
            "queue",
            &serde_json::json!({"ids": ids, "index": 1, "position": 23.}),
        )
        .unwrap();
    catalog
        .set_state(
            "preferences",
            &Preferences {
                shuffle: true,
                ..Preferences::default()
            },
        )
        .unwrap();
    drop(catalog);

    let mut worker = fixture.start_paused();
    synced(&worker);
    worker
        .service()
        .send(Command::Player(PlayerCommand::ClearError));
    let restored = snapshot(&worker, &ids);
    assert_eq!(restored.index, Some(1));
    assert_eq!(restored.position, 23.);
    worker
        .service()
        .send(Command::Preferences(Preferences::default()));
    worker.shutdown(WAIT).unwrap();
    let saved: SavedQueue = fixture.catalog().state("queue").unwrap();
    assert_eq!(saved.ids, ids);
    assert_eq!(saved.index, Some(1));
    fixture.finish();
}

#[test]
fn restart_preserves_duplicate_order_index_position_and_listen() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let listen = ListenSession {
        id: 42,
        account: fixture.account.namespace(),
        track_id: "song".into(),
        started_ms: 1700000000000,
        played_seconds: 19.,
        submitted: false,
        announced: true,
    };
    let mut worker = fixture.start_paused();
    synced(&worker);
    worker
        .service()
        .send(Command::Player(PlayerCommand::Restore(
            fixture
                .catalog()
                .by_ids(&["song".into(), "other-song".into(), "song".into()])
                .unwrap(),
            2,
            23.,
            Some(listen.clone()),
        )));
    let before = snapshot(&worker, &["song", "other-song", "song"]);
    assert_eq!(before.index, Some(2));
    worker.shutdown(WAIT).unwrap();
    let saved: SavedQueue = fixture.catalog().state("queue").unwrap();
    assert_eq!(saved.ids, ["song", "other-song", "song"]);
    assert_eq!(
        serde_json::to_value(&saved.listen).unwrap(),
        serde_json::to_value(Some(listen)).unwrap()
    );
    let mut restarted = fixture.start_paused();
    synced(&restarted);
    restarted
        .service()
        .send(Command::Player(PlayerCommand::ClearError));
    let after = snapshot(&restarted, &["song", "other-song", "song"]);
    assert_eq!(after.index, before.index);
    assert_eq!(after.position, before.position);
    assert!(!after.playing);
    restarted.shutdown(WAIT).unwrap();
    let after: SavedQueue = fixture.catalog().state("queue").unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(saved).unwrap()
    );
    fixture.finish();
}

#[test]
fn restart_skips_missing_tracks_without_moving_the_current_occurrence() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let catalog = fixture.catalog();
    catalog
        .set_state(
            "preferences",
            &Preferences {
                shuffle: true,
                ..Preferences::default()
            },
        )
        .unwrap();
    catalog
        .set_state(
            "queue",
            &SavedQueue {
                shuffle_positions: Some(vec![1, 3, 2, 0]),
                ids: vec![
                    "song".into(),
                    "missing".into(),
                    "song".into(),
                    "other-song".into(),
                ],
                index: Some(2),
                position: 23.,
                listen: Some(ListenSession {
                    id: 43,
                    account: fixture.account.namespace(),
                    track_id: "song".into(),
                    started_ms: 1700000000000,
                    played_seconds: 19.,
                    submitted: false,
                    announced: true,
                }),
            },
        )
        .unwrap();
    drop(catalog);
    let mut worker = fixture.start_paused();
    synced(&worker);
    worker
        .service()
        .send(Command::Player(PlayerCommand::ClearError));
    let restored = snapshot(&worker, &["song", "song", "other-song"]);
    assert_eq!(
        restored.index,
        Some(1),
        "missing predecessors must not change the current track"
    );
    assert_eq!(restored.position, 23.);
    worker
        .service()
        .send(Command::Preferences(Preferences::default()));
    let unshuffled = snapshot(&worker, &["other-song", "song", "song"]);
    assert_eq!(unshuffled.index, Some(2));
    assert_eq!(unshuffled.position, 23.);
    worker.shutdown(WAIT).unwrap();
    let saved: SavedQueue = fixture.catalog().state("queue").unwrap();
    assert_eq!(saved.listen.unwrap().id, 43);
    fixture.finish();
}

#[test]
fn missing_current_track_resets_position_and_listen_and_empty_queue_is_safe() {
    for ids in [
        vec!["song", "missing", "other-song"],
        vec!["missing", "missing"],
    ] {
        let fixture = Fixture::new(Library::default());
        fixture.seed();
        fixture
            .catalog()
            .set_state(
                "queue",
                &SavedQueue {
                    shuffle_positions: None,
                    ids: ids.iter().map(|s| (*s).into()).collect(),
                    index: Some(1),
                    position: 23.,
                    listen: Some(ListenSession {
                        id: 99,
                        account: fixture.account.namespace(),
                        track_id: "missing".into(),
                        started_ms: 1700000000000,
                        played_seconds: 19.,
                        submitted: false,
                        announced: true,
                    }),
                },
            )
            .unwrap();
        let mut worker = fixture.start_paused();
        synced(&worker);
        worker.shutdown(WAIT).unwrap();
        let saved: SavedQueue = fixture.catalog().state("queue").unwrap();
        assert_eq!(saved.position, 0.);
        assert!(saved.listen.is_none());
        if ids.len() == 3 {
            assert_eq!(saved.ids, ["song", "other-song"]);
            assert_eq!(saved.index, Some(1));
        } else {
            assert!(saved.ids.is_empty());
            assert!(saved.index.is_none());
        }
        fixture.finish();
    }
}

#[test]
fn play_album_matching_favorites_plays_hearted_songs_or_a_favorite_album_in_full() {
    use sonora::{
        model::{LibraryQuery, Sort},
        sorting::{Scope, SortSpec},
    };
    let starred = Some("2026-01-01T00:00:00Z".to_string());
    let mut library = Library::default();
    let mut hearted = crate::fixture::song("hearted", "album");
    hearted.starred = starred.clone();
    library.tracks.push(hearted);
    let mut whole = crate::fixture::album("whole");
    whole.starred = starred;
    library.albums.push(whole);
    for id in ["whole-1", "whole-2"] {
        library.tracks.push(crate::fixture::song(id, "whole"));
    }
    let fixture = Fixture::new(library);
    fixture.seed();
    let mut worker = fixture.start();
    synced(&worker);
    let query = LibraryQuery {
        favorites: true,
        ordering: Some(SortSpec::legacy(Sort::Album, false, Scope::Albums)),
        ..Default::default()
    };
    let played = |worker: &Worker, id: &str| {
        worker
            .service()
            .send(Command::PlayAlbumMatching(query.clone(), id.into()));
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            if let PlayerCommand::Replace(tracks, at) =
                worker.player_commands().recv_deadline(deadline).unwrap()
            {
                assert_eq!(at, 0);
                return tracks
                    .iter()
                    .map(|track| track.id.clone())
                    .collect::<Vec<_>>();
            }
        }
    };
    // Listed for one hearted song: only that song plays.
    assert_eq!(played(&worker, "album"), ["hearted"]);
    // A favorite album is a favorite as a whole.
    assert_eq!(played(&worker, "whole"), ["whole-1", "whole-2"]);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}
