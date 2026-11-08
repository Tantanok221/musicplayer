use serde_json::json;
use sonora::{
    browser::CoverRows,
    catalog::Catalog,
    model::{Album, LibraryQuery, Preferences, Track},
    sorting::{Field, Grouping, Preset, Rule, Scope, SortSpec, natural_key},
};

fn album(id: &str, name: &str, artist: &str, year: Option<i32>) -> Album {
    Album {
        id: id.into(),
        name: name.into(),
        artist: artist.into(),
        year,
        ..Default::default()
    }
}
fn track(id: &str, album: &Album, number: u32) -> Track {
    Track {
        id: id.into(),
        title: format!("Song {number}"),
        album_id: album.id.clone(),
        album: album.name.clone(),
        artist: album.artist.clone(),
        album_artist: album.artist.clone(),
        year: album.year,
        disc_number: Some(1),
        track: Some(number),
        duration: 180.,
        ..Default::default()
    }
}
fn seed(db: &mut Catalog, albums: &[Album]) {
    let tracks = albums
        .iter()
        .map(|a| track(&format!("track-{}", a.id), a, 1))
        .collect::<Vec<_>>();
    db.put_tracks(&tracks, 1).unwrap();
    db.put_albums(albums, 1).unwrap();
}
fn query(spec: SortSpec) -> LibraryQuery {
    LibraryQuery {
        ordering: Some(spec),
        ..Default::default()
    }
}
fn albums(db: &Catalog, spec: SortSpec) -> Vec<String> {
    db.view(&query(spec))
        .unwrap()
        .albums
        .iter()
        .map(|a| a.id.clone())
        .collect()
}
fn tracks(db: &Catalog, spec: SortSpec) -> Vec<String> {
    db.matching_ids(&query(spec)).unwrap()
}

#[test]
fn discography_uses_album_artist_original_date_and_album_identity() {
    let mut db = Catalog::memory().unwrap();
    let mut reissue = album("reissue", "Early Album", "The Beatles", Some(2024));
    reissue.original_release_date = Some(json!({"year":1963,"month":3,"day":22}));
    let later = album("later", "A Later Album", "The Beatles", Some(1969));
    let first = album("first", "Volume 2", "ABBA", Some(1973));
    let mut duplicate = reissue.clone();
    duplicate.id = "other-edition".into();
    seed(&mut db, &[later, reissue, duplicate, first]);
    assert_eq!(
        albums(&db, Preset::Discography.spec(Scope::Albums)),
        ["first", "other-edition", "reissue", "later"]
    );
    let mut song = db.by_ids(&["track-reissue".into()]).unwrap()[0]
        .as_ref()
        .clone();
    song.artist = "Guest singer".into();
    db.put_tracks(&[song], 2).unwrap();
    assert_eq!(
        albums(&db, Preset::Discography.spec(Scope::Albums)),
        ["first", "other-edition", "reissue", "later"],
        "track updates must not replace authoritative album sort metadata"
    );
}

#[test]
fn mixed_directions_do_not_reverse_album_tracks_or_playback() {
    let mut db = Catalog::memory().unwrap();
    let a = album("a", "A", "Artist", Some(2000));
    let b = album("b", "B", "Artist", Some(2020));
    seed(&mut db, &[a.clone(), b]);
    let mut more = vec![
        track("disc2-track1", &a, 1),
        track("disc1-track10", &a, 10),
        track("disc1-track2", &a, 2),
    ];
    more[0].disc_number = Some(2);
    db.put_tracks(&more, 1).unwrap();
    let mut spec = Preset::Discography.spec(Scope::Albums);
    spec.rules[1].descending = true;
    assert_eq!(albums(&db, spec.clone()), ["b", "a"]);
    let matching = db.album_tracks_matching(&query(spec), "a").unwrap();
    let ids = matching.iter().map(|t| t.id.as_str()).collect::<Vec<_>>();
    assert_eq!(
        ids,
        ["track-a", "disc1-track2", "disc1-track10", "disc2-track1"]
    );
    assert_eq!(
        db.album_queue_from("disc1-track2")
            .unwrap()
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        ["disc1-track2", "disc1-track10", "disc2-track1"]
    );
}

#[test]
fn natural_unicode_names_sort_without_changing_display_labels() {
    assert!(natural_key("Volume 2") < natural_key("Volume 10"));
    assert_eq!(natural_key("Ｖｏｌｕｍｅ ２"), natural_key("volume 2"));
    assert_eq!(natural_key("Cafe\u{301}"), natural_key("CAFÉ"));
    let mut db = Catalog::memory().unwrap();
    let mut first = album("first", "First", "The Beatles", None);
    first.sort_name = "Volume 2".into();
    seed(&mut db, &[album("last", "Volume 10", "Zoo", None), first]);
    let spec = Preset::Album.spec(Scope::Albums);
    assert_eq!(albums(&db, spec.clone()), ["first", "last"]);
    let view = db.view(&query(spec)).unwrap();
    assert_eq!(view.albums[0].name, "First");
    let artist = Preset::ArtistAlbum.spec(Scope::Albums);
    assert_eq!(artist.album_letter(&view.albums[0]), 'B');
    let mut literal = artist;
    literal.ignore_the = false;
    assert_eq!(literal.album_letter(&view.albums[0]), 'T');
    assert!(
        Preset::ReleaseDate
            .spec(Scope::Albums)
            .alphabetical_field()
            .is_none()
    );
}

#[test]
fn added_rating_and_plays_use_album_values_and_unknowns_stay_last() {
    let mut db = Catalog::memory().unwrap();
    let mut a = album("a", "A", "Artist", None);
    a.created = Some("2020-01-01T00:00:00Z".into());
    a.user_rating = Some(5);
    a.play_count = Some(0);
    let mut b = album("b", "B", "Artist", None);
    b.created = Some("2025-01-01T01:00:00+01:00".into());
    b.user_rating = Some(2);
    b.play_count = Some(20);
    let mut c = album("c", "C", "Artist", None);
    c.user_rating = Some(0);
    seed(&mut db, &[b, a, c]);
    assert_eq!(
        albums(&db, Preset::Added.spec(Scope::Albums)),
        ["b", "a", "c"]
    );
    assert_eq!(
        albums(&db, Preset::Rating.spec(Scope::Albums)),
        ["a", "b", "c"]
    );
    assert_eq!(
        albums(&db, Preset::Plays.spec(Scope::Albums)),
        ["b", "a", "c"]
    );
    let mut spec = Preset::Rating.spec(Scope::Albums);
    spec.rules[0].descending = false;
    assert_eq!(albums(&db, spec), ["b", "a", "c"]);
    let mut spec = Preset::Plays.spec(Scope::Albums);
    spec.rules[0].descending = false;
    assert_eq!(albums(&db, spec), ["a", "b", "c"]);
    let view = db.view(&query(Preset::Rating.spec(Scope::Albums))).unwrap();
    assert!(view.sort_fields.contains(&Field::Rating));
    assert!(view.sort_fields.contains(&Field::Added));
}

#[test]
fn dates_have_explicit_precision_fallbacks_and_null_placement() {
    let mut db = Catalog::memory().unwrap();
    let mut exact = album("exact", "A", "Artist", None);
    exact.original_release_date = Some(json!({"year":2000,"month":2,"day":3}));
    let partial = album("partial", "B", "Artist", Some(2000));
    let mut edition = album("edition", "C", "Artist", None);
    edition.release_date = Some(json!({"year":1990,"month":1}));
    let unknown = album("unknown", "D", "Artist", None);
    seed(&mut db, &[partial, unknown, exact, edition]);
    let mut spec = Preset::ReleaseDate.spec(Scope::Albums);
    assert_eq!(
        albums(&db, spec.clone()),
        ["exact", "partial", "edition", "unknown"]
    );
    spec.rules[0].descending = false;
    assert_eq!(
        albums(&db, spec),
        ["edition", "exact", "partial", "unknown"],
        "less precise dates follow precise ones within the same year in either direction"
    );
}

#[test]
fn track_artist_is_distinct_from_album_artist_and_album_statistics() {
    let mut db = Catalog::memory().unwrap();
    let a = album("compilation", "Compilation", "Various Artists", Some(2020));
    let mut t1 = track("one", &a, 1);
    t1.artist = "Zebra".into();
    t1.user_rating = Some(1);
    let mut t2 = track("two", &a, 2);
    t2.artist = "ABBA".into();
    t2.user_rating = Some(5);
    db.put_tracks(&[t1, t2], 1).unwrap();
    db.put_albums(&[a], 1).unwrap();
    assert_eq!(
        tracks(&db, Preset::ArtistTitle.spec(Scope::Tracks)),
        ["two", "one"]
    );
    assert_eq!(
        tracks(&db, Preset::ArtistAlbum.spec(Scope::Tracks)),
        ["one", "two"]
    );
    assert_eq!(
        tracks(&db, Preset::Rating.spec(Scope::Tracks)),
        ["two", "one"]
    );
    let mut spec = Preset::Rating.spec(Scope::Tracks);
    spec.rules[0].descending = false;
    assert_eq!(tracks(&db, spec), ["one", "two"]);
}

#[test]
fn filtering_does_not_change_an_albums_statistics_or_date() {
    let mut db = Catalog::memory().unwrap();
    let mut a = album("a", "A", "Artist", Some(2020));
    a.user_rating = Some(5);
    let mut b = album("b", "B", "Artist", Some(2020));
    b.user_rating = Some(1);
    seed(&mut db, &[a, b]);
    let mut q = query(Preset::Rating.spec(Scope::Albums));
    q.text = "song".into();
    assert_eq!(
        db.view(&q)
            .unwrap()
            .albums
            .iter()
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    q.text = "Song 1".into();
    assert_eq!(db.view(&q).unwrap().albums[0].user_rating, Some(5));
}

#[test]
fn pagination_sorts_the_complete_result_and_uses_stable_ties() {
    let mut db = Catalog::memory().unwrap();
    let a = album("a", "A", "Artist", Some(2020));
    let data = (0..1203)
        .rev()
        .map(|n| {
            let mut t = track(&format!("id-{n:04}"), &a, 1);
            t.title = format!("Volume {}", n / 2);
            t
        })
        .collect::<Vec<_>>();
    db.put_tracks(&data, 1).unwrap();
    let q = query(Preset::Title.spec(Scope::Tracks));
    let expected = db.matching_ids(&q).unwrap();
    let actual = [0, 500, 1000]
        .into_iter()
        .flat_map(|offset| {
            db.tracks(&q, offset, 500)
                .unwrap()
                .into_iter()
                .map(|t| t.id.clone())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(expected, actual);
    assert_eq!(actual.len(), 1203);
    assert_eq!(&actual[..4], ["id-0000", "id-0001", "id-0002", "id-0003"]);
    assert!(db.view(&q).unwrap().revision > 0);
}

#[test]
fn play_count_and_rating_updates_refresh_sort_keys() {
    let mut db = Catalog::memory().unwrap();
    let a = album("a", "A", "Artist", None);
    let mut one = track("one", &a, 1);
    one.play_count = Some(1);
    one.user_rating = Some(1);
    let mut two = track("two", &a, 2);
    two.play_count = Some(2);
    two.user_rating = Some(2);
    db.put_tracks(&[one.clone(), two], 1).unwrap();
    let before = db.revision();
    one.play_count = Some(5);
    db.refresh_play_count(&one).unwrap();
    assert!(db.revision() > before);
    assert_eq!(
        tracks(&db, Preset::Plays.spec(Scope::Tracks)),
        ["one", "two"]
    );
    one.user_rating = Some(5);
    db.put_tracks(&[one], 1).unwrap();
    assert_eq!(
        tracks(&db, Preset::Rating.spec(Scope::Tracks)),
        ["one", "two"]
    );
}

#[test]
fn additive_backfill_and_incomplete_sync_preserve_cached_data() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("catalog.sqlite");
    {
        let mut db = Catalog::open(&path).unwrap();
        seed(
            &mut db,
            &[
                album("a", "Volume 2", "Artist", Some(2000)),
                album("b", "Volume 10", "Artist", Some(2001)),
            ],
        );
    }
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE sort_tracks; DROP TABLE sort_albums;")
            .unwrap();
    }
    let mut db = Catalog::open(&path).unwrap();
    assert_eq!(db.count().unwrap(), 2);
    assert!(
        !db.view(&query(Preset::Added.spec(Scope::Albums)))
            .unwrap()
            .sort_fields
            .contains(&Field::Added),
        "migration must not invent historical added dates"
    );
    assert_eq!(albums(&db, Preset::Album.spec(Scope::Albums)), ["a", "b"]);
    let a = album("a", "Volume 2", "Artist", Some(2000));
    db.put_tracks(&[track("track-a", &a, 1)], 2).unwrap();
    assert_eq!(albums(&db, Preset::Album.spec(Scope::Albums)), ["a", "b"]);
    db.finish_sync(2).unwrap();
    assert_eq!(albums(&db, Preset::Album.spec(Scope::Albums)), ["a"]);
}

#[test]
fn built_in_sort_directions_roundtrip_with_independent_view_defaults() {
    let mut prefs: Preferences = serde_json::from_value(json!({"volume":0.5})).unwrap();
    assert_eq!(
        prefs.sorting.albums,
        Preset::ArtistAlbum.spec(Scope::Albums)
    );
    assert_eq!(prefs.sorting.tracks, Preset::Title.spec(Scope::Tracks));
    let mut spec = Preset::Discography.spec(Scope::Albums);
    spec.rules[1].descending = true;
    prefs.sorting.set(spec.clone());
    let copy: Preferences = serde_json::from_slice(&serde_json::to_vec(&prefs).unwrap()).unwrap();
    assert_eq!(copy.sorting.get(Scope::Albums), spec);
    assert_eq!(spec.label(), "Artist discography");
    assert_eq!(spec.preset(), Some(Preset::Discography));
    assert_eq!(copy.sorting.tracks, Preset::Title.spec(Scope::Tracks));
    let mut reverse_title = Preset::Title.spec(Scope::Tracks);
    reverse_title.rules[0].descending = true;
    assert_eq!(reverse_title.preset(), Some(Preset::Title));
    let db = Catalog::memory().unwrap();
    db.set_state("preferences", &copy).unwrap();
    let loaded: Preferences = db.state("preferences").unwrap();
    assert_eq!(loaded.sorting.get(Scope::Albums), spec);
}

#[test]
fn removed_custom_settings_load_safely_without_exposing_hidden_orders() {
    let mut custom = Preset::Discography.spec(Scope::Albums);
    custom.grouping = Grouping::Year;
    let prefs: Preferences = serde_json::from_value(json!({
        "volume":0.5,
        "sorting": { "albums": custom, "saved": [{"name":"Old custom", "spec": custom}] }
    }))
    .unwrap();
    assert_eq!(prefs.volume, 0.5);
    assert_eq!(
        prefs.sorting.get(Scope::Albums),
        Preset::ArtistAlbum.spec(Scope::Albums)
    );
    assert!(
        serde_json::to_value(&prefs.sorting)
            .unwrap()
            .get("saved")
            .is_none()
    );
}

#[test]
fn grouped_covers_and_jumps_preserve_every_album() {
    let rows = CoverRows::grouped(23, 4, Some(10), &[0, 3, 9, 16]);
    assert_eq!(
        (0..rows.row_count())
            .flat_map(|r| rows.albums_in_row(r))
            .collect::<Vec<_>>(),
        (0..23).collect::<Vec<_>>()
    );
    for index in [0, 3, 9, 10, 16] {
        assert_eq!(rows.albums_in_row(rows.row_for_album(index)).start, index);
    }
    let mut db = Catalog::memory().unwrap();
    seed(
        &mut db,
        &[
            album("old", "Z", "Zulu", Some(1990)),
            album("new", "A", "ABBA", Some(2020)),
        ],
    );
    let mut spec = Preset::Album.spec(Scope::Albums);
    spec.grouping = Grouping::Year;
    spec.group_descending = true;
    assert_eq!(albums(&db, spec), ["new", "old"]);
}

#[test]
fn invalid_or_duplicate_saved_rules_are_validated_for_the_view() {
    let mut spec = Preset::Title.spec(Scope::Tracks);
    spec.rules.push(Rule::new(Field::Title, true));
    let validated = spec.validated(Scope::Albums);
    assert_eq!(validated.scope, Scope::Albums);
    assert_eq!(validated.rules, [Rule::new(Field::Album, false)]);
    assert_eq!(validated.grouping, Grouping::None);
}
