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

#[derive(Clone, Debug)]
enum Op {
    Insert { count: usize, placement: Placement },
    Move { from: usize, to: usize },
    Remove(usize),
    EditRemove(Vec<usize>),
    EditMove { indices: Vec<usize>, before: usize },
    ClearUpcoming,
    StaleEdit,
    Undo,
    Shuffle(bool),
    JumpTo(usize),
    Previous,
    Next,
    SaveRestore { account: bool },
    Replace { count: usize, index: usize },
}

fn queue_op() -> impl Strategy<Value = Op> {
    let placement = prop_oneof![
        Just(Placement::Next),
        Just(Placement::Last),
        Just(Placement::AfterAlbum),
    ];
    prop_oneof![
        3 => (0usize..4, placement)
            .prop_map(|(count, placement)| Op::Insert { count, placement }),
        1 => (0usize..24, 0usize..24).prop_map(|(from, to)| Op::Move { from, to }),
        1 => (0usize..24).prop_map(Op::Remove),
        1 => prop::collection::vec(0usize..24, 2).prop_map(Op::EditRemove),
        1 => (prop::collection::vec(0usize..24, 1), 0usize..24)
            .prop_map(|(indices, before)| Op::EditMove { indices, before }),
        1 => Just(Op::ClearUpcoming),
        1 => Just(Op::StaleEdit),
        1 => Just(Op::Undo),
        2 => any::<bool>().prop_map(Op::Shuffle),
        1 => (0usize..24).prop_map(Op::JumpTo),
        1 => Just(Op::Previous),
        1 => Just(Op::Next),
        1 => any::<bool>().prop_map(|account| Op::SaveRestore { account }),
        1 => (0usize..5, 0usize..24).prop_map(|(count, index)| Op::Replace { count, index }),
    ]
}

fn track_occurrence(serial: usize) -> Arc<Track> {
    // Titles identify occurrences; track IDs deliberately repeat.
    Arc::new(Track {
        id: format!("track-{}", serial % 3),
        title: format!("occurrence-{serial}"),
        album_id: "album".into(),
        duration: 120.,
        ..Default::default()
    })
}

struct QueueModelUndo {
    queue: Vec<Arc<Track>>,
    original: Option<Vec<String>>,
}

#[derive(Default)]
struct QueueModel {
    queue: Vec<Arc<Track>>,
    current: Option<usize>,
    original: Option<Vec<String>>,
    undo: Option<QueueModelUndo>,
    shuffled: bool,
    serial: usize,
    reconcile_shuffle: bool,
    preserved_selection: Option<Option<String>>,
}

impl QueueModel {
    fn selected(&self) -> Option<String> {
        self.current.map(|i| self.queue[i].title.clone())
    }

    fn select_occurrence(&mut self, selected: Option<String>) {
        self.current = selected.and_then(|id| self.queue.iter().position(|t| t.title == id));
    }

    fn new_tracks(&mut self, count: usize) -> Vec<Arc<Track>> {
        (0..count)
            .map(|_| {
                self.serial += 1;
                track_occurrence(self.serial)
            })
            .collect()
    }

    fn command(
        &mut self,
        op: Op,
        state: &PlaybackSnapshot,
        player: &PausedPlayer,
    ) -> PlayerCommand {
        self.preserved_selection = Some(self.selected());
        match op {
            Op::Insert { count, placement } => self.insert(count, placement),
            Op::Move { from, to } => self.move_entry(from, to),
            Op::Remove(at) => self.remove(at),
            Op::EditRemove(indices) => self.edit_remove(indices, state.queue_revision),
            Op::EditMove { indices, before } => {
                self.edit_move(indices, before, state.queue_revision)
            }
            Op::ClearUpcoming => self.clear_upcoming(state.queue_revision),
            Op::StaleEdit => self.stale_edit(state.queue_revision),
            Op::Undo => self.undo(),
            Op::Shuffle(value) => self.shuffle(value),
            Op::JumpTo(at) => self.jump_to(at),
            Op::Previous => self.previous(),
            Op::Next => self.next(),
            Op::SaveRestore { account } => self.save_restore(account, player),
            Op::Replace { count, index } => self.replace(count, index),
        }
    }

    fn insert(&mut self, count: usize, placement: Placement) -> PlayerCommand {
        let added = self.new_tracks(count);
        let at = match (placement, self.current) {
            (Placement::Last, _) | (Placement::AfterAlbum, Some(_)) => self.queue.len(),
            (_, Some(i)) => i + 1,
            (_, None) => 0,
        };
        if let Some(order) = self.original.as_mut() {
            let destination = if placement == Placement::Last {
                order.len()
            } else if at == 0 {
                0
            } else {
                order
                    .iter()
                    .position(|id| *id == self.queue[at - 1].title)
                    .unwrap()
                    + 1
            };
            order.splice(destination..destination, occurrences(&added));
        }
        self.queue.splice(at..at, added.clone());
        self.undo = None;
        PlayerCommand::Insert(added, placement)
    }

    fn move_entry(&mut self, from: usize, to: usize) -> PlayerCommand {
        let selected = self.selected();
        if from < self.queue.len() && to < self.queue.len() {
            let entry = self.queue.remove(from);
            self.queue.insert(to, entry);
            self.select_occurrence(selected);
        }
        self.undo = None;
        PlayerCommand::Move(from, to)
    }

    fn remove(&mut self, at: usize) -> PlayerCommand {
        if at < self.queue.len() {
            let removed = self.queue.remove(at);
            if let Some(order) = self.original.as_mut() {
                order.retain(|id| id != &removed.title);
            }
            if let Some(i) = self.current {
                if at == i {
                    self.preserved_selection = None;
                    self.current = (!self.queue.is_empty())
                        .then_some(i.min(self.queue.len().saturating_sub(1)));
                } else if at < i {
                    self.current = Some(i - 1);
                }
            }
        }
        self.undo = None;
        PlayerCommand::Remove(at)
    }

    fn apply_edit(&mut self, mut indices: Vec<usize>, before: Option<usize>) {
        let start = self.current.map_or(0, |i| i + 1);
        indices.sort_unstable();
        indices.dedup();
        if indices.is_empty()
            || indices.iter().any(|i| *i < start || *i >= self.queue.len())
            || before.is_some_and(|at| at < start || at > self.queue.len())
        {
            return;
        }
        let mut result = self
            .queue
            .iter()
            .enumerate()
            .filter(|(i, _)| !indices.contains(i))
            .map(|(_, track)| track.clone())
            .collect::<Vec<_>>();
        if let Some(before) = before {
            let moving = indices.iter().map(|i| self.queue[*i].clone());
            let destination = (0..before).filter(|i| !indices.contains(i)).count();
            result.splice(destination..destination, moving);
        }
        if occurrences(&result) == occurrences(&self.queue) {
            return;
        }
        self.undo = Some(QueueModelUndo {
            queue: self.queue.clone(),
            original: self.original.clone(),
        });
        if let Some(order) = self.original.as_mut() {
            order.retain(|id| result.iter().any(|track| &track.title == id));
        }
        self.queue = result;
    }

    fn edit_remove(&mut self, indices: Vec<usize>, revision: u64) -> PlayerCommand {
        self.apply_edit(indices.clone(), None);
        PlayerCommand::EditQueue(revision, QueueEdit::Remove(indices))
    }

    fn edit_move(&mut self, indices: Vec<usize>, before: usize, revision: u64) -> PlayerCommand {
        self.apply_edit(indices.clone(), Some(before));
        PlayerCommand::EditQueue(revision, QueueEdit::Move { indices, before })
    }

    fn clear_upcoming(&mut self, revision: u64) -> PlayerCommand {
        let start = self.current.map_or(0, |i| i + 1);
        self.apply_edit((start..self.queue.len()).collect(), None);
        PlayerCommand::EditQueue(revision, QueueEdit::ClearUpcoming)
    }

    fn stale_edit(&mut self, revision: u64) -> PlayerCommand {
        PlayerCommand::EditQueue(revision.wrapping_add(1), QueueEdit::ClearUpcoming)
    }

    fn undo(&mut self) -> PlayerCommand {
        let selected = self.selected();
        if let Some(undo) = self.undo.take() {
            self.queue = undo.queue;
            self.original = undo.original;
            self.select_occurrence(selected);
        }
        PlayerCommand::UndoQueue
    }

    fn shuffle(&mut self, value: bool) -> PlayerCommand {
        let selected = self.selected();
        if value && !self.shuffled {
            if self.current.is_some() {
                self.original = Some(occurrences(&self.queue));
                self.reconcile_shuffle = true;
            }
            self.undo = None;
        }
        if !value
            && self.shuffled
            && let Some(order) = self.original.take()
        {
            self.queue
                .sort_by_key(|track| order.iter().position(|id| id == &track.title).unwrap());
            self.select_occurrence(selected);
            self.undo = None;
        }
        self.shuffled = value;
        PlayerCommand::Preferences(Preferences {
            shuffle: value,
            ..Default::default()
        })
    }

    fn jump_to(&mut self, at: usize) -> PlayerCommand {
        self.preserved_selection = None;
        if at < self.queue.len() {
            self.current = Some(at);
        }
        PlayerCommand::JumpTo(at)
    }

    fn previous(&mut self) -> PlayerCommand {
        self.preserved_selection = None;
        self.current = self.current.map(|i| i.saturating_sub(1));
        PlayerCommand::Previous
    }

    fn next(&mut self) -> PlayerCommand {
        self.preserved_selection = None;
        self.current = self
            .current
            .and_then(|i| (i + 1 < self.queue.len()).then_some(i + 1));
        PlayerCommand::Next
    }

    fn save_restore(&mut self, account: bool, player: &PausedPlayer) -> PlayerCommand {
        let (tx, rx) = flume::bounded(1);
        player.command(PlayerCommand::SaveQueue(tx), WAIT).unwrap();
        let saved = rx.recv_timeout(WAIT).unwrap();
        let saved: sonora::model::SavedQueue =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(
            saved.ids,
            self.queue.iter().map(|t| t.id.clone()).collect::<Vec<_>>()
        );
        assert_eq!(saved.index, self.current);
        self.original = None;
        self.undo = None;
        // Restore selects slot zero when persistence has no selection.
        if self.current.is_none() && !self.queue.is_empty() {
            self.current = Some(0);
            self.preserved_selection = None;
        }
        if account {
            PlayerCommand::RestoreAccount(
                "test-account".into(),
                self.queue.clone(),
                saved.index.unwrap_or(0),
                saved.position,
                saved.listen,
            )
        } else {
            PlayerCommand::Restore(
                self.queue.clone(),
                saved.index.unwrap_or(0),
                saved.position,
                saved.listen,
            )
        }
    }

    fn replace(&mut self, count: usize, index: usize) -> PlayerCommand {
        self.preserved_selection = None;
        self.queue = self.new_tracks(count);
        self.current =
            (!self.queue.is_empty()).then_some(index.min(self.queue.len().saturating_sub(1)));
        self.original = self
            .shuffled
            .then(|| occurrences(&self.queue))
            .filter(|order| !order.is_empty());
        self.undo = None;
        let command = PlayerCommand::Replace(self.queue.clone(), index);
        if self.shuffled
            && let Some(i) = self.current
        {
            let track = self.queue.remove(i);
            self.queue.insert(0, track);
            self.current = Some(0);
            self.reconcile_shuffle = true;
        }
        command
    }

    fn check(&mut self, state: &PlaybackSnapshot) {
        if self.reconcile_shuffle {
            // Accept any suffix permutation, but keep the fixed prefix and original order model.
            let mut actual = state
                .queue
                .iter()
                .map(|track| (&track.title, &track.id))
                .collect::<Vec<_>>();
            let mut expected = self
                .queue
                .iter()
                .map(|track| (&track.title, &track.id))
                .collect::<Vec<_>>();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected);
            if let Some(i) = self.current {
                assert_eq!(
                    occurrences(&state.queue[..=i]),
                    occurrences(&self.queue[..=i])
                );
            }
            self.queue = state.queue.as_ref().clone();
            self.reconcile_shuffle = false;
        }
        assert_eq!(occurrences(&state.queue), occurrences(&self.queue));
        assert_eq!(
            state
                .queue
                .iter()
                .map(|track| &track.id)
                .collect::<Vec<_>>(),
            self.queue.iter().map(|track| &track.id).collect::<Vec<_>>(),
        );
        assert_eq!(state.index, self.current);
        assert!(state.index.is_none_or(|i| i < state.queue.len()));
        assert_eq!(state.queue_ids.len(), state.queue.len());
        let mut ids = state.queue_ids.as_ref().clone();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), state.queue.len());
        if let Some(selected) = &self.preserved_selection {
            assert_eq!(&state.index.map(|i| state.queue[i].title.clone()), selected);
        }
    }
}

fn check_metadata_refresh(len: usize, chosen: usize, moves: Vec<(usize, usize)>) {
    let mut player = PausedPlayer::start(WAIT).unwrap();
    let shared = tracks(&["same"]).remove(0);
    let mut current = chosen % len;
    let mut order = (0..len).collect::<Vec<_>>();
    let mut state = player
        .command(
            PlayerCommand::Restore(vec![shared; len], current, 17., None),
            WAIT,
        )
        .unwrap();
    assert_eq!(state.index, Some(current));
    for (from, to) in moves {
        if from < len && to < len {
            let entry = order.remove(from);
            order.insert(to, entry);
        }
        current = order.iter().position(|i| *i == chosen % len).unwrap();
        state = player.command(PlayerCommand::Move(from, to), WAIT).unwrap();
        assert_eq!(state.index, Some(current));
    }
    player
        .command(
            PlayerCommand::Preferences(Preferences {
                shuffle: true,
                ..Default::default()
            }),
            WAIT,
        )
        .unwrap();
    state = player
        .command(
            PlayerCommand::UpdateTrack(Arc::new(Track {
                id: "same".into(),
                title: "Updated".into(),
                duration: 120.,
                ..Default::default()
            })),
            WAIT,
        )
        .unwrap();
    assert_eq!(state.index, Some(current));
    assert!(state.queue.iter().all(|t| t.title == "Updated"));
    state = player
        .command(PlayerCommand::Preferences(Preferences::default()), WAIT)
        .unwrap();
    assert_eq!(state.index, Some(current));
    assert!(state.queue.iter().all(|t| t.title == "Updated"));
    assert_eq!(state.position, 17.);
    player.shutdown(WAIT).unwrap();
}

proptest! {
    #![proptest_config(queue_property_config())]

    #[test]
    fn queue_operation_sequences(ops in prop::collection::vec(queue_op(), 1..80)) {
        let mut player = PausedPlayer::start(WAIT).unwrap();
        let mut state = player.command(PlayerCommand::ClearError, WAIT).unwrap();
        let mut model = QueueModel::default();
        for op in ops {
            let command = model.command(op, &state, &player);
            state = player.command(command, WAIT).unwrap();
            model.check(&state);
        }
        player.shutdown(WAIT).unwrap();
    }

    #[test]
    fn queue_metadata_refresh_keeps_duplicate_occurrence(
        len in 2usize..24,
        chosen in any::<usize>(),
        moves in prop::collection::vec((0usize..24, 0usize..24), 0..30),
    ) {
        check_metadata_refresh(len, chosen, moves);
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
