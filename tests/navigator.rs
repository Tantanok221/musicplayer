use sonora::{
    browser::{BrowserSelection, Facet},
    catalog::Catalog,
    model::*,
};

fn count(values: &[Facet], name: &str) -> usize {
    values.iter().find(|item| item.label == name).unwrap().count
}

#[test]
fn navigator_separates_performers_from_album_artists_and_counts_releases() {
    let mut db = Catalog::memory().unwrap();
    let mut tracks = Vec::new();
    for (id, album, artist, album_artist, year, genres) in [
        ("a1", "a", "Lead & Guest", "Lead", Some(2024), "Rock · R&B"),
        ("a2", "a", "Lead", "Lead", Some(2024), "Rock"),
        ("b1", "b", "Guest", "Other", Some(2025), "Pop"),
        ("c1", "c", "Joanne", "Joanne", None, "Rock"),
        ("d1", "d", "Alias", "Host ＆ Partner", Some(2025), "R&B"),
    ] {
        tracks.push(Track {
            id: id.into(),
            album_id: album.into(),
            album: album.into(),
            title: id.into(),
            artist: artist.into(),
            album_artist: album_artist.into(),
            year,
            genre: genres.into(),
            artists: if id == "d1" {
                vec![NamedId {
                    id: "ann-id".into(),
                    name: "Ann".into(),
                }]
            } else {
                vec![]
            },
            ..Default::default()
        });
    }
    db.put_tracks(&tracks, 1).unwrap();
    let view = db.view(&LibraryQuery::default()).unwrap();
    assert_eq!(count(&view.navigator.artists, "Guest"), 2);
    assert_eq!(count(&view.navigator.artists, "Lead"), 1);
    assert!(
        !view
            .navigator
            .album_artists
            .iter()
            .any(|item| item.label == "Guest")
    );
    assert_eq!(count(&view.navigator.album_artists, "Host"), 1);
    assert_eq!(count(&view.navigator.album_artists, "Partner"), 1);
    assert_eq!(count(&view.navigator.genres, "Rock"), 2);
    assert_eq!(count(&view.navigator.genres, "R&B"), 2);
    assert_eq!(
        view.navigator
            .years
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["2025", "2024"]
    );
    assert_eq!(count(&view.navigator.years, "2024"), 1);

    for (performer, expected) in [("Guest", vec!["a1", "b1"]), ("Ann", vec!["d1"])] {
        let query = LibraryQuery {
            performer: performer.into(),
            ..Default::default()
        };
        let filtered = db.view(&query).unwrap();
        let mut ids = filtered
            .tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, expected);
        assert_eq!(
            filtered.navigator.artists.len(),
            view.navigator.artists.len(),
            "Navigation choices remain available after filtering"
        );
        assert_eq!(filtered.navigator.years.len(), 2);
    }
    let album_artist = db
        .view(&LibraryQuery {
            selection: BrowserSelection {
                artists: vec!["Other".into()],
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    assert_eq!(album_artist.track_count, 1);
    assert_eq!(album_artist.tracks[0].id, "b1");
    let genre = db
        .view(&LibraryQuery {
            genre: "R&B".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(genre.track_count, 2);
    let year = db
        .view(&LibraryQuery {
            year: Some(2025),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(year.track_count, 2);

    db.put_tracks(
        &[Track {
            id: "e1".into(),
            album_id: "e".into(),
            album: "e".into(),
            artist: "Ann".into(),
            album_artist: "Ann".into(),
            genre: "Jazz".into(),
            year: Some(2026),
            ..Default::default()
        }],
        1,
    )
    .unwrap();
    let updated = db.view(&LibraryQuery::default()).unwrap();
    assert_eq!(updated.navigator.years[0].label, "2026");
    assert_eq!(count(&updated.navigator.artists, "Ann"), 2);
    assert_eq!(count(&updated.navigator.genres, "Jazz"), 1);
}
