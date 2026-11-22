//! The read-only Properties tab, driven through the information panel tabs.
use crate::common::ui::start;
use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    browser::InfoTab,
    model::{PlaybackSnapshot, Preferences, ReplayGain, Track},
    service::Event,
};
use std::sync::Arc;

fn play(fixture: &mut crate::common::ui::Fixture, track: Track) {
    let track = Arc::new(track);
    fixture.backend.emit(Event::Playback(PlaybackSnapshot {
        track: Some(track.clone()),
        queue: Arc::new(vec![track]),
        queue_ids: Arc::new(vec![1]),
        queue_revision: 1,
        index: Some(0),
        duration: 120.,
        ..Default::default()
    }));
}

#[gpui_kit::test]
fn properties_tab_shows_file_and_tag_details_and_follows_the_track(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    play(
        &mut fixture,
        Track {
            id: "props-1".into(),
            title: "Tagged song".into(),
            album_id: "album-1".into(),
            album: "Fixture album".into(),
            artist: "ONE OK ROCK".into(),
            album_artist: "ONE OK ROCK".into(),
            genre: "Rock".into(),
            year: Some(2017),
            track: Some(3),
            duration: 214.,
            path: "ONE OK ROCK/Fixture album/03 Tagged song.flac".into(),
            suffix: "flac".into(),
            bit_rate: Some(1015),
            sampling_rate: Some(48_000),
            bit_depth: Some(24),
            channel_count: Some(2),
            size: Some(27 * 1024 * 1024),
            music_brainz_id: "c1a0a8b2-mbid".into(),
            replay_gain: Some(ReplayGain {
                track_gain: Some(-8.25),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    fixture.settle(window, cx);
    fixture.click(window, "info-Playing", cx);
    fixture.click(window, "info-tab-Properties", cx);
    let label = |window: &mut gpui_kit::Window, id: &str| {
        window.find(id.to_string()).label().map(str::to_string)
    };
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("info-tab-Properties").selected(), Some(true));
        assert_eq!(
            label(window, "track-properties").as_deref(),
            Some("Properties of Tagged song")
        );
        for (id, expected) in [
            (
                "property-path",
                "Path: ONE OK ROCK/Fixture album/03 Tagged song.flac",
            ),
            ("property-format", "Format: FLAC"),
            ("property-bit-rate", "Bit rate: 1015 kbps"),
            ("property-sample-rate", "Sample rate: 48.0 kHz"),
            ("property-bit-depth", "Bit depth: 24-bit"),
            ("property-size", "Size: 27.0 MB"),
            ("property-year", "Year: 2017"),
            ("property-track-number", "Track: 3"),
            ("property-track-gain", "Track gain: -8.25 dB"),
            ("property-musicbrainz-id", "MusicBrainz: c1a0a8b2-mbid"),
        ] {
            assert_eq!(label(window, id).as_deref(), Some(expected));
        }
    })
    .unwrap();
    let prefs: Preferences = fixture.catalog.state("preferences").unwrap();
    assert_eq!(prefs.layout.info_tab, InfoTab::Properties);

    // A sparsely tagged next track replaces every value instead of keeping
    // the previous track's details.
    play(
        &mut fixture,
        Track {
            id: "props-2".into(),
            title: "Bare song".into(),
            duration: 90.,
            ..Default::default()
        },
    );
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            label(window, "track-properties").as_deref(),
            Some("Properties of Bare song")
        );
        for (id, expected) in [
            ("property-path", "Path: —"),
            ("property-bit-rate", "Bit rate: —"),
            ("property-year", "Year: —"),
            ("property-genre", "Genre: —"),
            ("property-track-number", "Track: —"),
            ("property-musicbrainz-id", "MusicBrainz: —"),
            ("property-duration", "Duration: 1:30"),
        ] {
            assert_eq!(label(window, id).as_deref(), Some(expected));
        }
    })
    .unwrap();
}
