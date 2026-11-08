use sonora::{browser::*, catalog::Catalog, model::*};

#[test]
fn cover_jump_starts_a_row_at_the_matching_album_without_losing_earlier_albums() {
    for count in [0_usize, 1, 17, 1455] {
        for columns in [1, 3, 7] {
            for jump in [None, Some(0), Some(5), Some(count.saturating_sub(1))] {
                let rows = CoverRows::new(count, columns, jump);
                let albums = (0..rows.row_count())
                    .flat_map(|row| rows.albums_in_row(row))
                    .collect::<Vec<_>>();
                assert_eq!(albums, (0..count).collect::<Vec<_>>());
                for index in 0..count {
                    let range = rows.albums_in_row(rows.row_for_album(index));
                    assert!(range.contains(&index));
                    assert!(range.len() <= columns);
                }
                if let Some(jump) = jump.filter(|jump| *jump < count) {
                    assert_eq!(rows.albums_in_row(rows.row_for_album(jump)).start, jump);
                }
            }
        }
    }
    // The first C album falls partway through the old B row.
    let rows = CoverRows::new(20, 7, Some(10));
    assert_eq!(rows.albums_in_row(rows.row_for_album(10)), 10..17);
    assert_eq!(rows.albums_in_row(1), 7..10);
}

#[test]
fn metadata_links_filter_the_library_by_individual_genre_or_year() {
    let mut db = Catalog::memory().unwrap();
    let mut tracks = Vec::new();
    for (id, year, genres) in [
        ("rock-new", Some(2024), vec!["Rock", "Alternative"]),
        ("rock-old", Some(2023), vec!["Rock"]),
        ("pop-new", Some(2024), vec!["Pop"]),
        ("no-year", None, vec!["Alternative"]),
        ("r-and-b", Some(2025), vec!["R&B"]),
    ] {
        tracks.push(Track {
            id: id.into(),
            album_id: id.into(),
            album: id.into(),
            title: id.into(),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            year,
            genres: genres
                .into_iter()
                .map(|name| Genre { name: name.into() })
                .collect(),
            ..Default::default()
        });
    }
    db.put_tracks(&tracks, 1).unwrap();
    let cases = [
        (ExplorerFilter::Year(2024), vec!["pop-new", "rock-new"]),
        (
            ExplorerFilter::Genre("Alternative".into()),
            vec!["no-year", "rock-new"],
        ),
        (ExplorerFilter::Genre("R&B".into()), vec!["r-and-b"]),
    ];
    for (filter, expected) in cases {
        let query = filter.query();
        assert!(query.text.is_empty() && query.artist.is_empty() && !query.favorites);
        assert!(query.selection.artists.is_empty() && query.selection.albums.is_empty());
        match filter {
            ExplorerFilter::Year(year) => {
                assert_eq!(query.year, Some(year));
                assert!(query.selection.genres.is_empty());
            }
            ExplorerFilter::Genre(genre) => {
                assert_eq!(query.selection.genres, [genre]);
                assert!(query.year.is_none());
            }
            ExplorerFilter::RecordLabel(label) => {
                assert_eq!(query.record_label, label.trim());
                assert!(query.year.is_none() && query.selection.is_empty());
            }
        }
        let view = db.view(&query).unwrap();
        let mut ids = view
            .tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, expected);
    }
}
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
    assert_eq!(old.layout.explorer_view, MainView::AlbumCovers);
    assert!(old.layout.show_information);
    assert_eq!(old.layout.navigator_mode, NavigatorMode::AlbumArtist);
    assert!(old.layout.show_navigator && !old.layout.navigator_auto_hide);
    let db = Catalog::memory().unwrap();
    let mut prefs = old;
    prefs.layout.navigator_auto_hide = true;
    prefs.layout.explorer_view = MainView::Tracks;
    prefs.layout.navigator_mode = NavigatorMode::Genre;
    prefs.layout.left_width = 250.;
    prefs.layout.info_source = InfoSource::Playing;
    prefs.layout.info_tab = InfoTab::Lyrics;
    db.set_state("preferences", &prefs).unwrap();
    let saved: Preferences = db.state("preferences").unwrap();
    assert_eq!(saved.layout.explorer_view, MainView::Tracks);
    assert!(saved.layout.navigator_auto_hide);
    assert_eq!(saved.layout.navigator_mode, NavigatorMode::Genre);
    assert_eq!(saved.layout.left_width, 250.);
    assert_eq!(saved.layout.info_source, InfoSource::Playing);
    assert_eq!(saved.layout.info_tab, InfoTab::Lyrics);
    let legacy: LayoutPreferences = serde_json::from_str(
        r#"{"music_view":"Tracks","music_browser":false,"explorer_view":"AlbumsAndTracks","explorer_browser":true}"#,
    )
    .unwrap();
    assert_eq!(legacy.explorer_view, MainView::AlbumsAndTracks);
    assert!(legacy.explorer_browser);
    let saved = serde_json::to_value(legacy).unwrap();
    assert!(saved.get("music_view").is_none());
    assert!(saved.get("music_browser").is_none());
}
#[test]
fn covers_reflow_within_the_actual_pane_width() {
    for width in [160., 260., 300., 400., 520., 700., 996., 1400.] {
        let (columns, side) = sonora::browser::cover_layout(width);
        assert!(columns >= 1);
        assert!((0. ..=150.).contains(&side));
        let occupied = 24. + columns as f32 * side + (columns - 1) as f32 * 16.;
        assert!(
            occupied <= width + 0.01,
            "Covers must fit the pane at {width}px"
        );
    }
    assert_eq!(sonora::browser::cover_layout(160.).0, 1);
    assert!(sonora::browser::cover_layout(996.).0 > sonora::browser::cover_layout(700.).0);
}
#[test]
fn compact_windows_budget_space_for_both_side_panels_and_library() {
    for width in [680., 720., 800., 900., 1080., 1440.] {
        for left in [None, Some(216.), Some(380.)] {
            for right in [None, Some(300.), Some(500.)] {
                let (nav, info) = panel_widths(width, left, right);
                assert!(nav + info + 320. + 4. <= width + 0.01);
                assert_eq!(nav == 0., left.is_none());
                assert_eq!(info == 0., right.is_none());
                if left.is_some() {
                    assert!((140. ..=380.).contains(&nav));
                }
                if right.is_some() {
                    assert!((180. ..=500.).contains(&info));
                }
            }
        }
    }
    assert_eq!(panel_widths(1440., Some(216.), Some(300.)), (216., 300.));
}
