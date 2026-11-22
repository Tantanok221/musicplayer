use crate::model::Track;
use rand::seq::SliceRandom;
use std::sync::Arc;

/// Original-order ranks aligned with queue occurrences, independent of track identity.
#[derive(Clone)]
pub(super) struct ShuffleOrder {
    positions: Vec<usize>,
}

impl ShuffleOrder {
    pub fn new(len: usize) -> Self {
        Self {
            positions: (0..len).collect(),
        }
    }
    pub fn shuffle(&mut self, tracks: &mut Vec<Arc<Track>>, start: usize) {
        let mut entries = self
            .positions
            .iter()
            .copied()
            .zip(std::mem::take(tracks))
            .collect::<Vec<_>>();
        entries[start..].shuffle(&mut rand::rng());
        (self.positions, *tracks) = entries.into_iter().unzip();
    }
    pub fn insert(&mut self, at: usize, len: usize, last: bool) {
        let after = if last {
            self.positions.iter().max().map_or(0, |i| i + 1)
        } else {
            at.checked_sub(1).map_or(0, |i| self.positions[i] + 1)
        };
        for position in &mut self.positions {
            if *position >= after {
                *position += len;
            }
        }
        self.positions.splice(at..at, after..after + len);
    }
    pub fn remove(&mut self, at: usize) {
        self.positions.remove(at);
    }
    pub fn move_entry(&mut self, from: usize, to: usize) {
        let position = self.positions.remove(from);
        self.positions.insert(to, position);
    }
    pub fn reorder(&mut self, order: &[usize]) {
        self.positions = order.iter().map(|i| self.positions[*i]).collect();
    }
    pub fn restore(
        self,
        tracks: Vec<Arc<Track>>,
        current: Option<usize>,
    ) -> (Vec<Arc<Track>>, Option<usize>) {
        let mut entries = self
            .positions
            .into_iter()
            .zip(tracks)
            .enumerate()
            .collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(_, (position, _))| *position);
        let current = entries.iter().position(|(i, _)| Some(*i) == current);
        (
            entries.into_iter().map(|(_, (_, track))| track).collect(),
            current,
        )
    }
}

/// Edits address the published play order, including when shuffle is enabled.
#[derive(Clone, Debug)]
pub enum QueueEdit {
    Remove(Vec<usize>),
    /// Insert the selected entries before this position in the existing queue.
    Move {
        indices: Vec<usize>,
        before: usize,
    },
    ClearUpcoming,
}

fn edited_order(len: usize, current: Option<usize>, edit: &QueueEdit) -> Option<Vec<usize>> {
    let start = current.map_or(0, |i| (i + 1).min(len));
    let mut selected = match edit {
        QueueEdit::Remove(indices) | QueueEdit::Move { indices, .. } => indices.clone(),
        QueueEdit::ClearUpcoming => (start..len).collect(),
    };
    selected.sort_unstable();
    selected.dedup();
    if selected.is_empty() || selected.iter().any(|i| *i < start || *i >= len) {
        return None;
    }
    let mut order = (0..len)
        .filter(|i| selected.binary_search(i).is_err())
        .collect::<Vec<_>>();
    if let QueueEdit::Move { before, .. } = edit {
        if *before < start || *before > len {
            return None;
        }
        let destination = order.partition_point(|i| i < before);
        order.splice(destination..destination, selected);
    }
    (order.iter().copied().ne(0..len)).then_some(order)
}

struct QueueUndo {
    tracks: Vec<Arc<Track>>,
    original: Option<ShuffleOrder>,
    ids: Arc<Vec<u64>>,
    /// Each post-edit index identifies its pre-edit occurrence, even for duplicates.
    order: Vec<usize>,
}

#[derive(Default)]
pub(super) struct QueueEdits {
    ids: Arc<Vec<u64>>,
    next_id: u64,
    revision: u64,
    undo: Option<QueueUndo>,
}

impl QueueEdits {
    pub fn ids(&self) -> Arc<Vec<u64>> {
        self.ids.clone()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn can_undo(&self) -> bool {
        self.undo.is_some()
    }
    /// Replacements, insertions, and shuffle changes start a new editing context.
    pub fn reset(&mut self, len: usize) {
        self.ids = Arc::new(
            (0..len)
                .map(|_| {
                    self.next_id += 1;
                    self.next_id
                })
                .collect(),
        );
        self.undo = None;
        self.revision += 1;
    }
    pub fn apply(
        &mut self,
        revision: u64,
        edit: &QueueEdit,
        tracks: &mut Vec<Arc<Track>>,
        original: &mut Option<ShuffleOrder>,
        current: Option<usize>,
    ) -> bool {
        if revision != self.revision {
            return false;
        }
        let Some(order) = edited_order(tracks.len(), current, edit) else {
            return false;
        };
        let before = std::mem::take(tracks);
        let before_original = original.clone();
        *tracks = order.iter().map(|i| before[*i].clone()).collect();
        if let Some(original) = original.as_mut() {
            original.reorder(&order);
        }
        let ids = self.ids.clone();
        self.ids = Arc::new(order.iter().map(|i| ids[*i]).collect());
        self.undo = Some(QueueUndo {
            tracks: before,
            original: before_original,
            ids,
            order,
        });
        self.revision += 1;
        true
    }
    pub fn undo(
        &mut self,
        tracks: &mut Vec<Arc<Track>>,
        original: &mut Option<ShuffleOrder>,
        current: &mut Option<usize>,
    ) -> bool {
        let Some(undo) = self.undo.take() else {
            return false;
        };
        *current = current.and_then(|i| undo.order.get(i).copied());
        *tracks = undo.tracks;
        *original = undo.original;
        self.ids = undo.ids;
        self.revision += 1;
        true
    }
    pub fn update_track(&mut self, track: &Arc<Track>) {
        if let Some(undo) = self.undo.as_mut() {
            for queued in &mut undo.tracks {
                if queued.id == track.id {
                    *queued = track.clone();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks(ids: &[&str]) -> Vec<Arc<Track>> {
        ids.iter()
            .map(|id| {
                Arc::new(Track {
                    id: (*id).into(),
                    ..Default::default()
                })
            })
            .collect()
    }

    #[test]
    fn shuffle_order_tracks_shared_arcs_through_moves_and_insertions() {
        let same = tracks(&["same"]).remove(0);
        let mut queue = vec![same.clone(), tracks(&["middle"]).remove(0), same];
        let mut order = ShuffleOrder::new(queue.len());
        queue.swap(0, 2);
        order.move_entry(2, 0);
        order.move_entry(2, 1);
        order.insert(1, 1, false);
        queue.insert(1, tracks(&["next"]).remove(0));
        order.insert(0, 1, false);
        queue.insert(0, tracks(&["first"]).remove(0));
        order.insert(queue.len(), 1, true);
        queue.extend(tracks(&["last"]));
        order.remove(4);
        queue.remove(4);
        let (queue, current) = order.restore(queue, Some(1));
        assert_eq!(
            queue
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "middle", "same", "next", "last"]
        );
        assert_eq!(current, Some(2));
    }

    #[test]
    fn moves_keep_selection_order_and_reject_current_past_or_stale_edits() {
        assert_eq!(
            edited_order(
                6,
                Some(1),
                &QueueEdit::Move {
                    indices: vec![5, 3],
                    before: 2
                }
            ),
            Some(vec![0, 1, 3, 5, 2, 4])
        );
        assert_eq!(
            edited_order(
                6,
                Some(1),
                &QueueEdit::Move {
                    indices: vec![2, 3],
                    before: 6
                }
            ),
            Some(vec![0, 1, 4, 5, 2, 3])
        );
        assert!(edited_order(4, Some(1), &QueueEdit::Remove(vec![1, 2])).is_none());
        assert!(
            edited_order(
                4,
                Some(1),
                &QueueEdit::Move {
                    indices: vec![2],
                    before: 0
                }
            )
            .is_none()
        );
        let mut queue = tracks(&["a", "b", "c"]);
        let mut edits = QueueEdits::default();
        edits.reset(queue.len());
        assert!(!edits.apply(0, &QueueEdit::ClearUpcoming, &mut queue, &mut None, Some(0)));
        assert!(!edits.can_undo());
    }

    #[test]
    fn undo_follows_the_current_occurrence_after_playback_advances() {
        let mut queue = tracks(&["a", "same", "removed", "same", "last"]);
        let mut original = Some(ShuffleOrder::new(queue.len()));
        let mut edits = QueueEdits::default();
        edits.reset(queue.len());
        let ids = edits.ids();
        assert!(edits.apply(
            edits.revision(),
            &QueueEdit::Remove(vec![2]),
            &mut queue,
            &mut original,
            Some(0)
        ));
        assert_eq!(edits.ids()[2], ids[3]);
        let mut current = Some(2);
        assert!(edits.undo(&mut queue, &mut original, &mut current));
        assert_eq!(current, Some(3), "keep the second occurrence playing");
        assert_eq!(edits.ids(), ids);
        assert_eq!(original.unwrap().positions.len(), 5);
        assert!(!edits.can_undo());
    }

    #[test]
    fn clear_upcoming_and_undo_preserve_current_and_updated_metadata() {
        let mut queue = tracks(&["old", "current", "next", "last"]);
        let mut original = None;
        let mut edits = QueueEdits::default();
        edits.reset(queue.len());
        assert!(edits.apply(
            edits.revision(),
            &QueueEdit::ClearUpcoming,
            &mut queue,
            &mut original,
            Some(1)
        ));
        assert_eq!(queue.len(), 2);
        let updated = Arc::new(Track {
            id: "next".into(),
            title: "Updated".into(),
            ..Default::default()
        });
        edits.update_track(&updated);
        let mut current = Some(1);
        assert!(edits.undo(&mut queue, &mut original, &mut current));
        assert_eq!(current, Some(1));
        assert_eq!(queue[2].title, "Updated");
        assert!(edits.apply(
            edits.revision(),
            &QueueEdit::ClearUpcoming,
            &mut queue,
            &mut original,
            Some(1)
        ));
        edits.reset(2);
        assert!(!edits.can_undo());
    }
}
