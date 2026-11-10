use sonora::{
    discography::{Discography, Library, ReleaseGroup, fold, is_various_artists, missing},
    model::Album,
};
use std::sync::Arc;

fn group(id: &str, title: &str, primary: &str) -> ReleaseGroup {
    ReleaseGroup {
        id: id.into(),
        title: title.into(),
        primary: primary.into(),
        date: "2010-01-01".into(),
        official: true,
        ..Default::default()
    }
}
fn album(name: &str, mbid: &str) -> Arc<Album> {
    Arc::new(Album {
        id: name.into(),
        name: name.into(),
        music_brainz_id: mbid.into(),
        ..Default::default()
    })
}
fn titles(discography: &Discography, library: &Library) -> Vec<String> {
    missing(discography, library, "2026-11-10")
        .into_iter()
        .map(|release| release.title)
        .collect()
}

#[test]
fn folding_ignores_width_case_accents_brackets_and_edition_words() {
    assert_eq!(fold("ＯＫ Computer"), fold("ok computer"));
    assert_eq!(fold("Zankyō Reference"), fold("Zankyo Reference"));
    assert_eq!(fold("Rammstein (Deluxe Edition)"), fold("RAMMSTEIN"));
    assert_eq!(fold("Happy Ending (通常盤)"), fold("Happy Ending"));
    assert_eq!(fold("Abbey Road - Remastered"), fold("Abbey Road"));
    assert_eq!(fold("Sad Robot EP"), fold("Sad Robot"));
    assert_eq!(fold("Don’t Let Me Down"), fold("Don't Let Me Down"));
    // Kana voicing marks distinguish titles and must survive.
    assert_ne!(fold("かき"), fold("がき"));
    // A title that is only a symbol still identifies itself.
    assert_eq!(fold("◯"), "◯");
    assert_ne!(fold("◯"), fold("△"));
    // A title that is only an edition word is kept whole.
    assert_eq!(fold("Version"), "version");
}

#[test]
fn owned_by_release_id_edition_title_or_regional_twin() {
    let mut by_id = group("rg-id", "残響リファレンス", "Album");
    by_id.releases = vec!["release-1".into(), "release-2".into()];
    let mut romanized = group("rg-roman", "感情エフェクト", "Album");
    romanized.titles = vec!["Kanjō Effect".into()];
    let japanese = group("rg-jp", "Ambitions", "Album");
    let mut international = group("rg-intl", "Ambitions", "Album");
    international.releases = vec!["release-intl".into()];
    let discography = Discography {
        artist: "artist".into(),
        groups: vec![
            by_id,
            romanized,
            japanese,
            international,
            group("rg-missing", "Eye of the Storm", "Album"),
        ],
        fetched_ms: 1,
    };
    let albums = [
        album("Zankyo Reference", "release-2"),
        album("Kanjo Effect", ""),
        album("Ambitions (international version)", "release-intl"),
    ];
    let library = Library {
        albums: &albums,
        release_ids: ["release-2".to_string(), "release-intl".to_string()].into(),
        ..Default::default()
    };
    assert_eq!(titles(&discography, &library), ["Eye of the Storm"]);
}

#[test]
fn release_ids_match_albums_filed_under_another_artist_row() {
    let mut owned = group("rg", "STRAY SHEEP", "Album");
    owned.releases = vec!["release".into()];
    let discography = Discography {
        artist: "artist".into(),
        groups: vec![owned, group("rg-2", "BOOTLEG", "Album")],
        fetched_ms: 1,
    };
    // The page's own artist row has no albums; the other row's album is in the library.
    let library = Library {
        release_ids: ["release".to_string()].into(),
        ..Default::default()
    };
    assert_eq!(titles(&discography, &library), ["BOOTLEG"]);
}

#[test]
fn filters_unofficial_hidden_duplicate_and_already_owned_singles() {
    let mut bootleg = group("rg-bootleg", "Live in a Basement", "Album");
    bootleg.official = false;
    let mut live = group("rg-live", "Live at Budokan", "Album");
    live.secondary = vec!["Live".into()];
    let mut upcoming = group("rg-upcoming", "Next Album", "Album");
    upcoming.date = "2027-03".into();
    let discography = Discography {
        artist: "artist".into(),
        groups: vec![
            bootleg,
            live,
            upcoming,
            group("rg-hidden", "Hidden Album", "Album"),
            group("rg-single", "Owned Song", "Single"),
            group("rg-single-2", "Unowned Song", "Single"),
            group("rg-twin-1", "Twin", "EP"),
            group("rg-twin-2", "Twin", "EP"),
        ],
        fetched_ms: 1,
    };
    let library = Library {
        track_titles: [fold("Owned Song (Album Version)")].into(),
        hidden: ["rg-hidden".to_string()].into(),
        ..Default::default()
    };
    let found = missing(&discography, &library, "2026-11-10");
    let names = found
        .iter()
        .map(|release| (release.title.as_str(), release.core, release.upcoming))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            ("Next Album", true, true),
            ("Live at Budokan", false, false),
            ("Twin", true, false),
            ("Unowned Song", false, false),
        ]
    );
    assert_eq!(found[1].kind, "Live album");
    assert_eq!(found[0].year, Some(2027));
    assert_eq!(
        found[2].cover,
        "remote:https://coverartarchive.org/release-group/rg-twin-1/front-250"
    );
}

#[test]
fn a_release_still_downloading_stays_listed_after_a_partial_scan() {
    let discography = Discography {
        artist: "artist".into(),
        groups: vec![
            group("rg", "Half Here", "Album"),
            group("rg-2", "All Here", "Album"),
        ],
        fetched_ms: 1,
    };
    let albums = [album("Half Here", ""), album("All Here", "")];
    let mut library = Library {
        albums: &albums,
        downloading: ["rg".to_string()].into(),
        hidden: ["rg".to_string()].into(),
        ..Default::default()
    };
    assert_eq!(titles(&discography, &library), ["Half Here"]);
    // Once the download is no longer tracked, the album is simply owned.
    library.downloading.clear();
    assert!(titles(&discography, &library).is_empty());
}

#[test]
fn most_listened_come_first_then_newest() {
    let mut popular = group("a", "Popular", "Album");
    popular.listens = 900;
    popular.date = "1999".into();
    let mut newer = group("b", "Newer", "Album");
    newer.date = "2020-05-01".into();
    let mut older = group("c", "Older", "Album");
    older.date = "2001".into();
    let discography = Discography {
        artist: "artist".into(),
        groups: vec![older, newer, popular],
        fetched_ms: 1,
    };
    assert_eq!(
        titles(&discography, &Library::default()),
        ["Popular", "Newer", "Older"]
    );
}

#[test]
fn freshness_and_various_artists() {
    let day = 86_400_000;
    let discography = Discography {
        fetched_ms: 10,
        ..Default::default()
    };
    assert!(discography.fresh(10 + 6 * day));
    assert!(!discography.fresh(10 + 8 * day));
    assert!(!Discography::default().fresh(1));
    assert!(is_various_artists("Various Artists", ""));
    assert!(is_various_artists("Ｖ.Ａ.", ""));
    assert!(is_various_artists(
        "Anything",
        "89ad4ac3-39f7-470e-963a-56509c546377"
    ));
    assert!(!is_various_artists("Mili", ""));
}
