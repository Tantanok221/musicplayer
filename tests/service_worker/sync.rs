use crate::fixture::{Fixture, Library, WAIT, album, event, song, view};
use serde_json::json;
use sonora::{
    model::Playlist,
    service::{Command, Event},
};

#[test]
fn malformed_playlist_lists_fail_sync_without_deleting_cached_playlists() {
    for value in [
        json!("oops"),
        json!(42),
        json!(false),
        json!({"id":"single"}),
    ] {
        let fixture = Fixture::new(Library {
            playlists_response: Some(json!({"playlist":value})),
            ..Library::default()
        });
        fixture
            .catalog()
            .save_playlist_summaries(
                &[Playlist {
                    id: "cached-playlist".into(),
                    name: "Offline playlist".into(),
                    ..Default::default()
                }],
                true,
            )
            .unwrap();
        let mut worker = fixture.start();
        worker.wait_ready(WAIT).unwrap();
        let outcome = event(&worker, |event| {
            matches!(event, Event::Error(_))
                || matches!(event, Event::Status(message) if message.starts_with("Up to date"))
        });
        assert!(
            fixture
                .catalog()
                .playlist("cached-playlist")
                .unwrap()
                .is_some(),
            "malformed playlist list must not delete cached playlists: {outcome:?}"
        );
        assert!(matches!(outcome, Event::Error(message)
            if message.contains("getPlaylists") && message.contains("playlist")
                && message.contains("Cached music is still available")));
        worker.shutdown(WAIT).unwrap();
        fixture.finish();
    }
}

#[test]
fn empty_playlist_lists_complete_sync_and_remove_cached_playlists() {
    for response in [json!({}), json!({"playlist":[]}), json!({"playlist":null})] {
        let fixture = Fixture::new(Library {
            playlists_response: Some(response),
            ..Library::default()
        });
        fixture
            .catalog()
            .save_playlist_summaries(
                &[Playlist {
                    id: "cached-playlist".into(),
                    ..Default::default()
                }],
                true,
            )
            .unwrap();
        let mut worker = fixture.start();
        worker.wait_ready(WAIT).unwrap();
        event(
            &worker,
            |event| matches!(event, Event::Status(message) if message.starts_with("Up to date")),
        );
        assert!(
            fixture
                .catalog()
                .playlist("cached-playlist")
                .unwrap()
                .is_none()
        );
        worker.shutdown(WAIT).unwrap();
        fixture.finish();
    }
}

#[test]
fn malformed_search3_lists_fail_sync_without_deleting_cached_music() {
    for kind in ["song", "album"] {
        for value in [
            json!({"id":"single"}),
            json!("invalid"),
            json!(42),
            json!(false),
        ] {
            let fixture = Fixture::new(Library {
                tracks: vec![],
                albums: vec![],
                sync_list: Some((kind, value)),
                ..Library::default()
            });
            {
                let mut catalog = fixture.catalog();
                catalog
                    .put_tracks(&[song("cached", "cached-album")], 1)
                    .unwrap();
                catalog.put_albums(&[album("cached-album")], 1).unwrap();
            }
            let mut worker = fixture.start();
            worker.wait_ready(WAIT).unwrap();
            let outcome = event(&worker, |event| {
                matches!(event, Event::Error(_))
                    || matches!(event, Event::Status(message) if message.starts_with("Up to date"))
            });
            let catalog = fixture.catalog();
            assert_eq!(
                catalog.by_ids(&["cached".into()]).unwrap().len(),
                1,
                "malformed {kind} must not delete cached tracks"
            );
            assert!(catalog.album("cached-album").unwrap().is_some());
            assert!(matches!(outcome, Event::Error(message)
                if message.contains("search3") && message.contains(kind)
                    && message.contains("Cached music is still available")));
            worker.shutdown(WAIT).unwrap();
            fixture.finish();
        }
    }
}

#[test]
fn interrupted_sync_keeps_cached_music_then_a_complete_refresh_reconciles() {
    let fixture = Fixture::new(Library {
        tracks: (0..501)
            .map(|index| song(&format!("fresh-{index}"), "album"))
            .collect(),
        albums: vec![album("album")],
        fail_song_offset: Some(500),
        ..Library::default()
    });
    {
        let mut catalog = fixture.catalog();
        catalog
            .put_tracks(&[song("cached", "cached-album")], 1)
            .unwrap();
        catalog.put_albums(&[album("cached-album")], 1).unwrap();
        catalog
            .save_playlist_summaries(
                &[Playlist {
                    id: "cached-playlist".into(),
                    name: "Offline playlist".into(),
                    ..Default::default()
                }],
                true,
            )
            .unwrap();
    }
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    event(
        &worker,
        |event| matches!(event, Event::Error(message) if message.contains("Fixture later page failed") && message.contains("Cached music is still available")),
    );
    let partial = view(&worker);
    assert_eq!(
        partial.track_count, 501,
        "500 imported songs plus the cached song"
    );
    let catalog = fixture.catalog();
    assert_eq!(
        catalog
            .by_ids(&["fresh-0".into(), "fresh-499".into()])
            .unwrap()
            .len(),
        2
    );
    assert_eq!(catalog.by_ids(&["cached".into()]).unwrap().len(), 1);
    assert!(catalog.album("cached-album").unwrap().is_some());
    assert!(catalog.playlist("cached-playlist").unwrap().is_some());

    // A successful pass during a server scan still must not remove cached data.
    fixture.edit_library(|library| {
        library.fail_song_offset = None;
        library.scanning = true;
    });
    worker.service().send(Command::Refresh);
    event(
        &worker,
        |event| matches!(event, Event::Status(message) if message.starts_with("Server scan in progress")),
    );
    assert_eq!(view(&worker).track_count, 502);
    assert!(catalog.album("cached-album").unwrap().is_some());
    assert!(catalog.playlist("cached-playlist").unwrap().is_some());

    fixture.edit_library(|library| library.scanning = false);
    worker.service().send(Command::Refresh);
    event(
        &worker,
        |event| matches!(event, Event::Status(message) if message == "Up to date · 501 tracks · 1 albums"),
    );
    let complete = view(&worker);
    assert_eq!(complete.track_count, 501);
    assert_eq!(complete.album_count, 1);
    assert!(catalog.by_ids(&["cached".into()]).unwrap().is_empty());
    assert!(catalog.album("cached-album").unwrap().is_none());
    assert!(catalog.playlist("cached-playlist").unwrap().is_none());
    assert_eq!(catalog.by_ids(&["fresh-500".into()]).unwrap().len(), 1);
    worker.shutdown(WAIT).unwrap();
    assert_eq!(fixture.catalog().count().unwrap(), 501);
    fixture.finish();
}
