use crate::{model::PlaybackSnapshot, playback::QueueEdit};

pub(super) fn next_index(playback: &PlaybackSnapshot) -> usize {
    playback
        .index
        .map_or(0, |i| (i + 1).min(playback.queue.len()))
}

/// The queue occurrences and play order when a drag began.
pub(super) struct QueueDrag {
    revision: u64,
    ids: Vec<u64>,
    first_index: usize,
    title: String,
}

impl QueueDrag {
    pub(super) fn new(playback: &PlaybackSnapshot, indices: &[usize], title: &str) -> Self {
        Self {
            revision: playback.queue_revision,
            ids: indices
                .iter()
                .filter_map(|i| playback.queue_ids.get(*i).copied())
                .collect(),
            first_index: indices[0],
            title: if indices.len() == 1 {
                title.to_owned()
            } else {
                format!("{} tracks", indices.len())
            },
        }
    }

    pub(super) fn ids(&self) -> &[u64] {
        &self.ids
    }

    pub(super) fn title(&self) -> &str {
        &self.title
    }

    pub(super) fn row_destination(&self, revision: u64, row: usize) -> Option<usize> {
        if revision != self.revision || (row == self.first_index && self.ids.len() == 1) {
            return None;
        }
        Some(row + usize::from(self.first_index < row))
    }

    pub(super) fn edit(&self, playback: &PlaybackSnapshot, before: usize) -> Option<QueueEdit> {
        let start = next_index(playback);
        if self.revision != playback.queue_revision
            || !(start..=playback.queue.len()).contains(&before)
            || self.ids.is_empty()
        {
            return None;
        }
        let indices = self
            .ids
            .iter()
            .map(|id| {
                playback
                    .queue_ids
                    .iter()
                    .position(|value| value == id)
                    .filter(|index| (start..playback.queue.len()).contains(index))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(QueueEdit::Move { indices, before })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;
    use std::sync::Arc;

    fn playback() -> PlaybackSnapshot {
        let duplicate = Arc::new(Track::default());
        PlaybackSnapshot {
            queue: Arc::new(vec![duplicate; 4]),
            queue_ids: Arc::new(vec![4, 8, 12, 16]),
            queue_revision: 5,
            index: Some(0),
            ..Default::default()
        }
    }

    #[test]
    fn move_uses_occurrences_and_the_insertion_edge_for_both_directions() {
        let snapshot = playback();
        let drag = QueueDrag::new(&snapshot, &[2], "Song");
        assert_eq!(drag.ids(), [12]);
        assert_eq!(drag.row_destination(5, 1), Some(1));
        assert_eq!(drag.row_destination(5, 3), Some(4));
        assert_eq!(drag.row_destination(5, 2), None);
        assert!(matches!(
            drag.edit(&snapshot, 4),
            Some(QueueEdit::Move { indices, before: 4 }) if indices == [2]
        ));
    }

    #[test]
    fn rejects_a_stale_revision_and_tracks_that_have_started_playing() {
        let mut snapshot = playback();
        let drag = QueueDrag::new(&snapshot, &[1, 3], "Song");
        assert_eq!(drag.row_destination(6, 2), None);
        assert!(drag.edit(&snapshot, 0).is_none());
        assert!(drag.edit(&snapshot, 3).is_some());
        snapshot.index = Some(1);
        assert!(drag.edit(&snapshot, 3).is_none());
        snapshot.index = Some(0);
        snapshot.queue_revision += 1;
        assert!(drag.edit(&snapshot, 3).is_none());
    }
}
