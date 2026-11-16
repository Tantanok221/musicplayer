use super::{ARTIST, Fixture, SECOND_ARTIST};
use sonora::model::{Album, NamedId, Preferences, Track};

impl Fixture {
    pub(crate) fn seed(&mut self) {
        self.catalog
            .put_tracks(
                &[
                    Track {
                        id: "track-1".into(),
                        title: "Fixture song".into(),
                        album_id: "album-1".into(),
                        album: "Fixture album".into(),
                        artist: ARTIST.into(),
                        album_artist: ARTIST.into(),
                        artist_id: "artist-1".into(),
                        duration: 120.,
                        starred: Some("2026-01-01T00:00:00Z".into()),
                        user_rating: Some(5),
                        ..Default::default()
                    },
                    Track {
                        id: "track-2".into(),
                        title: "Second song".into(),
                        album_id: "album-2".into(),
                        album: "Second release".into(),
                        artist: ARTIST.into(),
                        album_artist: ARTIST.into(),
                        artist_id: "artist-1".into(),
                        duration: 120.,
                        user_rating: Some(1),
                        ..Default::default()
                    },
                    Track {
                        id: "track-3".into(),
                        title: "Another song".into(),
                        album_id: "album-3".into(),
                        album: "Another album".into(),
                        artist: SECOND_ARTIST.into(),
                        album_artist: SECOND_ARTIST.into(),
                        artist_id: "artist-2".into(),
                        duration: 120.,
                        ..Default::default()
                    },
                ],
                1,
            )
            .unwrap();
        self.catalog
            .put_albums(
                &[
                    Album {
                        id: "album-1".into(),
                        name: "Fixture album".into(),
                        artist: ARTIST.into(),
                        artist_id: "artist-1".into(),
                        user_rating: Some(2),
                        song_count: 1,
                        release_types: vec!["album".into()],
                        ..Default::default()
                    },
                    Album {
                        id: "album-2".into(),
                        name: "Second release".into(),
                        artist: ARTIST.into(),
                        artist_id: "artist-1".into(),
                        starred: Some("2026-01-01T00:00:00Z".into()),
                        user_rating: Some(4),
                        song_count: 1,
                        release_types: vec!["album".into()],
                        ..Default::default()
                    },
                    Album {
                        id: "album-3".into(),
                        name: "Another album".into(),
                        artist: SECOND_ARTIST.into(),
                        artist_id: "artist-2".into(),
                        user_rating: Some(3),
                        song_count: 1,
                        release_types: vec!["album".into()],
                        ..Default::default()
                    },
                ],
                1,
            )
            .unwrap();
        let mut prefs = Preferences::default();
        // Reproduce creation from a middle tab, with tabs to its right.
        prefs.tabs.active = 1;
        prefs.layout.show_spectrum = false;
        self.catalog.set_state("preferences", &prefs).unwrap();
    }

    pub(crate) fn seed_explorer(&mut self) {
        for (id, track_id, genre, year, label) in [
            ("album-1", "track-1", "Rock", 2024, "Label A"),
            ("album-2", "track-2", "Rock", 2023, "Label B"),
            ("album-3", "track-3", "Pop", 2024, "Label A"),
        ] {
            let mut album = (*self.catalog.album(id).unwrap().unwrap()).clone();
            album.genre = genre.into();
            album.year = Some(year);
            album.record_labels = vec![NamedId {
                id: label.into(),
                name: label.into(),
            }];
            self.catalog.put_albums(&[album], 1).unwrap();
            let mut track = (*self.catalog.by_ids(&[track_id.into()]).unwrap()[0]).clone();
            track.genre = genre.into();
            track.year = Some(year);
            self.catalog.put_tracks(&[track], 1).unwrap();
        }
    }
}
