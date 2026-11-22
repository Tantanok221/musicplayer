use crate::fixture::{Fixture, Library, WAIT, album, event, song, view};
use serde_json::{Value, json};
use sonora::{
    model::{NamedId, Playlist},
    service::{Command, Event},
};

#[test]
fn interrupted_sync_placeholder_albums_belong_to_album_artists() {
    let mut tracks = (0..501)
        .map(|index| song(&format!("track-{index}"), "main-album"))
        .collect::<Vec<_>>();
    for track in &mut tracks {
        track.artist = "Guest".into();
        track.artist_id = "guest".into();
        track.album_artist = "Main".into();
        track.album_artists = vec![NamedId {
            id: "main".into(),
            name: "Main".into(),
        }];
    }
    tracks[1].album_id = "unknown-owner".into();
    tracks[1].album_artists.clear();
    tracks[2].album_id = "performer-fallback".into();
    tracks[2].album_artists.clear();
    tracks[2].album_artist.clear();
    tracks[3].album_id = "joint-album".into();
    tracks[3].album_artist = "Main feat. Other".into();
    tracks[3].album_artists.push(NamedId {
        id: "other".into(),
        name: "Other".into(),
    });
    tracks[4].album_id = "display-performer".into();
    tracks[4].artist = "Raw Performer".into();
    tracks[4].artist_id = "display-performer-id".into();
    tracks[4].display_artist = "Display Performer".into();
    tracks[4].album_artist.clear();
    tracks[4].album_artists.clear();
    let fixture = Fixture::new(Library {
        tracks,
        albums: vec![],
        fail_song_offset: Some(500),
        ..Default::default()
    });
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    event(
        &worker,
        |event| matches!(event, Event::Error(message) if message.contains("Fixture later page failed")),
    );
    let catalog = fixture.catalog();
    let guest = catalog.artist_releases("Guest", "guest").unwrap();
    assert_eq!(
        guest
            .iter()
            .map(|album| album.id.as_str())
            .collect::<Vec<_>>(),
        ["performer-fallback"],
        "guest performers must not own another artist's placeholder albums"
    );
    assert_eq!(
        catalog.album("main-album").unwrap().unwrap().artist_id,
        "main"
    );
    assert!(
        catalog
            .album("unknown-owner")
            .unwrap()
            .unwrap()
            .artist_id
            .is_empty()
    );
    assert_eq!(
        catalog
            .album("performer-fallback")
            .unwrap()
            .unwrap()
            .artist_id,
        "guest"
    );
    let joint = catalog.album("joint-album").unwrap().unwrap();
    assert!(
        joint.artist_id.is_empty(),
        "a joint label must not borrow a solo artist ID"
    );
    let joint_credit = joint
        .artist_credits()
        .into_iter()
        .find(|credit| credit.name == "Main feat. Other")
        .unwrap();
    assert_eq!(
        catalog
            .artist_releases(&joint_credit.name, &joint_credit.id)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(joint.artists.len(), 2);
    assert_eq!(catalog.artist_releases("Main", "main").unwrap().len(), 3);
    let display_releases = catalog
        .artist_releases("Display Performer", "display-performer-id")
        .unwrap();
    assert_eq!(
        display_releases.len(),
        1,
        "display-name performer page must include its placeholder album"
    );
    assert_eq!(display_releases[0].id, "display-performer");
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn malformed_playlist_lists_fail_sync_without_deleting_cached_playlists() {
    for value in [
        json!("oops"),
        json!(42),
        json!(false),
        json!({"id":"single"}),
    ] {
        assert_malformed_playlists_preserve_cache(
            json!({"status":"ok","playlists":{"playlist":value}}),
        );
    }
}

#[test]
fn malformed_playlist_containers_fail_sync_without_deleting_cache() {
    for response in [
        json!({"status":"ok"}),
        json!({"status":"ok","playlists":null}),
        json!({"status":"ok","playlists":[]}),
    ] {
        assert_malformed_playlists_preserve_cache(response);
    }
}

#[test]
fn partial_playlist_entries_fail_sync_without_deleting_cache() {
    for invalid in [json!({}), json!({"id":""}), json!({"id":"fresh-playlist"})] {
        assert_malformed_playlists_preserve_cache(json!({"status":"ok","playlists":{"playlist":[
            {"id":"fresh-playlist","name":"Fresh playlist"}, invalid
        ]}}));
    }
}

fn assert_malformed_playlists_preserve_cache(response: Value) {
    let case = format!("getPlaylists {response}");
    assert_incomplete_sync_preserves_cache(
        Library {
            playlists_response: Some(response),
            ..Library::default()
        },
        &case,
        "getPlaylists",
    );
}

#[test]
fn malformed_scan_status_at_either_sync_boundary_preserves_cache() {
    for (phase, before) in [("before", true), ("after", false)] {
        for response in [
            json!({"status":"ok"}),
            json!({"status":"ok","scanStatus":null}),
            json!({"status":"ok","scanStatus":{}}),
            json!({"status":"ok","scanStatus":{"scanning":"false"}}),
        ] {
            let case = format!("getScanStatus {phase} sync: {response}");
            let mut replies = std::collections::VecDeque::from([response]);
            if !before {
                replies.push_front(json!({"status":"ok","scanStatus":{"scanning":false}}));
            }
            assert_incomplete_sync_preserves_cache(
                Library {
                    scan_status_responses: replies,
                    ..Library::default()
                },
                &case,
                "getScanStatus",
            );
        }
    }
}

#[test]
fn empty_playlist_lists_complete_sync_and_remove_cached_playlists() {
    for response in [json!({}), json!({"playlist":[]}), json!({"playlist":null})] {
        let fixture = Fixture::new(Library {
            playlists_response: Some(json!({"status":"ok","playlists":response})),
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
fn malformed_search3_responses_fail_sync_without_deleting_cached_music() {
    for kind in ["song", "album"] {
        for response in [
            json!({"status":"ok"}),
            json!({"status":"ok","searchResult3":null}),
            json!({"status":"ok","searchResult3":[]}),
            json!({"status":"ok","searchResult3":{kind:{"id":"single"}}}),
            json!({"status":"ok","searchResult3":{kind:"invalid"}}),
            json!({"status":"ok","searchResult3":{kind:42}}),
            json!({"status":"ok","searchResult3":{kind:false}}),
        ] {
            let case = format!("search3 {kind} {response}");
            assert_incomplete_sync_preserves_cache(
                Library {
                    sync_response: Some((kind, response)),
                    ..Library::default()
                },
                &case,
                &format!("search3 {kind}"),
            );
        }
    }
}

#[test]
fn partial_search3_entries_fail_sync_without_deleting_cache() {
    for kind in ["song", "album"] {
        for invalid in [json!({}), json!({"id":"fresh"})] {
            let response = json!({"status":"ok","searchResult3":{kind:[{"id":"fresh"},invalid]}});
            let case = format!("search3 {kind} {response}");
            assert_incomplete_sync_preserves_cache(
                Library {
                    sync_response: Some((kind, response)),
                    ..Library::default()
                },
                &case,
                "Library changed during sync",
            );
        }
    }
}

fn assert_incomplete_sync_preserves_cache(library: Library, case: &str, error: &str) {
    let fixture = Fixture::new(library);
    let mut catalog = fixture.catalog();
    let mut track = song("cached", "cached-album");
    track.artist = "Cached Artist".into();
    track.artist_id = "cached-artist".into();
    track.album_artist = track.artist.clone();
    let mut cached_album = album("cached-album");
    cached_album.artist = track.artist.clone();
    cached_album.artist_id = track.artist_id.clone();
    catalog.put_tracks(std::slice::from_ref(&track), 1).unwrap();
    catalog.put_albums(&[cached_album], 1).unwrap();
    catalog
        .save_playlist(&Playlist {
            id: "cached-playlist".into(),
            name: "Offline playlist".into(),
            song_count: 1,
            entry: vec![track.into()].into(),
            ..Default::default()
        })
        .unwrap();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    let outcome = event(&worker, |event| {
        matches!(event, Event::Error(_))
            || matches!(event, Event::Status(message) if message.starts_with("Up to date"))
    });
    let cached = [
        (
            "track",
            !catalog.by_ids(&["cached".into()]).unwrap().is_empty(),
        ),
        ("album", catalog.album("cached-album").unwrap().is_some()),
        (
            "artist releases",
            !catalog
                .artist_releases("Cached Artist", "cached-artist")
                .unwrap()
                .is_empty(),
        ),
        (
            "playlist with offline track",
            catalog
                .playlist("cached-playlist")
                .unwrap()
                .is_some_and(|playlist| playlist.entry.iter().any(|track| track.id == "cached")),
        ),
    ];
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
    let missing = cached
        .into_iter()
        .filter_map(|(record, present)| (!present).then_some(record))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "{case} deleted cached {missing:?}: {outcome:?}"
    );
    assert!(
        matches!(&outcome, Event::Error(message)
        if message.contains(error) && message.contains("Cached music is still available")),
        "{case} did not report an incomplete refresh: {outcome:?}"
    );
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
