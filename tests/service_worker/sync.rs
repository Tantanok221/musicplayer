use crate::fixture::{Fixture, Library, WAIT, album, event, song, view};
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
    assert_eq!(catalog.artist_releases("Main", "main").unwrap().len(), 2);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
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
