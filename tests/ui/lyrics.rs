//! The source switch and timed-line input use real MusicApp handlers.
use crate::common::ui::start;
use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    browser::InfoSource,
    lyrics::{LyricLine, Lyrics},
    model::{PlaybackSnapshot, Preferences},
    playback::PlayerCommand,
    service::{Command, Event},
};
use std::sync::Arc;

fn timed(prefix: &str) -> Lyrics {
    Lyrics {
        synced: true,
        offset: 100,
        line: vec![
            LyricLine {
                start: Some(1000),
                value: format!("{prefix} first"),
            },
            LyricLine {
                start: Some(2000),
                value: format!("{prefix} second"),
            },
        ],
    }
}

#[gpui_kit::test]
fn timed_lyrics_seek_only_the_playing_track_and_keep_selected_source_distinct(
    cx: &mut TestAppContext,
) {
    let (_dir, mut fixture, window, app) = start(cx);
    let playing = fixture
        .catalog
        .by_ids(&["track-2".into()])
        .unwrap()
        .remove(0);
    fixture.backend.emit(Event::Playback(PlaybackSnapshot {
        track: Some(playing.clone()),
        queue: Arc::new(vec![playing]),
        queue_ids: Arc::new(vec![1]),
        queue_revision: 1,
        index: Some(0),
        duration: 120.,
        position: 1.,
        ..Default::default()
    }));
    fixture.settle(window, cx);
    fixture.open_album(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("inspected-track").label(),
            Some("Selected track: Fixture song")
        );
    })
    .unwrap();
    fixture.expect("playing lyrics", |command, _| {
        assert!(matches!(command, Command::Lyrics(track) if track.id == "track-2"));
        vec![Event::Lyrics(
            "track-2".into(),
            Ok(Arc::new(timed("Playing"))),
        )]
    });
    fixture.click(window, "info-tab-Lyrics", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("info-tab-Lyrics").selected(), Some(true));
        assert_eq!(
            window.find("lyrics-scroll").label(),
            Some("Lyrics for Second song")
        );
        assert_eq!(
            window.find("lyric-0").label(),
            Some("Current lyric: Playing first")
        );
        assert!(window.find("lyric-1").visible());
    })
    .unwrap();
    // Ignore preference writes, but reject any transport command before input.
    for command in fixture.backend.player_commands().try_iter() {
        assert!(matches!(command, PlayerCommand::Preferences(_)));
    }
    fixture.click(window, "lyric-1", cx);
    let commands: Vec<_> = fixture.backend.player_commands().try_iter().collect();
    assert_eq!(commands.len(), 1);
    assert!(
        matches!(commands[0], PlayerCommand::Seek(position) if (position - 1.9).abs() < 0.000001)
    );

    fixture.expect("selected lyrics", |command, _| {
        assert!(matches!(command, Command::Lyrics(track) if track.id == "track-1"));
        vec![Event::Lyrics(
            "track-1".into(),
            Ok(Arc::new(timed("Selected"))),
        )]
    });
    fixture.click(window, "info-Selected", cx);
    let prefs: Preferences = fixture.catalog.state("preferences").unwrap();
    assert_eq!(prefs.layout.info_source, InfoSource::Selected);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("lyrics-scroll").label(),
            Some("Lyrics for Fixture song")
        );
        assert_eq!(window.find("lyric-1").label(), Some("Selected second"));
    })
    .unwrap();
    fixture.click(window, "lyric-1", cx);
    for command in fixture.backend.player_commands().try_iter() {
        assert!(
            matches!(command, PlayerCommand::Preferences(_)),
            "selected-track lyrics must not seek the playing track"
        );
    }
    fixture.click(window, "info-Playing", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("lyrics-scroll").label(),
            Some("Lyrics for Second song")
        );
    })
    .unwrap();
    fixture.click(window, "info-tab-Details", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("inspected-track").label(),
            Some("Playing track: Second song")
        );
    })
    .unwrap();
    fixture.assert_script_finished();
    fixture.diagnose("lyrics source and seek checked", window, &app, cx);
}

#[gpui_kit::test]
fn now_playing_lyrics_follow_position_only_playback_updates(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let playing = fixture
        .catalog
        .by_ids(&["track-2".into()])
        .unwrap()
        .remove(0);
    let snapshot = |position: f64| PlaybackSnapshot {
        track: Some(playing.clone()),
        queue: Arc::new(vec![playing.clone()]),
        queue_ids: Arc::new(vec![1]),
        queue_revision: 1,
        index: Some(0),
        playing: true,
        duration: 120.,
        position,
        ..Default::default()
    };
    fixture.backend.emit(Event::Lyrics(
        "track-2".into(),
        Ok(Arc::new(timed("Playing"))),
    ));
    fixture.backend.emit(Event::Playback(snapshot(1.)));
    fixture.settle(window, cx);
    let album = playing.album_id.clone();
    fixture.expect("now-playing album tracks", move |command, catalog| {
        assert!(matches!(command, Command::Album(id) if *id == album));
        vec![Event::Album(
            album.clone(),
            catalog.album_tracks(&album).unwrap(),
        )]
    });
    fixture.click(window, ("workspace-tab", 4_u64), cx);
    // A display-driven frame: unlike settle(), it does not refresh the whole
    // window, so cached parts redraw only when something notified them.
    let platform_frame = |cx: &mut TestAppContext| {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    };
    let current = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            (0..2)
                .filter_map(|index| {
                    window
                        .find(format!("lyric-{index}"))
                        .label()
                        .map(str::to_owned)
                })
                .find(|label| label.starts_with("Current lyric: "))
        })
        .unwrap()
    };
    assert_eq!(current(cx).as_deref(), Some("Current lyric: Playing first"));

    // Only the position moves, with no pointer input to force a redraw.
    fixture.backend.emit(Event::Playback(snapshot(2.5)));
    platform_frame(cx);
    assert_eq!(
        current(cx).as_deref(),
        Some("Current lyric: Playing second")
    );

    // A seek back is also position-only and must move the highlight back.
    fixture.backend.emit(Event::Playback(snapshot(1.)));
    platform_frame(cx);
    assert_eq!(current(cx).as_deref(), Some("Current lyric: Playing first"));
    fixture.assert_script_finished();
    fixture.diagnose("now playing lyrics followed position", window, &app, cx);
}
