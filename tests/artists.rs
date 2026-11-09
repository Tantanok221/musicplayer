use sonora::{artists::search_releases, catalog::Catalog, model::*};
use std::sync::Arc;

#[test]
fn artist_separators_trim_deduplicate_and_do_not_reuse_combined_ids() {
    assert_eq!(
        split_artist_names(" A & B, C; D ＆ E， F；a ; & "),
        ["A", "B", "C", "D", "E", "F"]
    );
    let credits = artist_credits(
        "A & B, C",
        &[
            NamedId {
                id: "a-id".into(),
                name: "A".into(),
            },
            NamedId {
                id: "pair-id".into(),
                name: "B; C".into(),
            },
        ],
        "combined-id",
    );
    assert_eq!(
        credits
            .iter()
            .map(|artist| (artist.name.as_str(), artist.id.as_str()))
            .collect::<Vec<_>>(),
        [("A", "a-id"), ("B", ""), ("C", "")]
    );
    let credits = artist_credits("A & B", &[], "combined-id");
    assert!(credits.iter().all(|artist| artist.id.is_empty()));
    assert_eq!(artist_credits("Solo", &[], "solo-id")[0].id, "solo-id");
}

#[test]
fn separated_artists_have_distinct_counts_filters_and_shared_releases() {
    let mut db = Catalog::memory().unwrap();
    for (album, artist, count) in [
        ("amp", "Ann & Bee", 2),
        ("comma", "Bee, Cee", 1),
        ("semi", "Ann; Dee", 1),
        ("repeat", "Ann & Ann", 1),
        ("solo", "Bee", 1),
        ("unrelated", "Joanne", 1),
        ("wide", "打打だいず ＆ Tanchiky", 1),
    ] {
        let tracks = (0..count)
            .map(|index| Track {
                id: format!("{album}-{index}"),
                album_id: album.into(),
                album: album.into(),
                artist: artist.into(),
                album_artist: artist.into(),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        db.put_tracks(&tracks, 1).unwrap();
    }
    let view = db.view(&LibraryQuery::default()).unwrap();
    let counts = view
        .browser
        .artists
        .iter()
        .map(|artist| (artist.label.as_str(), artist.count))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(counts["Ann"], 3);
    assert_eq!(counts["Bee"], 3);
    assert_eq!(counts["Cee"], 1);
    assert_eq!(counts["Tanchiky"], 1);
    assert!(!counts.contains_key("Ann & Bee"));
    let query = LibraryQuery {
        selection: sonora::browser::BrowserSelection {
            artists: vec!["Bee".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    let filtered = db.view(&query).unwrap();
    assert_eq!(filtered.track_count, 4);
    assert_eq!(filtered.albums.len(), 3);
    let releases = db.artist_releases("Ann", "").unwrap();
    let ids = releases
        .iter()
        .map(|album| album.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        ids,
        std::collections::HashSet::from(["amp", "semi", "repeat"])
    );
    assert_eq!(db.artist_releases("Tanchiky", "").unwrap().len(), 1);
}

#[test]
fn release_categories_follow_server_types_without_guessing_from_titles() {
    for (types, expected) in [
        (vec!["album", "live"], ReleaseCategory::Album),
        (vec!["EP"], ReleaseCategory::Ep),
        (vec!["single", "remix"], ReleaseCategory::Single),
        (vec![], ReleaseCategory::Album),
        (vec![""], ReleaseCategory::Album),
        (vec!["compilation"], ReleaseCategory::Other),
    ] {
        let album = Album {
            name: "An EP sounding title".into(),
            release_types: types.into_iter().map(str::to_string).collect(),
            ..Default::default()
        };
        assert_eq!(album.category(), expected);
    }
    let album: Album = serde_json::from_value(serde_json::json!({"id":"single","releaseTypes":["single"],"artists":[{"id":"main","name":"Main"}]})).unwrap();
    assert_eq!(album.category(), ReleaseCategory::Single);
    assert_eq!(album.artists[0].id, "main");
}

#[test]
fn artist_discography_includes_collaborations_and_ignores_filters_and_featured_credits() {
    let mut db = Catalog::memory().unwrap();
    let main = NamedId {
        id: "main".into(),
        name: "Main".into(),
    };
    let mut albums = Vec::new();
    for (id, name, artist, artist_id, year, release_type) in [
        ("album", "Debut", "Main", "main", 2023, "album"),
        ("ep", "Follow-up", "Main", "main", 2025, "ep"),
        ("single", "Together", "Main & Other", "pair", 2024, "single"),
        (
            "legacy",
            "Legacy",
            "Main & Guest",
            "legacy-pair",
            2022,
            "album",
        ),
        ("featured", "Featured song", "Other", "other", 2026, "album"),
        (
            "unrelated",
            "Not the same artist",
            "Mainland",
            "unrelated",
            2026,
            "album",
        ),
    ] {
        let track = Track {
            id: format!("track-{id}"),
            album_id: id.into(),
            album: name.into(),
            artist: if id == "featured" {
                "Main".into()
            } else {
                artist.into()
            },
            album_artist: artist.into(),
            artists: if id == "featured" {
                vec![main.clone()]
            } else {
                vec![]
            },
            album_artists: if id == "legacy" {
                vec![main.clone()]
            } else {
                vec![]
            },
            ..Default::default()
        };
        db.put_tracks(&[track], 1).unwrap();
        albums.push(Album {
            id: id.into(),
            name: name.into(),
            artist: artist.into(),
            artist_id: artist_id.into(),
            year: Some(year),
            release_types: vec![release_type.into()],
            artists: if id == "single" {
                vec![main.clone()]
            } else {
                vec![]
            },
            ..Default::default()
        });
    }
    db.put_albums(&albums, 1).unwrap();
    let unrelated_filter = LibraryQuery {
        text: "nothing matches this".into(),
        ..Default::default()
    };
    assert!(db.view(&unrelated_filter).unwrap().albums.is_empty());
    for id in ["main", ""] {
        let releases = db.artist_releases("Main", id).unwrap();
        assert_eq!(
            releases
                .iter()
                .map(|album| album.id.as_str())
                .collect::<Vec<_>>(),
            ["ep", "single", "album", "legacy"]
        );
    }
    assert!(
        db.artist_releases("Missing artist", "missing")
            .unwrap()
            .is_empty()
    );
    let page = db.artist_page("Main", "main").unwrap();
    let track_ids = page
        .tracks
        .iter()
        .map(|track| track.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        track_ids,
        std::collections::HashSet::from([
            "track-album",
            "track-ep",
            "track-single",
            "track-legacy"
        ])
    );
}

#[test]
fn artist_search_places_songs_in_their_release_section() {
    let release = |id: &str, name: &str, kind: &str| {
        Arc::new(Album {
            id: id.into(),
            name: name.into(),
            release_types: vec![kind.into()],
            ..Default::default()
        })
    };
    let song = |id: &str, album_id: &str, title: &str| {
        Arc::new(Track {
            id: id.into(),
            album_id: album_id.into(),
            title: title.into(),
            ..Default::default()
        })
    };
    let albums = vec![
        release("a", "Debut", "album"),
        release("e", "Follow Up", "ep"),
        release("s", "Together", "single"),
        release("c", "Collection", "compilation"),
    ];
    let tracks = vec![
        song("1", "a", "Yellow"),
        song("2", "a", "Other"),
        song("3", "e", "Yellow Light"),
        song("4", "s", "Together"),
        song("5", "missing", "Yellow"),
    ];
    let yellow = search_releases(&albums, &tracks, "YELLOW");
    assert_eq!(
        yellow
            .iter()
            .map(|(category, hits)| {
                (
                    *category,
                    hits.iter()
                        .map(|hit| {
                            (
                                hit.album.id.as_str(),
                                hit.songs
                                    .iter()
                                    .map(|track| track.id.as_str())
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (ReleaseCategory::Album, vec![("a", vec!["1"])]),
            (ReleaseCategory::Ep, vec![("e", vec!["3"])]),
        ]
    );
    let debut = search_releases(&albums, &tracks, "debut");
    assert_eq!(debut.len(), 1);
    assert_eq!(debut[0].0, ReleaseCategory::Album);
    assert!(debut[0].1[0].songs.is_empty());
    let together = search_releases(&albums, &tracks, "together");
    assert_eq!(together[0].0, ReleaseCategory::Single);
    assert_eq!(together[0].1[0].songs[0].id, "4");
    assert!(search_releases(&albums, &tracks, "  ").is_empty());
    assert!(search_releases(&albums, &tracks, "nope").is_empty());
}
