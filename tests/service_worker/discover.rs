use crate::fixture::{Fixture, Library, WAIT, album, event, song, synced};
use chrono::{Duration, Utc};
use sonora::{
    discover::{Dismissal, Rediscover, Shelf},
    model::Album,
    service::{Command, Event},
};
use std::sync::Arc;

fn played(id: &str, plays: u64, days_ago: i64) -> Album {
    Album {
        play_count: Some(plays),
        played: Some((Utc::now() - Duration::days(days_ago)).to_rfc3339()),
        ..album(id)
    }
}

fn shelves(worker: &sonora::service::test_support::Worker, request: u64) -> Arc<Rediscover> {
    worker.service().send(Command::Discover(request));
    let Event::Discover(_, shelves) = event(
        worker,
        |event| matches!(event, Event::Discover(id, _) if *id == request),
    ) else {
        unreachable!()
    };
    shelves
}

fn ids(shelf: &Shelf) -> Vec<&str> {
    shelf
        .ranked
        .iter()
        .map(|pick| pick.album.id.as_str())
        .collect()
}

fn library() -> Library {
    let mut albums = (0..10)
        .map(|ix| played(&format!("filler-{ix}"), 1, 300))
        .collect::<Vec<_>>();
    albums.push(played("heavy", 50, 200));
    albums.push(played("fresh", 60, 10));
    albums.push(played("loved", 1, 300));
    albums.push(played("also-loved", 1, 310));
    let mut tracks = vec![song("song", "album"), song("other-song", "other-album")];
    for (id, album) in [
        ("loved-1", "loved"),
        ("loved-2", "loved"),
        ("also-1", "also-loved"),
        ("also-2", "also-loved"),
    ] {
        let mut track = song(id, album);
        track.starred = Some("2025-01-01T00:00:00Z".into());
        tracks.push(track);
    }
    Library {
        albums,
        tracks,
        ..Library::default()
    }
}

#[test]
fn rediscover_shelves_follow_synced_plays_and_favorites() {
    let fixture = Fixture::new(library());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let found = shelves(&worker, 1);
    assert_eq!(ids(&found.old_rotation), ["heavy"]);
    assert_eq!(ids(&found.forgotten_favorites), ["also-loved", "loved"]);
    assert_eq!(
        found.old_rotation.ranked[0].reason_text(Utc::now()),
        "50 plays · last played 6 months ago"
    );

    fixture.edit_library(|library| {
        for album in &mut library.albums {
            if ["loved", "also-loved", "heavy"].contains(&album.id.as_str()) {
                *album = played(&album.id, album.play_count.unwrap_or(0) + 1, 0);
            }
        }
    });
    worker.service().send(Command::Refresh);
    synced(&worker);
    let refreshed = shelves(&worker, 2);
    assert!(refreshed.old_rotation.is_empty());
    assert!(refreshed.forgotten_favorites.is_empty());
    assert!(refreshed.forgotten_favorites.today.is_empty());
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn an_empty_library_has_empty_shelves() {
    let fixture = Fixture::new(Library {
        albums: vec![],
        tracks: vec![],
        ..Library::default()
    });
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let found = shelves(&worker, 7);
    assert!(found.old_rotation.is_empty() && found.forgotten_favorites.is_empty());
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn dismissed_albums_leave_their_shelves_and_stay_hidden_after_restart() {
    let fixture = Fixture::new(library());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::HideDiscover("heavy".into(), Dismissal::NotNow));
    worker
        .service()
        .send(Command::HideDiscover("loved".into(), Dismissal::Never));
    let found = shelves(&worker, 1);
    assert!(found.old_rotation.is_empty());
    assert_eq!(ids(&found.forgotten_favorites), ["also-loved"]);
    worker.shutdown(WAIT).unwrap();

    let mut restarted = fixture.start();
    restarted.wait_ready(WAIT).unwrap();
    synced(&restarted);
    let found = shelves(&restarted, 2);
    assert!(found.old_rotation.is_empty());
    assert_eq!(ids(&found.forgotten_favorites), ["also-loved"]);
    restarted.shutdown(WAIT).unwrap();
    fixture.finish();
}
