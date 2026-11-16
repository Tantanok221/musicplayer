use crate::common::ui::{ARTIST, ARTIST_TARGET, SECOND_ARTIST, SECOND_ARTIST_TARGET, start};
use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    artists::{ArtistProfile, TopSongs, cache_key},
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
        discography::{DiscographyView, ReleaseDetails, ReleaseTrack},
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
