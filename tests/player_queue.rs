//! Production paused actor, with no audio-device or OS media-control adapters.
use sonora::{
    model::{PlaybackSnapshot, Preferences, Track},
    playback::{
        Placement, PlayerCommand, QueueEdit,
        test_support::{ListeningPlayer, OutputPlayer, PausedPlayer},
    },
};
use std::{sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(5);

#[test]
fn late_previous_listen_progress_does_not_duplicate_scrobbles() {
    let mut player = ListeningPlayer::start(WAIT).unwrap();
    for _ in 0..20 {
        let previous = player
            .restart(PlayerCommand::Replace(tracks(&["previous"]), 0), WAIT)
            .unwrap();
        assert_eq!(player.progress(50., WAIT).unwrap().len(), 1);
        let current = player
            .restart(PlayerCommand::Replace(tracks(&["current"]), 0), WAIT)
            .unwrap();
        assert_eq!(player.progress(60., WAIT).unwrap().len(), 2);
        let reports = player.previous_progress(10., WAIT).unwrap();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].submission);
        assert_eq!(reports[0].id, previous);
        assert!(player.previous_progress(60., WAIT).unwrap().is_empty());
        assert!(player.progress(60., WAIT).unwrap().is_empty());
        assert_ne!(previous, current);
    }
}

#[test]
fn play_after_queue_end_starts_a_fresh_listen_and_scrobble_cycle() {
    for replay in [PlayerCommand::Play, PlayerCommand::Toggle] {
        let mut player = ListeningPlayer::start(WAIT).unwrap();
        let first = player
            .restart(PlayerCommand::Replace(tracks(&["last"]), 0), WAIT)
            .unwrap();
        let reports = player.progress(60., WAIT).unwrap();
        assert_eq!(reports.len(), 2);
        assert!(!reports[0].submission);
        assert!(reports[1].submission);
        assert!(player.progress(60., WAIT).unwrap().is_empty());
        let ended = player.finish(WAIT).unwrap();
        assert!(!ended.playing);
        assert_eq!(ended.index, Some(0));
        assert_eq!(ended.position, 0.);
        let second = player.restart(replay, WAIT).unwrap();
        assert_ne!(first, second, "replay must not reuse the completed listen");
        let reports = player.progress(60., WAIT).unwrap();
        assert_eq!(reports.len(), 2);
        assert!(!reports[0].submission);
        assert!(reports[1].submission);
        assert!(reports.iter().all(|report| report.id == second));
        assert!(player.progress(60., WAIT).unwrap().is_empty());
    }
}

#[test]
fn pause_and_retry_preserve_the_current_listen() {
    let mut player = ListeningPlayer::start(WAIT).unwrap();
    let first = player
        .restart(PlayerCommand::Replace(tracks(&["current"]), 0), WAIT)
        .unwrap();
    assert_eq!(player.progress(30., WAIT).unwrap().len(), 1);
    player.command(PlayerCommand::Pause).unwrap();
    player.command(PlayerCommand::Play).unwrap();
    assert_eq!(player.restart(PlayerCommand::Retry, WAIT).unwrap(), first);
    let reports = player.progress(30., WAIT).unwrap();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].submission);
    assert_eq!(reports[0].id, first);
}

fn tracks(ids: &[&str]) -> Vec<Arc<Track>> {
    ids.iter()
        .map(|id| {
            Arc::new(Track {
                id: id.to_string(),
                album_id: "album".into(),
                duration: 120.,
                ..Default::default()
            })
        })
        .collect()
}

fn ids(snapshot: &PlaybackSnapshot) -> Vec<&str> {
    snapshot
        .queue
        .iter()
        .map(|track| track.id.as_str())
        .collect()
}

#[test]
fn legacy_moves_preserve_the_current_duplicate_occurrence() {
    let mut player = PausedPlayer::start(WAIT).unwrap();
    for (current, from, to, expected, expected_ids) in [
        (2, 1, 0, 2, ["B", "A", "A"]),
        (2, 2, 1, 1, ["A", "A", "B"]),
        (1, 2, 0, 2, ["A", "A", "B"]),
        (1, 0, 2, 0, ["B", "A", "A"]),
        (2, 1, 1, 2, ["A", "B", "A"]),
        (2, 3, 0, 2, ["A", "B", "A"]),
        (2, 0, 3, 2, ["A", "B", "A"]),
    ] {
        let initial = player
            .command(
                PlayerCommand::Restore(tracks(&["A", "B", "A"]), current, 17., None),
                WAIT,
            )
            .unwrap();
        let moved = player.command(PlayerCommand::Move(from, to), WAIT).unwrap();
        assert_eq!(ids(&moved), expected_ids);
        assert_eq!(
            moved.index,
            Some(expected),
            "current {current}, move {from} to {to}"
        );
        assert!(Arc::ptr_eq(&moved.queue[expected], &initial.queue[current]));
        assert_eq!(moved.position, 17.);
        assert!(!moved.playing);
    }
    player
        .command(PlayerCommand::Restore(vec![], 0, 0., None), WAIT)
        .unwrap();
    let empty = player.command(PlayerCommand::Move(0, 0), WAIT).unwrap();
    assert!(empty.queue.is_empty());
    assert_eq!(empty.index, None);
    player.shutdown(WAIT).unwrap();
}

#[test]
fn unshuffle_after_replace_keeps_selected_duplicate_and_its_next_track() {
    let mut player = PausedPlayer::start(WAIT).unwrap();
    player
        .command(
            PlayerCommand::Preferences(Preferences {
                shuffle: true,
                ..Default::default()
            }),
            WAIT,
        )
        .unwrap();
    let shuffled = player
        .command(
            PlayerCommand::Replace(tracks(&["same", "middle", "same", "last"]), 2),
            WAIT,
        )
        .unwrap();
    assert_eq!(shuffled.index, Some(0));
    assert_eq!(shuffled.queue[0].id, "same");
    let restored = player
        .command(PlayerCommand::Preferences(Preferences::default()), WAIT)
        .unwrap();
    assert_eq!(ids(&restored), ["same", "middle", "same", "last"]);
    assert_eq!(restored.index, Some(2));
    let next = player.command(PlayerCommand::Next, WAIT).unwrap();
    assert_eq!(next.index, Some(3));
    assert_eq!(next.queue[3].id, "last");
    player.shutdown(WAIT).unwrap();
}

#[test]
fn unshuffle_after_track_update_keeps_later_duplicate_current() {
    let mut player = PausedPlayer::start(WAIT).unwrap();
    let initial = player
        .command(
            PlayerCommand::Restore(
                tracks(&["same", "middle", "same", "next", "last"]),
                2,
                17.,
                None,
            ),
            WAIT,
        )
        .unwrap();
    player
        .command(
            PlayerCommand::Preferences(Preferences {
                shuffle: true,
                ..Default::default()
            }),
            WAIT,
        )
        .unwrap();
    let updated = player
        .command(
            PlayerCommand::UpdateTrack(Arc::new(Track {
                id: "same".into(),
                title: "Fresh metadata".into(),
                duration: 120.,
                ..Default::default()
            })),
            WAIT,
        )
        .unwrap();
    assert_eq!(updated.index, Some(2));
    let restored = player
        .command(PlayerCommand::Preferences(Preferences::default()), WAIT)
        .unwrap();
    assert_eq!(restored.index, Some(2));
    assert_eq!(ids(&restored), ids(&initial));
    assert_eq!(restored.position, 17.);
    assert_eq!(restored.queue[0].title, "Fresh metadata");
    assert_eq!(restored.queue[2].title, "Fresh metadata");
    player.shutdown(WAIT).unwrap();
}

#[test]
fn shuffled_occurrences_survive_insert_remove_undo_and_metadata_refresh() {
    let mut player = PausedPlayer::start(WAIT).unwrap();
    player
        .command(
            PlayerCommand::Preferences(Preferences {
                shuffle: true,
                ..Default::default()
            }),
            WAIT,
        )
        .unwrap();
    let mut queue = tracks(&["same", "middle", "same", "last"]);
    Arc::make_mut(&mut queue[0]).title = "Earlier copy".into();
    player
        .command(PlayerCommand::Replace(queue, 2), WAIT)
        .unwrap();
    player
        .command(
            PlayerCommand::Insert(tracks(&["inserted"]), Placement::Next),
            WAIT,
        )
        .unwrap();
    let inserted = player
        .command(
            PlayerCommand::Insert(tracks(&["same"]), Placement::Last),
            WAIT,
        )
        .unwrap();
    let earlier = inserted
        .queue
        .iter()
        .position(|track| track.title == "Earlier copy")
        .unwrap();
    let removed = player
        .command(
            PlayerCommand::EditQueue(inserted.queue_revision, QueueEdit::Remove(vec![earlier])),
            WAIT,
        )
        .unwrap();
    let stale = player
        .command(
            PlayerCommand::EditQueue(inserted.queue_revision, QueueEdit::ClearUpcoming),
            WAIT,
        )
        .unwrap();
    assert_eq!(stale.queue_ids, removed.queue_ids);
    player
        .command(
            PlayerCommand::UpdateTrack(Arc::new(Track {
                id: "same".into(),
                title: "Updated".into(),
                ..Default::default()
            })),
            WAIT,
        )
        .unwrap();
    let undone = player.command(PlayerCommand::UndoQueue, WAIT).unwrap();
    assert_eq!(undone.queue_ids, inserted.queue_ids);
    assert!(
        undone
            .queue
            .iter()
            .filter(|track| track.id == "same")
            .all(|track| track.title == "Updated")
    );
    let removed = player
        .command(
            PlayerCommand::EditQueue(undone.queue_revision, QueueEdit::Remove(vec![earlier])),
            WAIT,
        )
        .unwrap();
    player
        .command(
            PlayerCommand::EditQueue(
                removed.queue_revision,
                QueueEdit::Move {
                    indices: vec![1],
                    before: removed.queue.len(),
                },
            ),
            WAIT,
        )
        .unwrap();
    let restored = player
        .command(PlayerCommand::Preferences(Preferences::default()), WAIT)
        .unwrap();
    assert_eq!(
        ids(&restored),
        ["middle", "same", "inserted", "last", "same"]
    );
    assert_eq!(restored.index, Some(1));
    let next = player.command(PlayerCommand::Next, WAIT).unwrap();
    assert_eq!(next.index, Some(2));
    assert_eq!(next.queue[2].id, "inserted");
    player.shutdown(WAIT).unwrap();
}

#[test]
fn paused_actor_rejects_stale_edits_and_undo_keeps_the_exact_duplicate_occurrence() {
    let mut player = PausedPlayer::start(WAIT).unwrap();
    let initial = player
        .command(
            PlayerCommand::Restore(
                tracks(&["past", "same", "remove", "same", "last"]),
                1,
                17.,
                None,
            ),
            WAIT,
        )
        .unwrap();
    assert_ne!(initial.queue_ids[1], initial.queue_ids[3]);
    let moved = player
        .command(
            PlayerCommand::EditQueue(
                initial.queue_revision,
                QueueEdit::Move {
                    indices: vec![3],
                    before: 2,
                },
            ),
            WAIT,
        )
        .unwrap();
    assert_eq!(ids(&moved), ["past", "same", "same", "remove", "last"]);
    assert_eq!(
        *moved.queue_ids,
        [
            initial.queue_ids[0],
            initial.queue_ids[1],
            initial.queue_ids[3],
            initial.queue_ids[2],
            initial.queue_ids[4]
        ]
    );
    assert_eq!(moved.index, Some(1));
    assert_eq!(moved.position, 17.);
    assert!(!moved.playing);
    let stale = player
        .command(
            PlayerCommand::EditQueue(initial.queue_revision, QueueEdit::Remove(vec![2])),
            WAIT,
        )
        .unwrap();
    assert_eq!(stale.queue_ids, moved.queue_ids);
    assert_eq!(stale.queue_revision, moved.queue_revision);
    assert!(
        stale.can_undo_queue,
        "a rejected edit preserves the prior Undo"
    );
    for protected in [0, 1] {
        let rejected = player
            .command(
                PlayerCommand::EditQueue(moved.queue_revision, QueueEdit::Remove(vec![protected])),
                WAIT,
            )
            .unwrap();
        assert_eq!(rejected.queue_ids, moved.queue_ids);
        assert_eq!(rejected.queue_revision, moved.queue_revision);
    }
    let undone = player.command(PlayerCommand::UndoQueue, WAIT).unwrap();
    assert_eq!(undone.queue_ids, initial.queue_ids);
    assert_eq!(ids(&undone), ids(&initial));
    assert_eq!(undone.index, Some(1));
    assert_eq!(undone.position, 17.);
    assert!(!undone.can_undo_queue);
    let repeated = player.command(PlayerCommand::UndoQueue, WAIT).unwrap();
    assert_eq!(repeated.queue_ids, undone.queue_ids);
    assert_eq!(repeated.queue_revision, undone.queue_revision);
    player.shutdown(WAIT).unwrap();
}

#[test]
fn paused_actor_insertion_clear_and_undo_preserve_current_position_and_metadata() {
    let mut player = PausedPlayer::start(WAIT).unwrap();
    let initial = player
        .command(
            PlayerCommand::Restore(tracks(&["current", "next", "last"]), 0, 11., None),
            WAIT,
        )
        .unwrap();
    let next = player
        .command(
            PlayerCommand::Insert(tracks(&["insert-next"]), Placement::Next),
            WAIT,
        )
        .unwrap();
    assert_eq!(ids(&next), ["current", "insert-next", "next", "last"]);
    let last = player
        .command(
            PlayerCommand::Insert(tracks(&["insert-last"]), Placement::Last),
            WAIT,
        )
        .unwrap();
    assert_eq!(
        ids(&last),
        ["current", "insert-next", "next", "last", "insert-last"]
    );
    assert_eq!(last.index, initial.index);
    assert_eq!(last.position, initial.position);
    let cleared = player
        .command(
            PlayerCommand::EditQueue(last.queue_revision, QueueEdit::ClearUpcoming),
            WAIT,
        )
        .unwrap();
    assert_eq!(ids(&cleared), ["current"]);
    assert!(cleared.can_undo_queue);
    let updated = Arc::new(Track {
        id: "next".into(),
        title: "Fresh metadata".into(),
        duration: 120.,
        ..Default::default()
    });
    player
        .command(PlayerCommand::UpdateTrack(updated), WAIT)
        .unwrap();
    let undone = player.command(PlayerCommand::UndoQueue, WAIT).unwrap();
    assert_eq!(undone.queue_ids, last.queue_ids);
    assert_eq!(ids(&undone), ids(&last));
    assert_eq!(undone.queue[2].title, "Fresh metadata");
    assert_eq!(undone.position, 11.);
    assert!(!undone.playing);
    assert!(player.command(PlayerCommand::Play, WAIT).is_err());
    assert!(player.command(PlayerCommand::Seek(60.), WAIT).is_err());
    player.shutdown(WAIT).unwrap();
}

#[cfg(target_os = "macos")]
const OUTPUT_INTERRUPTED: &str =
    "Audio output changed or Mac woke from sleep. Press Play to resume.";
#[cfg(not(target_os = "macos"))]
const OUTPUT_INTERRUPTED: &str =
    "Audio output changed or the computer woke from sleep. Press Play to resume.";

#[test]
fn output_change_while_playing_stops_with_this_platforms_wording() {
    let player = OutputPlayer::start("speakers", WAIT).unwrap();
    player.switch_output(Some("headphones"));
    player.stays_clear(Duration::from_millis(1500)).unwrap();

    player.start_audio();
    player.switch_output(Some("speakers"));
    let stopped = player.next_error(WAIT).unwrap();
    assert!(!stopped.playing);
    assert_eq!(stopped.error.as_deref(), Some(OUTPUT_INTERRUPTED));
    #[cfg(not(target_os = "macos"))]
    assert!(!OUTPUT_INTERRUPTED.contains("Mac"));

    assert_eq!(player.clear_error(WAIT).unwrap().error, None);
    player.start_audio();
    player.switch_output(None);
    let disconnected = player.next_error(WAIT).unwrap();
    assert_eq!(
        disconnected.error.as_deref(),
        Some("Audio output disconnected. Connect a device and retry.")
    );
}
