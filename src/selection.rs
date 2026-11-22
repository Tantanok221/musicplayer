//! Which songs are selected in a song list, for actions on several at once.
use crate::model::Track;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct TrackSelection {
    tracks: Vec<Arc<Track>>,
    anchor: Option<String>,
}

impl TrackSelection {
    /// A plain click: just this song.
    pub fn only(&mut self, track: Arc<Track>) {
        self.anchor = Some(track.id.clone());
        self.tracks = vec![track];
    }

    /// ⌘-click: adds or removes one song.
    pub fn toggle(&mut self, track: Arc<Track>) {
        self.anchor = Some(track.id.clone());
        match self.tracks.iter().position(|t| t.id == track.id) {
            Some(index) => {
                self.tracks.remove(index);
            }
            None => self.tracks.push(track),
        }
    }

    /// Shift-click: every song from the last clicked one to this one in
    /// `list`, or just adds it when either is not in `list`.
    pub fn extend(&mut self, track: Arc<Track>, list: &[Arc<Track>]) {
        let find = |id: &str| list.iter().position(|t| t.id == id);
        match (self.anchor.as_deref().and_then(find), find(&track.id)) {
            (Some(from), Some(to)) => {
                self.tracks = list[from.min(to)..=from.max(to)].to_vec();
            }
            _ if self.contains(&track.id) => {}
            _ => {
                if self.anchor.is_none() {
                    self.anchor = Some(track.id.clone());
                }
                self.tracks.push(track);
            }
        }
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.anchor = None;
    }

    pub fn contains(&self, id: &str) -> bool {
        self.tracks.iter().any(|t| t.id == id)
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// What a right-click on `track` acts on: the selection when it holds
    /// that song among others, otherwise the song alone.
    pub fn acting_on(&self, track: &Arc<Track>) -> Vec<Arc<Track>> {
        if self.len() > 1 && self.contains(&track.id) {
            self.tracks.clone()
        } else {
            vec![track.clone()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn songs() -> Vec<Arc<Track>> {
        ["a", "b", "c", "d"]
            .iter()
            .map(|id| {
                Arc::new(Track {
                    id: id.to_string(),
                    ..Default::default()
                })
            })
            .collect()
    }

    fn ids(tracks: &[Arc<Track>]) -> Vec<&str> {
        tracks.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn shift_selects_a_range_in_either_direction() {
        let list = songs();
        let mut selection = TrackSelection::default();
        selection.only(list[2].clone());
        selection.extend(list[0].clone(), &list);
        assert_eq!(ids(&selection.acting_on(&list[1])), ["a", "b", "c"]);
        selection.extend(list[3].clone(), &list);
        assert_eq!(ids(&selection.acting_on(&list[3])), ["c", "d"]);
    }

    #[test]
    fn command_toggles_and_a_plain_click_resets() {
        let list = songs();
        let mut selection = TrackSelection::default();
        selection.only(list[0].clone());
        selection.toggle(list[3].clone());
        assert_eq!(ids(&selection.acting_on(&list[0])), ["a", "d"]);
        selection.toggle(list[0].clone());
        assert_eq!(selection.len(), 1);
        selection.only(list[1].clone());
        assert_eq!(ids(&selection.acting_on(&list[1])), ["b"]);
    }

    #[test]
    fn a_song_outside_the_selection_acts_alone() {
        let list = songs();
        let mut selection = TrackSelection::default();
        selection.only(list[0].clone());
        selection.toggle(list[1].clone());
        assert_eq!(ids(&selection.acting_on(&list[2])), ["c"]);
    }

    #[test]
    fn shift_outside_the_list_adds_the_song() {
        let list = songs();
        let mut selection = TrackSelection::default();
        selection.only(list[0].clone());
        let other = Arc::new(Track {
            id: "x".into(),
            ..Default::default()
        });
        selection.extend(other.clone(), &list);
        assert_eq!(ids(&selection.acting_on(&other)), ["a", "x"]);
    }
}
