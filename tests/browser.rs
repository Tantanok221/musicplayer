use sonora::{browser::*, catalog::Catalog, model::*};
fn track(id: &str, album: &str, artist: &str, genre: &str, title: &str) -> Track {
    Track {
        id: id.into(),
        album_id: album.into(),
        album: album.into(),
        album_artist: artist.into(),
        artist: artist.into(),
        genre: genre.into(),
        title: title.into(),
        duration: 120.,
        ..Default::default()
    }
}
fn catalog() -> Catalog {
    let mut db = Catalog::memory().unwrap();
    db.put_tracks(
        &[
            track("a1", "A", "Alpha", "Ambient", "Quiet One"),
            track("a2", "A", "Alpha", "Ambient", "Quiet Two"),
            track("b1", "B", "Beta", "Jazz", "Blue One"),
            track("c1", "C", "Alpha", "Jazz", "Blue Two"),
            track("d1", "D", "Gamma", "Dark Ambient", "Dark One"),
        ],
        1,
    )
    .unwrap();
    db
}
#[test]
fn columns_are_narrowed_only_by_the_columns_to_their_left() {
    let db = catalog();
    let query = LibraryQuery {
        selection: BrowserSelection {
            genres: vec!["Jazz".into()],
            artists: vec!["Alpha".into()],
            albums: vec!["C".into()],
        },
        ..Default::default()
    };
    let view = db.view(&query).unwrap();
    assert_eq!(view.track_count, 1);
    assert_eq!(view.albums[0].id, "C");
    assert_eq!(
        view.browser.genres.len(),
        3,
        "Genre choices must not disappear after a downstream selection"
    );
    assert_eq!(
        view.browser
            .artists
            .iter()
            .map(|v| v.id.as_str())
            .collect::<Vec<_>>(),
        ["Alpha", "Beta"]
    );
    assert_eq!(
        view.browser
            .albums
            .iter()
            .map(|v| v.id.as_str())
            .collect::<Vec<_>>(),
        ["C"]
    );
    assert_eq!(
        view.browser
            .artists
            .iter()
            .find(|v| v.id == "Alpha")
            .unwrap()
            .count,
        1
    );
}
#[test]
fn multiple_values_union_within_a_column_and_intersect_across_columns() {
    let db = catalog();
    let mut query = LibraryQuery {
        selection: BrowserSelection {
            genres: vec!["Ambient".into(), "Jazz".into()],
            artists: vec!["Alpha".into()],
            albums: vec![],
        },
        ..Default::default()
    };
    assert_eq!(db.view(&query).unwrap().track_count, 3);
    query.selection.artists.push("Beta".into());
    assert_eq!(db.view(&query).unwrap().track_count, 4);
    query.selection.genres = vec!["Ambient".into()];
    assert_eq!(
        db.view(&query).unwrap().track_count,
        2,
        "Ambient must not match Dark Ambient"
    );
}
#[test]
fn group_tracks_respect_search_and_do_not_leak_other_album_tracks() {
    let db = catalog();
    let query = LibraryQuery {
        text: "Quiet One".into(),
        ..Default::default()
    };
    let tracks = db.album_tracks_matching(&query, "A").unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id, "a1");
    assert!(db.album_tracks_matching(&query, "B").unwrap().is_empty());
    let query = LibraryQuery {
        selection: BrowserSelection {
            genres: vec!["Jazz".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(db.matching_ids(&query).unwrap().len(), 2);
}
#[test]
fn layout_defaults_upgrade_old_preferences_and_survive_restart() {
    let old: Preferences = serde_json::from_str(r#"{"volume":0.4,"dark":true}"#).unwrap();
    assert_eq!(old.layout.music_view, MainView::AlbumsAndTracks);
    assert!(old.layout.show_information);
    let db = Catalog::memory().unwrap();
    let mut prefs = old;
    prefs.layout.music_view = MainView::Tracks;
    prefs.layout.left_width = 250.;
    prefs.layout.info_source = InfoSource::Playing;
    prefs.layout.info_tab = InfoTab::Lyrics;
    db.set_state("preferences", &prefs).unwrap();
    let saved: Preferences = db.state("preferences").unwrap();
    assert_eq!(saved.layout.music_view, MainView::Tracks);
    assert_eq!(saved.layout.left_width, 250.);
    assert_eq!(saved.layout.info_source, InfoSource::Playing);
    assert_eq!(saved.layout.info_tab, InfoTab::Lyrics);
}
