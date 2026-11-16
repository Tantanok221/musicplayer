use crate::fixture::{Fixture, Library, WAIT, event, synced};
use sonora::{
    listening::ListenSession,
    model::{PlaybackSnapshot, SavedQueue},
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
    fixture
        .catalog()
        .set_state(
            "queue",
            &SavedQueue {
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
