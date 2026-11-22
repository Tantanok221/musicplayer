//! Production paused actor, with no audio-device or OS media-control adapters.
use sonora::{
    model::{PlaybackSnapshot, Track},
    playback::{
        Placement, PlayerCommand, QueueEdit,
        test_support::{OutputPlayer, PausedPlayer},
    },
};
use std::{sync::Arc, time::Duration};

const WAIT: Duration = Duration::from_secs(5);

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
