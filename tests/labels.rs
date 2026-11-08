use sonora::{browser::ExplorerFilter, catalog::Catalog, model::*};

fn label(name: &str) -> NamedId {
    NamedId {
        name: name.into(),
        ..Default::default()
    }
}
fn album(id: &str, names: &[&str], genre: &str, year: i32) -> Album {
    Album {
        id: id.into(),
        name: "Shared album title".into(),
        artist: "Artist".into(),
        genre: genre.into(),
        year: Some(year),
        record_labels: names.iter().map(|name| label(name)).collect(),
        ..Default::default()
    }
}
fn tracks(album: &Album) -> Vec<Track> {
    (1..=2)
        .map(|number| Track {
            id: format!("{}-{number}", album.id),
            album_id: album.id.clone(),
            album: album.name.clone(),
            title: format!("Track {number}"),
            artist: album.artist.clone(),
            album_artist: album.artist.clone(),
            genre: album.genre.clone(),
            year: album.year,
            disc_number: Some(1),
            track: Some(number),
            ..Default::default()
        })
        .collect()
}
fn library() -> Catalog {
    let mut db = Catalog::memory().unwrap();
    let albums = [
        album("a", &[" Warp "], "Electronic", 1995),
        album("b", &["WARP"], "Jazz", 2024),
        album("c", &["Warp Records"], "Rock", 2024),
        album("d", &["R&B, Inc."], "Soul", 2024),
        album("e", &["Warp", "Second Label"], "Electronic", 2020),
        album("missing", &[], "Unknown", 2024),
    ];
    db.put_tracks(&albums.iter().flat_map(tracks).collect::<Vec<_>>(), 1)
        .unwrap();
    db.put_albums(&albums, 1).unwrap();
    db
}
fn ids(db: &Catalog, query: &LibraryQuery) -> Vec<String> {
    let mut ids = db.matching_ids(query).unwrap();
    ids.sort();
    ids
}

#[test]
fn label_links_filter_whole_albums_by_exact_normalized_label_names() {
    let db = library();
    for name in ["Warp", "warp", " WARP ", "Ｗａｒｐ"] {
        let query = ExplorerFilter::RecordLabel(name.into()).query();
        assert!(query.text.is_empty() && query.artist.is_empty() && query.performer.is_empty());
        assert!(query.genre.is_empty() && query.year.is_none() && !query.favorites);
        assert!(query.selection.is_empty());
        assert_eq!(ids(&db, &query), ["a-1", "a-2", "b-1", "b-2", "e-1", "e-2"]);
        let view = db.view(&query).unwrap();
        assert_eq!(view.albums.len(), 3, "same-titled editions remain separate");
        assert_eq!(view.track_count, 6);
        assert_eq!(db.album_tracks_matching(&query, "a").unwrap().len(), 2);
        assert!(db.album_tracks_matching(&query, "c").unwrap().is_empty());
        assert_eq!(
            view.browser
                .genres
                .iter()
                .map(|facet| facet.id.as_str())
                .collect::<Vec<_>>(),
            ["Electronic", "Jazz"]
        );
    }
    assert_eq!(
        ids(
            &db,
            &ExplorerFilter::RecordLabel("Second Label".into()).query()
        ),
        ["e-1", "e-2"]
    );
    assert_eq!(
        ids(
            &db,
            &ExplorerFilter::RecordLabel("R&B, Inc.".into()).query()
        ),
        ["d-1", "d-2"]
    );
    for name in ["R&B", "Warp Rec", "' OR 1=1 --"] {
        assert!(ids(&db, &ExplorerFilter::RecordLabel(name.into()).query()).is_empty());
    }
}

#[test]
fn label_filter_composes_with_existing_filters_and_clears_independently() {
    let db = library();
    let mut query = ExplorerFilter::RecordLabel("Warp".into()).query();
    query.selection.genres = vec!["Jazz".into()];
    assert_eq!(ids(&db, &query), ["b-1", "b-2"]);
    query.selection.genres.clear();
    query.year = Some(1995);
    assert_eq!(ids(&db, &query), ["a-1", "a-2"]);
    query.record_label.clear();
    assert_eq!(
        ids(&db, &query),
        ["a-1", "a-2"],
        "clearing the label keeps the year filter"
    );
    query.year = None;
    assert_eq!(ids(&db, &query).len(), 12);
    assert_eq!(ids(&db, &ExplorerFilter::Year(2024).query()).len(), 8);
    assert_eq!(
        ids(&db, &ExplorerFilter::Genre("Soul".into()).query()),
        ["d-1", "d-2"]
    );
}

#[test]
fn record_label_display_omits_blanks_and_duplicates_without_splitting_names() {
    let a = album(
        "a",
        &[
            " Warp ",
            "WARP",
            "Ｗａｒｐ",
            "",
            "  ",
            "R&B, Inc.",
            "Label A; Label B",
        ],
        "",
        2024,
    );
    assert_eq!(
        a.record_label_names(),
        ["Warp", "R&B, Inc.", "Label A; Label B"]
    );
    assert!(Album::default().record_label_names().is_empty());
    let old: Album = serde_json::from_str(r#"{"id":"old","name":"Album"}"#).unwrap();
    assert!(old.record_label_names().is_empty());
    let old_query: LibraryQuery = serde_json::from_str("{}").unwrap();
    assert!(old_query.record_label.is_empty());
}

#[test]
fn label_metadata_updates_and_partial_syncs_preserve_correct_cached_results() {
    let mut db = library();
    let query = ExplorerFilter::RecordLabel("Warp".into()).query();
    let updated = album("a", &["New Label"], "Electronic", 1995);
    db.put_albums(std::slice::from_ref(&updated), 2).unwrap();
    assert_eq!(ids(&db, &query), ["b-1", "b-2", "e-1", "e-2"]);
    db.put_tracks(&tracks(&updated), 2).unwrap();
    assert_eq!(
        ids(&db, &query),
        ["b-1", "b-2", "e-1", "e-2"],
        "incomplete refresh keeps other albums and their label metadata"
    );
    assert_eq!(
        ids(
            &db,
            &ExplorerFilter::RecordLabel("New Label".into()).query()
        ),
        ["a-1", "a-2"]
    );
    assert_eq!(db.count().unwrap(), 12);
}
