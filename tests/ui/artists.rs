use crate::common::ui::{
    ARTIST, ARTIST_TARGET, SECOND_ARTIST, SECOND_ARTIST_TARGET, UNOWNED_ARTIST, start,
};
use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    artists::{ArtistProfile, SimilarArtist, TopSongs, cache_key},
    discography::{DiscographyView, MissingRelease},
    service::Event,
};
use std::sync::Arc;

#[gpui_kit::test]
fn stale_artist_and_late_enrichment_do_not_replace_the_newer_page(cx: &mut TestAppContext) {
    let (_dir, mut f, window, _) = start(cx);
    f.hold_artists();
    f.click(window, "new-tab", cx);
    f.click(window, ARTIST_TARGET, cx);
    f.click(window, SECOND_ARTIST_TARGET, cx);
    f.deliver_artist(SECOND_ARTIST);
    f.settle(window, cx);
    let assert_second = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            assert!(window.find("artist-release-album-3").visible());
            assert!(window.try_find("artist-release-album-1").is_none());
        })
        .unwrap();
    };
    assert_second(cx);
    f.deliver_artist(ARTIST);
    f.settle(window, cx);
    assert_second(cx);
    for name in [ARTIST, SECOND_ARTIST] {
        let id = if name == ARTIST {
            "artist-1"
        } else {
            "artist-2"
        };
        // Empty profile is the worker's failure fallback; top-song failure sends no event.
        f.backend.emit(Event::ArtistProfile(
            cache_key(name, id),
            Arc::new(ArtistProfile::default()),
        ));
        f.settle(window, cx);
        assert_second(cx);
        f.backend.emit(Event::ArtistProfile(
            cache_key(name, id),
            Arc::new(ArtistProfile {
                name: name.into(),
                complete: true,
                biography: "Late biography".into(),
                ..Default::default()
            }),
        ));
        f.backend.emit(Event::TopSongs(
            cache_key(name, id),
            Arc::new(TopSongs::default()),
        ));
        f.settle(window, cx);
        assert_second(cx);
    }
    f.click(window, "artist-release-album-3", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("album-favorite-album-3").visible())
    })
    .unwrap();
}

#[gpui_kit::test]
fn failed_release_details_can_be_reopened_and_hidden_through_the_ui(cx: &mut TestAppContext) {
    use crate::downloads::{reveal, start_missing_with_details};
    use sonora::{
        discography::{ReleaseDetails, ReleaseTrack},
        service::Command,
    };
    let (_dir, mut f, window) =
        start_missing_with_details(cx, Err("Fixture details failed".into()));
    reveal(&mut f, window, "missing-hide", cx);
    cx.update_window(window, |_, window, _| {
        assert!(
            window
                .find("release-details-error")
                .label()
                .unwrap()
                .contains("Fixture details failed")
        );
        assert!(window.find("missing-hide").visible());
    })
    .unwrap();
    reveal(&mut f, window, "missing-release-missing", cx);
    f.click(window, "missing-release-missing", cx); // close failed panel
    f.expect("retry release details", |command, _| {
        assert!(matches!(command, Command::ReleaseDetails(id) if id == "missing"));
        vec![Event::ReleaseDetails(
            "missing".into(),
            Ok(Arc::new(ReleaseDetails {
                tracks: vec![ReleaseTrack {
                    disc: 1,
                    number: "1".into(),
                    title: "Recovered track".into(),
                    ..Default::default()
                }],
                ..Default::default()
            })),
        )]
    });
    f.click(window, "missing-release-missing", cx);
    reveal(&mut f, window, "release-detail-track-missing-1-1", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("release-detail-track-missing-1-1").label(),
            Some("Recovered track")
        )
    })
    .unwrap();
    f.expect("hide selected missing release", |command, _| {
        assert!(matches!(command, Command::HideRelease(name, id, group) if name == ARTIST && id == "artist-1" && group == "missing"));
        vec![Event::Discography(cache_key(ARTIST, "artist-1"), Arc::new(DiscographyView::default()))]
    });
    reveal(&mut f, window, "missing-hide", cx);
    f.click(window, "missing-hide", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("missing-release-missing").is_none());
        assert!(window.try_find("missing-hide").is_none());
    })
    .unwrap();
    f.assert_script_finished();
}

#[gpui_kit::test]
fn unowned_similar_artist_opens_its_artist_page_with_a_not_in_library_label(
    cx: &mut TestAppContext,
) {
    use crate::downloads::reveal;
    let (_dir, mut f, window, app) = start(cx);
    f.click(window, "new-tab", cx);
    f.click(window, ARTIST_TARGET, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("artist-release-album-1").visible());
        assert!(window.try_find("artist-not-in-library").is_none());
    })
    .unwrap();
    f.backend.emit(Event::ArtistProfile(
        cache_key(ARTIST, "artist-1"),
        Arc::new(ArtistProfile {
            name: ARTIST.into(),
            complete: true,
            similar: vec![
                SimilarArtist {
                    name: SECOND_ARTIST.into(),
                    id: "artist-2".into(),
                    in_library: true,
                    ..Default::default()
                },
                SimilarArtist {
                    name: UNOWNED_ARTIST.into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
    ));
    f.settle(window, cx);
    let card = format!("similar-{UNOWNED_ARTIST}");
    let owned = format!("similar-{SECOND_ARTIST}");
    reveal(&mut f, window, card.clone(), cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find(card.clone()).label(),
            Some("Open artist Stranger Band, not in your library")
        );
        assert_eq!(
            window.find(owned.clone()).label(),
            Some(format!("Open artist {SECOND_ARTIST}").as_str())
        );
    })
    .unwrap();

    // The click stays in the app: the artist page opens instead of Last.fm.
    f.click(window, card.clone(), cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("artist-release-album-1").is_none());
        let label = window.find("artist-not-in-library");
        assert!(label.visible());
        assert_eq!(
            label.label(),
            Some("Not in your library: no songs from this artist")
        );
    })
    .unwrap();

    // Its discography still fills in as missing releases.
    f.backend.emit(Event::Discography(
        cache_key(UNOWNED_ARTIST, ""),
        Arc::new(DiscographyView {
            loading: false,
            missing: vec![MissingRelease {
                id: "stranger-debut".into(),
                title: "Stranger Debut".into(),
                kind: "Album".into(),
                core: true,
                ..Default::default()
            }],
        }),
    ));
    f.settle(window, cx);
    reveal(&mut f, window, "missing-release-stranger-debut", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("missing-release-stranger-debut").visible());
    })
    .unwrap();
    f.assert_script_finished();
    f.diagnose("unowned similar artist page", window, &app, cx);
}
