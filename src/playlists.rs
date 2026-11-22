use crate::{
    model::{Playlist, Track, normalize},
    service::Mutation,
};
use std::sync::Arc;

pub fn is_smart(playlist: &Playlist, username: Option<&str>) -> bool {
    playlist.readonly
        && (playlist.valid_until.is_some() || username.is_some_and(|user| playlist.owner == user))
}

pub fn refresh_due(
    playlist: &Playlist,
    username: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    is_smart(playlist, username)
        && playlist
            .valid_until
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .is_some_and(|until| until <= now)
}

#[derive(Clone, Default)]
pub struct Playlists {
    summaries: Vec<Playlist>,
    current: Option<Playlist>,
    search: String,
}

impl Playlists {
    pub fn open(&mut self, summary: Playlist) -> String {
        let id = summary.id.clone();
        self.current = Some(summary);
        id
    }

    pub fn close(&mut self) {
        self.current = None;
    }

    pub fn summaries_loaded(&mut self, summaries: Vec<Playlist>) {
        if let Some(current) = &mut self.current {
            if let Some(summary) = summaries.iter().find(|p| p.id == current.id) {
                current.name.clone_from(&summary.name);
                current.song_count = summary.song_count;
                current.readonly = summary.readonly;
                current.owner.clone_from(&summary.owner);
                current.valid_until.clone_from(&summary.valid_until);
            } else {
                self.current = None;
            }
        }
        self.summaries = summaries
            .into_iter()
            .map(|mut playlist| {
                playlist.entry = Arc::default();
                playlist
            })
            .collect();
    }

    pub fn detail_loaded(&mut self, playlist: Playlist) {
        if self.current.as_ref().is_some_and(|p| p.id == playlist.id) {
            self.current = Some(playlist);
        }
    }

    pub fn set_search(&mut self, search: String) {
        self.search = search;
    }
    pub fn search(&self) -> &str {
        &self.search
    }
    pub fn current(&self) -> Option<&Playlist> {
        self.current.as_ref()
    }
    pub fn summaries(&self) -> &[Playlist] {
        &self.summaries
    }
    pub fn name_of(&self, id: &str) -> Option<&str> {
        self.summaries
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.as_str())
    }

    pub fn visible_entries(&self) -> Vec<(usize, Arc<Track>)> {
        self.current
            .iter()
            .flat_map(|p| p.entry.iter().enumerate())
            .filter(|(_, track)| {
                normalize(&format!(
                    "{} {} {}",
                    track.title,
                    track.artist_label(),
                    track.album
                ))
                .contains(&normalize(&self.search))
            })
            .map(|(i, track)| (i, track.clone()))
            .collect()
    }

    pub fn editable(&self, username: Option<&str>) -> bool {
        self.current
            .as_ref()
            .is_some_and(|p| Self::can_edit(p, username))
    }

    pub fn can_edit(playlist: &Playlist, username: Option<&str>) -> bool {
        !playlist.readonly
            && (playlist.owner.is_empty() || Some(playlist.owner.as_str()) == username)
    }

    pub fn append_queue(&self, ids: Vec<String>) -> Option<Mutation> {
        self.current
            .as_ref()
            .map(|p| Mutation::AddToPlaylist(p.id.clone(), ids))
    }

    pub fn update_track(&mut self, track: Arc<Track>) {
        if let Some(playlist) = &mut self.current {
            for value in Arc::make_mut(&mut playlist.entry) {
                if value.id == track.id {
                    *value = track.clone();
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct PlaylistEntry {
    id: String,
    index: usize,
    entries: Arc<Vec<Arc<Track>>>,
}

impl PlaylistEntry {
    pub fn drop_target(&self, target: &Self) -> Option<std::cmp::Ordering> {
        (self.id == target.id && self.index != target.index).then(|| self.index.cmp(&target.index))
    }

    pub fn drop_on(&self, current: &Playlist, target: &Self) -> Option<Mutation> {
        (self.id == target.id)
            .then(|| self.move_to(current, target.index))
            .flatten()
    }

    pub fn new(playlist: &Playlist, index: usize) -> Self {
        Self {
            id: playlist.id.clone(),
            index,
            entries: playlist.entry.clone(),
        }
    }

    pub fn mutation(&self, current: Option<&Playlist>) -> Option<Mutation> {
        let current = current?;
        (!current.readonly && self.matches(current))
            .then(|| Mutation::RemoveFromPlaylist(self.id.clone(), self.index))
    }

    fn matches(&self, current: &Playlist) -> bool {
        current.id == self.id
            && self.index < current.entry.len()
            && current
                .track_ids()
                .eq(self.entries.iter().map(|track| track.id.as_str()))
    }

    pub fn move_to(&self, current: &Playlist, destination: usize) -> Option<Mutation> {
        if destination == self.index
            || !current.can_reorder()
            || !self.matches(current)
            || destination >= current.entry.len()
        {
            return None;
        }
        let original: Vec<_> = current.track_ids().map(str::to_owned).collect();
        let mut reordered = original.clone();
        let song = reordered.remove(self.index);
        reordered.insert(destination, song);
        (original != reordered).then(|| Mutation::ReorderPlaylist {
            id: self.id.clone(),
            original,
            reordered,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_requires_readonly_and_expiry_or_signed_in_owner() {
        for readonly in [false, true] {
            for until in [None, Some("2999-01-01T00:00:00Z".to_string())] {
                for user in [None, Some("owner"), Some("other")] {
                    let value = Playlist {
                        owner: "owner".into(),
                        readonly,
                        valid_until: until.clone(),
                        ..Default::default()
                    };
                    assert_eq!(
                        is_smart(&value, user),
                        readonly && (until.is_some() || user == Some("owner"))
                    );
                }
            }
        }
        assert!(!is_smart(
            &Playlist {
                readonly: true,
                ..Default::default()
            },
            None
        ));
    }

    #[test]
    fn refresh_requires_smart_and_parseable_expired_timestamp() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut value = Playlist {
            readonly: true,
            owner: "owner".into(),
            ..Default::default()
        };
        for until in [None, Some("invalid"), Some("2027-01-01T00:00:00Z")] {
            value.valid_until = until.map(str::to_string);
            assert!(!refresh_due(&value, Some("owner"), now));
        }
        value.valid_until = Some("2026-01-01T01:00:00+01:00".into());
        assert!(refresh_due(&value, Some("owner"), now));
        value.readonly = false;
        assert!(!refresh_due(&value, Some("owner"), now));
    }

    #[test]
    fn open_close_and_stale_details() {
        let mut state = Playlists::default();
        assert!(state.current().is_none());
        assert!(state.visible_entries().is_empty());
        assert!(state.append_queue(vec![]).is_none());
        assert_eq!(state.open(playlist(&[])), "playlist");
        let mut stale = playlist(&["a"]);
        stale.id = "other".into();
        state.detail_loaded(stale);
        assert!(state.current().unwrap().entry.is_empty());
        state.detail_loaded(playlist(&["a"]));
        assert_eq!(state.current().unwrap().entry.len(), 1);
        state.set_search("saved".into());
        state.close();
        state.detail_loaded(playlist(&["b"]));
        assert!(state.current().is_none());
        assert_eq!(state.search(), "saved");
    }

    #[test]
    fn summaries_refresh_metadata_strip_entries_and_close_missing() {
        let mut state = Playlists::default();
        let mut current = playlist(&["a"]);
        current.duration = 12.;
        state.open(current);
        let mut summary = playlist(&["b", "c"]);
        summary.name = "Renamed".into();
        summary.duration = 99.;
        state.summaries_loaded(vec![summary]);
        let current = state.current().unwrap();
        assert_eq!(current.name, "Renamed");
        assert_eq!(current.song_count, 2);
        assert_eq!(current.duration, 12.);
        assert_eq!(current.entry[0].id, "a");
        assert!(state.summaries()[0].entry.is_empty());
        assert_eq!(state.name_of("playlist"), Some("Renamed"));
        assert_eq!(state.name_of("missing"), None);
        state.summaries_loaded(vec![]);
        assert!(state.current().is_none());
        assert!(state.summaries().is_empty());
    }

    #[test]
    fn search_keeps_original_indices_and_matches_metadata() {
        let mut state = Playlists::default();
        let mut value = playlist(&["a", "b", "a"]);
        Arc::make_mut(&mut value.entry)[1] = Arc::new(Track {
            title: "Café".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            ..Default::default()
        });
        state.open(value);
        assert_eq!(state.visible_entries().len(), 3);
        for search in ["CAFÉ", "Artist", "Album"] {
            state.set_search(search.into());
            assert_eq!(
                state
                    .visible_entries()
                    .iter()
                    .map(|(i, _)| *i)
                    .collect::<Vec<_>>(),
                vec![1]
            );
        }
        state.set_search("missing".into());
        assert!(state.visible_entries().is_empty());
    }

    #[test]
    fn editable_requires_writable_playlist_and_matching_owner() {
        let mut state = Playlists::default();
        assert!(!state.editable(None));
        let mut value = playlist(&[]);
        value.readonly = true;
        state.open(value.clone());
        assert!(!state.editable(None));
        value.owner = "owner".into();
        state.open(value.clone());
        assert!(!state.editable(None));
        assert!(!state.editable(Some("other")));
        assert!(!state.editable(Some("owner")));
        value.readonly = false;
        state.open(value);
        assert!(state.editable(Some("owner")));
    }

    #[test]
    fn append_keeps_empty_queue_and_duplicate_ids() {
        let mut state = Playlists::default();
        state.open(playlist(&["existing"]));
        for ids in [vec![], vec!["a".into(), "a".into(), "b".into()]] {
            assert_eq!(
                state.append_queue(ids.clone()),
                Some(Mutation::AddToPlaylist("playlist".into(), ids))
            );
        }
    }

    #[test]
    fn track_update_replaces_all_occurrences() {
        let mut state = Playlists::default();
        let updated = Arc::new(Track {
            id: "a".into(),
            title: "New".into(),
            ..Default::default()
        });
        state.update_track(updated.clone());
        state.open(playlist(&["a", "b", "a"]));
        state.update_track(updated);
        let entries = &state.current().unwrap().entry;
        assert_eq!(entries[0].title, "New");
        assert_eq!(entries[1].title, "");
        assert_eq!(entries[2].title, "New");
    }

    fn playlist(ids: &[&str]) -> Playlist {
        Playlist {
            id: "playlist".into(),
            song_count: ids.len() as u32,
            entry: Arc::new(
                ids.iter()
                    .map(|id| {
                        Arc::new(Track {
                            id: (*id).into(),
                            ..Default::default()
                        })
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }

    #[::core::prelude::v1::test]
    fn removal_keeps_duplicate_entry_indices_and_rejects_stale_or_empty_playlists() {
        let original = playlist(&["a", "b", "a"]);
        let removal = PlaylistEntry::new(&original, 2);
        assert_eq!(
            removal.mutation(Some(&original)),
            Some(Mutation::RemoveFromPlaylist("playlist".into(), 2))
        );
        assert!(removal.mutation(None).is_none());
        assert!(removal.mutation(Some(&playlist(&[]))).is_none());
        assert!(
            removal
                .mutation(Some(&playlist(&["a", "a", "b"])))
                .is_none()
        );
        let mut other = original.clone();
        other.readonly = true;
        assert!(removal.mutation(Some(&other)).is_none());
        other.id = "other".into();
        assert!(removal.mutation(Some(&other)).is_none());
        assert!(
            PlaylistEntry::new(&original, 3)
                .mutation(Some(&original))
                .is_none()
        );
    }

    #[::core::prelude::v1::test]
    fn reorder_uses_full_indices_and_rejects_changed_incomplete_or_unchanged_order() {
        let original = playlist(&["a", "b", "a", "c"]);
        let source = PlaylistEntry::new(&original, 2);
        let expected = |ids: [&str; 4]| Mutation::ReorderPlaylist {
            id: "playlist".into(),
            original: ["a", "b", "a", "c"].map(String::from).into(),
            reordered: ids.map(String::from).into(),
        };
        assert_eq!(
            source.move_to(&original, 3),
            Some(expected(["a", "b", "c", "a"]))
        );
        assert_eq!(
            source.move_to(&original, 1),
            Some(expected(["a", "a", "b", "c"]))
        );
        assert!(source.move_to(&original, 2).is_none());
        assert!(source.move_to(&original, 4).is_none());
        assert!(
            source
                .move_to(&playlist(&["a", "c", "a", "b"]), 3)
                .is_none()
        );
        let mut incomplete = original;
        incomplete.song_count += 1;
        assert!(source.move_to(&incomplete, 3).is_none());
        incomplete.song_count -= 1;
        incomplete.readonly = true;
        assert!(source.move_to(&incomplete, 3).is_none());
        assert!(
            PlaylistEntry::new(&playlist(&["a"]), 0)
                .move_to(&playlist(&["a"]), 0)
                .is_none()
        );
        let duplicates = playlist(&["a", "a"]);
        assert!(
            PlaylistEntry::new(&duplicates, 0)
                .move_to(&duplicates, 1)
                .is_none()
        );
    }

    #[test]
    fn drag_direction_and_drop_target_keep_playlist_and_occurrence() {
        use std::cmp::Ordering;
        let current = playlist(&["a", "b"]);
        let first = PlaylistEntry::new(&current, 0);
        let last = PlaylistEntry::new(&current, 1);
        assert_eq!(first.drop_target(&last), Some(Ordering::Less));
        assert_eq!(last.drop_target(&first), Some(Ordering::Greater));
        assert_eq!(first.drop_target(&first), None);
        assert_eq!(first.drop_on(&current, &last), first.move_to(&current, 1));
        let mut other = current.clone();
        other.id = "other".into();
        let other = PlaylistEntry::new(&other, 1);
        assert_eq!(first.drop_target(&other), None);
        assert!(first.drop_on(&current, &other).is_none());
    }
}
