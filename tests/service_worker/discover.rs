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
    let mut albums = (0..30)
        .map(|ix| played(&format!("filler-{ix}"), 1, 300))
        .collect::<Vec<_>>();
    albums.push(played("heavy", 50, 200));
    albums.push(played("steady", 46, 250));
    albums.push(played("kept", 45, 220));
    albums.push(played("fresh", 60, 10));
    let mut favorite = played("favorite", 1, 300);
    favorite.starred = Some("2025-01-01T00:00:00Z".into());
    albums.push(favorite);
    let mut tracks = vec![song("song", "album")];
    for id in ["favorite-1", "favorite-2"] {
        let mut track = song(id, "favorite");
        track.starred = Some("2025-01-01T00:00:00Z".into());
        tracks.push(track);
    }
    Library {
        albums,
        tracks,
        ..Library::default()
    }
}

fn lane(found: &Rediscover) -> Vec<&str> {
    found
        .lane
        .iter()
        .flat_map(|season| ids(&season.shelf))
        .collect()
}

#[test]
fn memory_lane_follows_synced_plays() {
    let fixture = Fixture::new(library());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let found = shelves(&worker, 1);
    assert_eq!(ids(&found.old_rotation), ["heavy", "steady", "kept"]);
    let mut seasons = lane(&found);
    seasons.sort();
    assert_eq!(
        seasons,
        ["heavy", "kept", "steady"],
        "favorites alone are not picked"
    );
    assert_eq!(
        found.old_rotation.ranked[0].reason_text(Utc::now()),
        "50 plays · last played 6 months ago"
    );

    fixture.edit_library(|library| {
        for album in &mut library.albums {
            if ["heavy", "steady", "kept"].contains(&album.id.as_str()) {
                *album = played(&album.id, album.play_count.unwrap_or(0) + 1, 0);
            }
        }
    });
    worker.service().send(Command::Refresh);
    synced(&worker);
    let refreshed = shelves(&worker, 2);
    assert!(refreshed.old_rotation.is_empty() && refreshed.lane.is_empty());
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
    assert!(found.old_rotation.is_empty() && found.lane.is_empty());
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
        .send(Command::HideDiscover("steady".into(), Dismissal::Never));
    let found = shelves(&worker, 1);
    assert_eq!(ids(&found.old_rotation), ["kept"]);
    assert_eq!(lane(&found), ["kept"]);
    worker.shutdown(WAIT).unwrap();

    let mut restarted = fixture.start();
    restarted.wait_ready(WAIT).unwrap();
    synced(&restarted);
    let found = shelves(&restarted, 2);
    assert_eq!(ids(&found.old_rotation), ["kept"]);
    restarted.shutdown(WAIT).unwrap();
    fixture.finish();
}
