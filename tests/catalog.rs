use sonora::{catalog::Catalog, model::*};

fn sample(id: &str, title: &str) -> Track {
    Track {
        id: id.into(),
        title: title.into(),
        album_id: "album-a".into(),
        album: "Quiet Hours".into(),
        artist: "青葉市子".into(),
        duration: 180.,
        suffix: "flac".into(),
        year: Some(2024),
        ..Default::default()
    }
}

#[test]
fn album_playback_stays_in_album_and_track_playback_starts_at_selected_track() {
    let mut db = Catalog::memory().unwrap();
    let mut tracks = Vec::new();
    for (id, album, disc, number) in [
        ("other", "album-b", 1, 1),
        ("last", "album-a", 2, 2),
        ("first", "album-a", 1, 1),
        ("second", "album-a", 1, 2),
        ("third", "album-a", 2, 1),
        ("loose-a", "", 1, 1),
        ("loose-b", "", 1, 2),
    ] {
        let mut track = sample(id, id);
        track.album_id = album.into();
        track.disc_number = Some(disc);
        track.track = Some(number);
        tracks.push(track);
    }
    db.put_tracks(&tracks, 1).unwrap();
    let ids = |tracks: Vec<std::sync::Arc<Track>>| {
        tracks
            .into_iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(db.album_tracks("album-a").unwrap()),
        ["first", "second", "third", "last"]
    );
    assert_eq!(
        ids(db.album_queue_from("second").unwrap()),
        ["second", "third", "last"]
    );
    assert_eq!(ids(db.album_queue_from("last").unwrap()), ["last"]);
    assert_eq!(ids(db.album_queue_from("loose-a").unwrap()), ["loose-a"]);
    assert!(db.album_queue_from("deleted-track").is_err());
}

#[test]
fn interrupted_sync_keeps_previous_records_and_complete_sync_reconciles() {
    let mut db = Catalog::memory().unwrap();
    db.put_tracks(&[sample("a", "Old"), sample("b", "Keep")], 1)
        .unwrap();
    db.finish_sync(1).unwrap();
    db.put_tracks(&[sample("b", "Updated")], 2).unwrap();
    assert_eq!(
        db.count().unwrap(),
        2,
        "A failed sync must not remove old tracks"
    );
    db.put_tracks(&[sample("b", "Updated")], 2).unwrap();
    assert_eq!(db.count().unwrap(), 2, "Upserts must not duplicate tracks");
    db.finish_sync(2).unwrap();
    assert_eq!(db.count().unwrap(), 1);
    assert_eq!(db.by_ids(&["b".into()]).unwrap()[0].title, "Updated");
    assert!(db.by_ids(&["a".into()]).unwrap().is_empty());
}

#[test]
fn multilingual_substrings_normalization_and_literal_search() {
    let mut db = Catalog::memory().unwrap();
    db.put_tracks(
        &[
            sample("a", "夜の散歩道"),
            sample("b", "Ｓｏｆｔ Geometry"),
            sample("c", r#"Café "crème""#),
            sample("d", "100%_real"),
            sample("e", "꽃의 노래"),
            sample("f", "城市之光"),
        ],
        1,
    )
    .unwrap();
    for (text, id) in [
        ("散歩道", "a"),
        ("の散", "a"),
        ("soft", "b"),
        ("ＳＯＦＴ", "b"),
        ("Cafe\u{301}", "c"),
        ("\"crème\"", "c"),
        ("%_", "d"),
        ("노래", "e"),
        ("城市", "f"),
    ] {
        let query = LibraryQuery {
            text: text.into(),
            ..Default::default()
        };
        let tracks = db.tracks(&query, 0, 20).unwrap();
        assert!(
            tracks.iter().any(|t| t.id == id),
            "{text:?} should match {id}"
        );
    }
    assert!(
        db.tracks(
            &LibraryQuery {
                text: "\" OR *".into(),
                ..Default::default()
            },
            0,
            20
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn filters_pagination_order_and_saved_state_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite");
    {
        let mut db = Catalog::open(&path).unwrap();
        let mut tracks = test_tracks(1100);
        tracks[0].user_rating = Some(5);
        db.put_tracks(&tracks, 1).unwrap();
        let query = LibraryQuery {
            sort: Sort::Rating,
            descending: true,
            ..Default::default()
        };
        assert_eq!(db.tracks(&query, 0, 1).unwrap()[0].id, "test-0");
        let page_a = db.tracks(&query, 0, 500).unwrap();
        let page_b = db.tracks(&query, 500, 500).unwrap();
        assert_eq!(page_a.len(), 500);
        assert_eq!(page_b.len(), 500);
        assert!(page_a.iter().all(|a| page_b.iter().all(|b| a.id != b.id)));
        let favorites = LibraryQuery {
            favorites: true,
            ..Default::default()
        };
        assert!(
            db.tracks(&favorites, 0, 1000)
                .unwrap()
                .iter()
                .all(|t| t.starred.is_some())
        );
        let query = LibraryQuery {
            artist: "青葉市子".into(),
            year: Some(2016),
            ..Default::default()
        };
        assert!(
            db.tracks(&query, 0, 1000)
                .unwrap()
                .iter()
                .all(|t| t.artist == "青葉市子" && t.year == Some(2016))
        );
        let queue = SavedQueue {
            listen: None,
            ids: vec!["test-0".into(), "test-1".into()],
            index: Some(1),
            position: 42.,
        };
        db.set_state("queue", &queue).unwrap();
    }
    let db = Catalog::open(&path).unwrap();
    let queue: SavedQueue = db.state("queue").unwrap();
    assert_eq!(queue.position, 42.);
    assert_eq!(queue.index, Some(1));
    assert_eq!(db.by_ids(&queue.ids).unwrap().len(), 2);
}

#[test]
fn rejected_empty_id_does_not_partially_commit_batch() {
    let mut db = Catalog::memory().unwrap();
    assert!(
        db.put_tracks(&[sample("a", "Valid"), sample("", "Broken")], 1)
            .is_err()
    );
    assert_eq!(db.count().unwrap(), 0);
}

fn test_tracks(count: usize) -> Vec<Track> {
    (0..count)
        .map(|i| Track {
            id: format!("test-{i}"),
            title: format!("Track {i}"),
            album_id: format!("album-{}", i / 10),
            album: format!("Album {}", i / 10),
            artist: "青葉市子".into(),
            year: Some(2016),
            track: Some((i % 10 + 1) as u32),
            duration: 180.,
            suffix: "flac".into(),
            starred: (i % 17 == 0).then(|| "2026-01-01T00:00:00Z".into()),
            ..Default::default()
        })
        .collect()
}

#[test]
fn playlist_summaries_leave_offline_songs_in_the_catalog() {
    let mut catalog = Catalog::memory().unwrap();
    let playlist = Playlist {
        id: "large".into(),
        name: "Library".into(),
        song_count: 1000,
        entry: std::sync::Arc::new(
            (0..1000)
                .map(|i| std::sync::Arc::new(sample(&i.to_string(), "Song")))
                .collect(),
        ),
        ..Default::default()
    };
    catalog.save_playlists(&[playlist]).unwrap();
    let summaries = catalog.playlist_summaries().unwrap();
    assert_eq!(summaries[0].song_count, 1000);
    assert!(summaries[0].entry.is_empty());
    let mut full = catalog.playlist("large").unwrap().unwrap();
    assert_eq!(full.entry.len(), 1000);
    full.name = "Renamed".into();
    catalog.save_playlist(&full).unwrap();
    assert_eq!(catalog.playlist_summaries().unwrap()[0].name, "Renamed");
    assert_eq!(
        catalog.playlist("large").unwrap().unwrap().entry.len(),
        1000
    );
    assert!(catalog.playlist("missing").unwrap().is_none());
}

#[test]
fn summary_refresh_keeps_cached_songs_and_only_reconciles_complete_lists() {
    let mut catalog = Catalog::memory().unwrap();
    let existing = Playlist {
        id: "kept".into(),
        name: "Old name".into(),
        entry: std::sync::Arc::new(vec![std::sync::Arc::new(sample("song", "Offline song"))]),
        ..Default::default()
    };
    let missing = Playlist {
        id: "missing".into(),
        ..Default::default()
    };
    catalog.save_playlists(&[existing, missing]).unwrap();
    let summary = Playlist {
        id: "kept".into(),
        name: "New name".into(),
        song_count: 2,
        ..Default::default()
    };
    catalog
        .save_playlist_summaries(std::slice::from_ref(&summary), false)
        .unwrap();
    let cached = catalog.playlist("kept").unwrap().unwrap();
    assert_eq!(cached.name, "New name");
    assert_eq!(cached.song_count, 2);
    assert_eq!(cached.entry[0].title, "Offline song");
    assert!(catalog.playlist("missing").unwrap().is_some());
    catalog.save_playlist_summaries(&[summary], true).unwrap();
    assert!(catalog.playlist("missing").unwrap().is_none());
    assert_eq!(catalog.playlist("kept").unwrap().unwrap().entry.len(), 1);
}

#[test]
fn favorites_filter_combines_hearts_and_ratings() {
    let mut db = Catalog::memory().unwrap();
    let track = |id: &str, hearted: bool, rating: u8| Track {
        user_rating: (rating > 0).then_some(rating),
        starred: hearted.then(|| "2026-01-01T00:00:00Z".into()),
        ..sample(id, id)
    };
    db.put_tracks(
        &[
            track("hearted", true, 0),
            track("rated-4", false, 4),
            track("rated-2", false, 2),
            track("both", true, 5),
            track("neither", false, 0),
        ],
        1,
    )
    .unwrap();
    let ids = |favorite_filter: FavoritesFilter| {
        let mut ids = db
            .tracks(
                &LibraryQuery {
                    favorites: true,
                    favorite_filter,
                    ..Default::default()
                },
                0,
                10,
            )
            .unwrap()
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    };
    assert_eq!(ids(FavoritesFilter::default()), ["both", "hearted"]);
    let rated = FavoritesFilter {
        hearted: false,
        min_rating: 4,
        match_all: false,
    };
    assert_eq!(ids(rated.clone()), ["both", "rated-4"]);
    let either = FavoritesFilter {
        hearted: true,
        ..rated.clone()
    };
    assert_eq!(ids(either.clone()), ["both", "hearted", "rated-4"]);
    assert_eq!(either.summary(), "Hearted or 4+ stars");
    let every = FavoritesFilter {
        match_all: true,
        ..either
    };
    assert_eq!(ids(every.clone()), ["both"]);
    assert_eq!(every.summary(), "Hearted and 4+ stars");
    // With every rule off, hearts still apply rather than listing the whole library.
    let none = FavoritesFilter {
        hearted: false,
        min_rating: 0,
        match_all: false,
    };
    assert_eq!(ids(none), ["both", "hearted"]);
}
