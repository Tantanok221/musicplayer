use crate::common::ui::{Fixture, start};
use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    model::{PlaybackSnapshot, Preferences, Track},
    platform::Account,
    service::{Command, Event},
};
use std::sync::Arc;

#[gpui_kit::test]
fn reconnect_keeps_cached_cover_and_backdrop_but_account_switch_drops_them(
    cx: &mut TestAppContext,
) {
    let (dir, mut f, window, app) = start(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let path = dir.path().join("shared-cover.png");
    image::RgbImage::from_pixel(32, 32, image::Rgb([80, 100, 160]))
        .save(&path)
        .unwrap();
    let backdrop = Arc::new(sonora::backdrop::load_or_render(&path).unwrap());
    f.backend
        .emit(Event::Artwork("shared-cover".into(), path, false));
    f.backend
        .emit(Event::Backdrop("shared-cover".into(), backdrop));

    let track = Arc::new(Track {
        id: "shared-track".into(),
        cover_art: "shared-cover".into(),
        ..Default::default()
    });
    let play = |f: &Fixture| {
        f.backend
            .emit(Event::Lyrics(track.id.clone(), Err("No lyrics".into())));
        f.backend.emit(Event::Playback(PlaybackSnapshot {
            track: Some(track.clone()),
            ..Default::default()
        }));
    };
    let stop = |f: &mut Fixture, cx: &mut TestAppContext| {
        f.backend.emit(Event::Playback(PlaybackSnapshot::default()));
        f.settle(window, cx);
    };
    play(&f);
    f.settle(window, cx);
    f.click(window, ("workspace-tab", 4_u64), cx);

    let backdrop_label = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            window
                .find("now-playing-backdrop")
                .label()
                .unwrap()
                .to_owned()
        })
        .unwrap()
    };
    assert_eq!(backdrop_label(cx), "Backdrop for shared-cover");

    let mut account = Account {
        url: "https://fixture.invalid".into(),
        username: "fixture".into(),
    };
    stop(&mut f, cx);
    f.backend
        .emit(Event::Connected(account.clone(), "Connected".into()));
    f.settle(window, cx);
    play(&f);
    f.settle(window, cx);
    assert_eq!(backdrop_label(cx), "Backdrop for shared-cover");

    account.username = "other-listener".into();
    stop(&mut f, cx);
    let prefs = Preferences {
        tabs: cx.update(|cx| app.read(cx).tab_state().clone()),
        ..f.catalog.state("preferences").unwrap()
    };
    f.expect(
        "look up the reused cover ID for the new account",
        |command, _| {
            assert!(matches!(command, Command::Artwork(id, None) if id == "shared-cover"));
            vec![Event::ArtworkFailed("shared-cover".into())]
        },
    );
    f.backend.emit(Event::Ready(Some(account.clone()), prefs));
    f.backend
        .emit(Event::Connected(account, "Connected".into()));
    f.settle(window, cx);
    play(&f);
    f.settle(window, cx);
    assert_eq!(backdrop_label(cx), "Backdrop unavailable");
    f.assert_script_finished();
}

#[gpui_kit::test]
fn backdrop_waits_for_current_cover_ignores_late_previous_cover_and_handles_missing_art(
    cx: &mut TestAppContext,
) {
    let (dir, mut f, window, _) = start(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let mut tracks = Vec::new();
    let mut backdrops = Vec::new();
    for (index, id) in ["cover-a", "cover-b"].iter().enumerate() {
        let path = dir.path().join(format!("{id}.png"));
        image::RgbImage::from_pixel(32, 32, image::Rgb([80, 100 + index as u8 * 40, 160]))
            .save(&path)
            .unwrap();
        backdrops.push(Arc::new(sonora::backdrop::load_or_render(&path).unwrap()));
        let mut track = (*f.catalog.by_ids(&[format!("track-{}", index + 1)]).unwrap()[0]).clone();
        track.cover_art = (*id).into();
        f.backend.emit(Event::Artwork((*id).into(), path, false));
        f.backend.emit(Event::Lyrics(
            track.id.clone(),
            Err("Fixture has no lyrics".into()),
        ));
        tracks.push(Arc::new(track));
    }
    let publish = |f: &Fixture, index: usize, revision: u64| {
        f.backend.emit(Event::Playback(PlaybackSnapshot {
            track: Some(tracks[index].clone()),
            queue: Arc::new(tracks.clone()),
            queue_ids: Arc::new(vec![1, 2]),
            index: Some(index),
            queue_revision: revision,
            duration: 120.,
            ..Default::default()
        }))
    };
    publish(&f, 0, 1);
    f.settle(window, cx);
    f.expect("first backdrop request", |command, _| {
        assert!(matches!(command, Command::Backdrop(id) if id == "cover-a"));
        vec![]
    });
    album(&mut f, "album-1");

    f.click(window, ("workspace-tab", 4_u64), cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("now-playing-backdrop").label(),
            Some("Backdrop unavailable")
        )
    })
    .unwrap();
    f.expect("new playing cover backdrop", |command, _| {
        assert!(matches!(command, Command::Backdrop(id) if id == "cover-b"));
        vec![]
    });
    album(&mut f, "album-2");

    publish(&f, 1, 2);
    f.settle(window, cx);
    // The old request completes after playback has moved on.
    f.backend
        .emit(Event::Backdrop("cover-a".into(), backdrops[0].clone()));
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("backdrop-layer-cover-a").is_none());
        assert_eq!(
            window.find("now-playing-backdrop").label(),
            Some("Backdrop unavailable")
        );
    })
    .unwrap();
    f.backend
        .emit(Event::Backdrop("cover-b".into(), backdrops[1].clone()));
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        let layer = window.find("backdrop-layer-cover-b");
        assert!(layer.visible());
        assert_eq!(layer.bounds(), window.find("now-playing-backdrop").bounds());
        assert_eq!(layer.role(), Some(gpui_kit::Role::Image));
    })
    .unwrap();
    // Return to A: the cached result renders without a second request, and
    // reduced motion leaves only the current image rather than a previous layer.
    album(&mut f, "album-1");
    publish(&f, 0, 3);
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("backdrop-layer-cover-a").visible());
        assert!(window.try_find("backdrop-layer-cover-b").is_none());
    })
    .unwrap();
    let mut missing = (*tracks[1]).clone();
    missing.cover_art = "missing".into();
    album(&mut f, "album-2");
    f.expect("missing cover disk lookup", |command, _| {
        assert!(matches!(command, Command::Artwork(id, _) if id == "missing"));
        vec![Event::ArtworkFailed("missing".into())]
    });
    f.backend.emit(Event::Playback(PlaybackSnapshot {
        track: Some(Arc::new(missing)),
        queue_revision: 4,
        ..Default::default()
    }));
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("now-playing-backdrop").label(),
            Some("Backdrop unavailable")
        );
        assert!(window.try_find("backdrop-layer-cover-a").is_none());
        assert!(window.find("play-pause").visible());
    })
    .unwrap();
    f.assert_script_finished();
}

fn album(f: &mut Fixture, expected: &'static str) {
    f.expect("now-playing album tracks", move |command, catalog| {
        assert!(matches!(command, Command::Album(id) if id == expected));
        vec![Event::Album(
            expected.into(),
            catalog.album_tracks(expected).unwrap(),
        )]
    });
}
