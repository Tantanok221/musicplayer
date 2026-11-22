//! Production paused actor, with no audio-device or OS media-control adapters.
use proptest::prelude::*;
use sonora::{
    model::{PlaybackSnapshot, Preferences, Track},
    playback::{
        Placement, PlayerCommand, QueueEdit,
        test_support::{ListeningPlayer, OutputPlayer, PausedPlayer},
    },
};
use std::{sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(5);

fn occurrences(queue: &[Arc<Track>]) -> Vec<String> {
    queue.iter().map(|track| track.title.clone()).collect()
}

fn queue_property_config() -> ProptestConfig {
    let config = ProptestConfig {
        cases: 256,
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(
                "tests/player_queue.proptest-regressions",
            ),
        )),
        ..ProptestConfig::default()
    };
    eprintln!("Queue property: {} generated cases", config.cases);
    config
}

proptest! {
    #![proptest_config(queue_property_config())]

    #[test]
    fn queue_operation_sequences(ops in prop::collection::vec((0u8..17, 0usize..24, 0usize..24), 1..80)) {
        let mut player = PausedPlayer::start(WAIT).unwrap();
        let mut state = player.command(PlayerCommand::ClearError, WAIT).unwrap();
        let mut expected = Vec::<Arc<Track>>::new();
        let mut current: Option<usize> = None;
        let mut original: Option<Vec<String>> = None;
        let mut undo = None;
        let mut shuffled = false;
        let mut serial = 0;
        for (op, a, b) in ops {
            let before = state.clone();
            let selected = current.and_then(|i| expected.get(i)).map(|t| t.title.clone());
            let mut preserves_current = true;
            let command = match op {
                0..=2 => {
                    let added = (0..b % 4).map(|_| {
                        serial += 1;
                        Arc::new(Track { id: format!("track-{}", serial % 3), title: format!("occurrence-{serial}"), album_id: "album".into(), duration: 120., ..Default::default() })
                    }).collect::<Vec<_>>();
                    let placement = match op { 0 => Placement::Next, 1 => Placement::Last, _ => Placement::AfterAlbum };
                    let at = if op == 1 || (op == 2 && current.is_some()) { expected.len() } else { current.map_or(0, |i| i + 1) };
                    if let Some(order) = original.as_mut() {
                        let dest = if op == 1 { order.len() } else if at == 0 { 0 } else { order.iter().position(|id| *id == expected[at - 1].title).unwrap() + 1 };
                        order.splice(dest..dest, occurrences(&added));
                    }
                    expected.splice(at..at, added.clone());
                    undo = None;
                    PlayerCommand::Insert(added, placement)
                }
                3 => {
                    if a < expected.len() && b < expected.len() {
                        let entry = expected.remove(a);
                        expected.insert(b, entry);
                        current = selected.as_ref().and_then(|id| expected.iter().position(|t| &t.title == id));
                    }
                    undo = None;
                    PlayerCommand::Move(a, b)
                }
                4 => {
                    if a < expected.len() {
                        let removed = expected.remove(a);
                        if let Some(order) = original.as_mut() { order.retain(|id| id != &removed.title); }
                        if let Some(i) = current {
                            if a == i { preserves_current = false; current = (!expected.is_empty()).then_some(i.min(expected.len().saturating_sub(1))); }
                            else if a < i { current = Some(i - 1); }
                        }
                    }
                    undo = None;
                    PlayerCommand::Remove(a)
                }
                5..=7 => {
                    let edit = match op { 5 => QueueEdit::Remove(vec![a, b]), 6 => QueueEdit::Move { indices: vec![a], before: b }, _ => QueueEdit::ClearUpcoming };
                    let start = current.map_or(0, |i| i + 1);
                    let mut indices = match op { 5 => vec![a, b], 6 => vec![a], _ => (start..expected.len()).collect() };
                    indices.sort_unstable(); indices.dedup();
                    if !indices.is_empty() && indices.iter().all(|i| *i >= start && *i < expected.len()) && (op != 6 || (b >= start && b <= expected.len())) {
                        let saved = (expected.clone(), original.clone());
                        let moving = indices.iter().map(|i| expected[*i].clone()).collect::<Vec<_>>();
                        let mut result = expected.iter().enumerate().filter(|(i, _)| !indices.contains(i)).map(|(_, t)| t.clone()).collect::<Vec<_>>();
                        if op == 6 { let dest = (0..b).filter(|i| !indices.contains(i)).count(); result.splice(dest..dest, moving); }
                        if occurrences(&result) != occurrences(&expected) {
                            if let Some(order) = original.as_mut() { order.retain(|id| result.iter().any(|t| &t.title == id)); }
                            expected = result; undo = Some(saved);
                        }
                    }
                    PlayerCommand::EditQueue(state.queue_revision, edit)
                }
                8 => {
                    if let Some((queue, order)) = undo.take() { expected = queue; original = order; current = selected.as_ref().and_then(|id| expected.iter().position(|t| &t.title == id)); }
                    PlayerCommand::UndoQueue
                }
                9 | 10 => {
                    let value = op == 9;
                    if value && !shuffled { if current.is_some() { original = Some(occurrences(&expected)); } undo = None; }
                    if !value && shuffled && let Some(order) = original.take() {
                        expected.sort_by_key(|t| order.iter().position(|id| id == &t.title).unwrap());
                        current = selected.as_ref().and_then(|id| expected.iter().position(|t| &t.title == id));
                        undo = None;
                    }
                    shuffled = value;
                    PlayerCommand::Preferences(Preferences { shuffle: value, ..Default::default() })
                }
                11 => {
                    preserves_current = false;
                    if a < expected.len() { current = Some(a); }
                    PlayerCommand::JumpTo(a)
                }
                12 => {
                    preserves_current = false;
                    current = current.map(|i: usize| i.saturating_sub(1));
                    PlayerCommand::Previous
                }
                13 => {
                    // Restoring persisted IDs must preserve their order and duplicate slot.
                    let (tx, rx) = flume::bounded(1);
                    player.command(PlayerCommand::SaveQueue(tx), WAIT).unwrap();
                    let saved = rx.recv_timeout(WAIT).unwrap();
                    let saved: sonora::model::SavedQueue = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
                    prop_assert_eq!(&saved.ids, &expected.iter().map(|t| t.id.clone()).collect::<Vec<_>>());
                    prop_assert_eq!(saved.index, current);
                    original = None; undo = None;
                    // The restore command selects slot zero when persistence has no selection.
                    if current.is_none() && !expected.is_empty() { current = Some(0); preserves_current = false; }
                    if a % 2 == 0 { PlayerCommand::Restore(expected.clone(), saved.index.unwrap_or(0), saved.position, saved.listen) }
                    else { PlayerCommand::RestoreAccount("test-account".into(), expected.clone(), saved.index.unwrap_or(0), saved.position, saved.listen) }
                }
                14 => {
                    PlayerCommand::EditQueue(state.queue_revision.wrapping_add(1), QueueEdit::ClearUpcoming)
                }
                16 => {
                    preserves_current = false;
                    current = current.and_then(|i| (i + 1 < expected.len()).then_some(i + 1));
                    PlayerCommand::Next
                }
                _ => {
                    preserves_current = false;
                    expected = (0..b % 5).map(|_| {
                        serial += 1;
                        Arc::new(Track { id: format!("track-{}", serial % 3), title: format!("occurrence-{serial}"), album_id: "album".into(), duration: 120., ..Default::default() })
                    }).collect();
                    current = (!expected.is_empty()).then_some(a.min(expected.len().saturating_sub(1)));
                    original = shuffled.then(|| occurrences(&expected)).filter(|order| !order.is_empty()); undo = None;
                    let command = PlayerCommand::Replace(expected.clone(), a);
                    if shuffled && let Some(i) = current { let track = expected.remove(i); expected.insert(0, track); current = Some(0); }
                    command
                }
            };
            state = player.command(command, WAIT).unwrap();
            if (op == 9 && !before.queue.is_empty()) || (op == 15 && shuffled) {
                let mut actual = occurrences(&state.queue); actual.sort();
                let mut wanted = occurrences(&expected); wanted.sort();
                prop_assert_eq!(actual, wanted);
                if let Some(i) = current { prop_assert_eq!(occurrences(&state.queue[..=i]), occurrences(&expected[..=i])); }
                expected = state.queue.as_ref().clone();
            }
            prop_assert_eq!(occurrences(&state.queue), occurrences(&expected), "operation {} ({}, {})", op, a, b);
            prop_assert_eq!(state.index, current, "operation {} ({}, {})", op, a, b);
            prop_assert!(state.index.is_none_or(|i| i < state.queue.len()));
            prop_assert_eq!(state.queue_ids.len(), state.queue.len());
            let mut ids = state.queue_ids.as_ref().clone(); ids.sort(); ids.dedup();
            prop_assert_eq!(ids.len(), state.queue.len());
            if preserves_current { prop_assert_eq!(state.index.map(|i| state.queue[i].title.clone()), selected); }
        }
        player.shutdown(WAIT).unwrap();
    }

    #[test]
    fn queue_metadata_refresh_keeps_duplicate_occurrence(len in 2usize..24, chosen in any::<usize>(), moves in prop::collection::vec((0usize..24, 0usize..24), 0..30)) {
        let mut player = PausedPlayer::start(WAIT).unwrap();
        let shared = tracks(&["same"]).remove(0);
        let mut current = chosen % len;
        let mut order = (0..len).collect::<Vec<_>>();
        let mut state = player.command(PlayerCommand::Restore(vec![shared; len], current, 17., None), WAIT).unwrap();
        prop_assert_eq!(state.index, Some(current));
        for (from, to) in moves {
            if from < len && to < len { let entry = order.remove(from); order.insert(to, entry); }
            current = order.iter().position(|i| *i == chosen % len).unwrap();
            state = player.command(PlayerCommand::Move(from, to), WAIT).unwrap();
            prop_assert_eq!(state.index, Some(current));
        }
        player.command(PlayerCommand::Preferences(Preferences { shuffle: true, ..Default::default() }), WAIT).unwrap();
        state = player.command(PlayerCommand::UpdateTrack(Arc::new(Track { id: "same".into(), title: "Updated".into(), duration: 120., ..Default::default() })), WAIT).unwrap();
        prop_assert_eq!(state.index, Some(current));
        prop_assert!(state.queue.iter().all(|t| t.title == "Updated"));
        state = player.command(PlayerCommand::Preferences(Preferences::default()), WAIT).unwrap();
        prop_assert_eq!(state.index, Some(current));
        prop_assert!(state.queue.iter().all(|t| t.title == "Updated"));
        prop_assert_eq!(state.position, 17.);
        player.shutdown(WAIT).unwrap();
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
