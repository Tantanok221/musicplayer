use crate::fixture::{Fixture, LASTFM_KEY, Library, WAIT, album, event, song, synced};
use chrono::{Duration, Utc};
use sonora::{
    discover::{Dismissal, HistoryStatus, Rediscover, Shelf, seasons_between},
    model::{Album, Preferences},
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

fn listen_as(worker: &sonora::service::test_support::Worker, user: &str) {
    worker.service().send(Command::Preferences(Preferences {
        lastfm_user: user.into(),
        ..Default::default()
    }));
}

/// Asks for Discover and waits until its Last.fm history has finished loading.
fn settled(worker: &sonora::service::test_support::Worker, request: u64) -> Arc<Rediscover> {
    worker.service().send(Command::Discover(request));
    loop {
        let Event::Discover(_, shelves) = event(
            worker,
            |event| matches!(event, Event::Discover(id, _) if *id == request),
        ) else {
            unreachable!()
        };
        if !matches!(shelves.history, HistoryStatus::Loading { .. }) {
            return shelves;
        }
    }
}

fn lastfm_requests(fixture: &Fixture) -> Vec<String> {
    fixture
        .requests()
        .into_iter()
        .filter(|request| request.endpoint.starts_with("lastfm/"))
        .map(|request| request.endpoint)
        .collect()
}

fn history_library() -> (Library, Vec<sonora::discover::SeasonSpan>) {
    let registered = Utc::now() - Duration::days(500);
    let spans = seasons_between(registered, Utc::now());
    let mut albums = vec![
        album("old-love"),
        album("single"),
        album("other"),
        played("still-playing", 3, 5),
    ];
    albums[1].name = "Lone Album (Remastered)".into();
    let library = Library {
        albums,
        tracks: vec![song("song", "old-love")],
        lastfm_registered: registered.timestamp(),
        lastfm_charts: vec![
            (
                spans[0].from,
                vec![
                    ("Fixture Artist", "Release old-love", 30),
                    ("Fixture Artist", "Release still-playing", 40),
                    ("Somebody Else", "Release other", 25),
                ],
            ),
            (spans[1].from, vec![("Fixture Artist", "Lone Album", 12)]),
        ],
        ..Library::default()
    };
    (library, spans)
}

#[test]
fn memory_lane_reaches_back_through_lastfm_history_and_keeps_it() {
    let (library, spans) = history_library();
    let fixture = Fixture::new(library);
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    listen_as(&worker, "listener");
    worker.service().send(Command::Discover(1));
    let Event::Discover(_, first) = event(&worker, |event| matches!(event, Event::Discover(1, _)))
    else {
        unreachable!()
    };
    assert!(matches!(first.history, HistoryStatus::Loading { .. }));
    let found = settled(&worker, 2);
    assert_eq!(found.history, HistoryStatus::Ready);
    let lane = found
        .lane
        .iter()
        .map(|season| (season.label.as_str(), ids(&season.shelf)))
        .collect::<Vec<_>>();
    assert_eq!(
        lane,
        [
            (spans[1].label.as_str(), vec!["single"]),
            (spans[0].label.as_str(), vec!["old-love"]),
        ],
        "still-playing was played this month, and the other artist's album is not ours"
    );
    assert_eq!(
        found.lane[1].shelf.ranked[0].reason_text(Utc::now()),
        "30 plays that season"
    );
    let mut expected = vec!["lastfm/getinfo".to_string()];
    expected.extend(spans.iter().map(|_| "lastfm/chart".to_string()));
    assert_eq!(lastfm_requests(&fixture), expected);
    let charts = fixture
        .requests()
        .into_iter()
        .filter(|request| request.endpoint == "lastfm/chart")
        .map(|request| request.params["from"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        charts,
        spans
            .iter()
            .map(|span| span.from.to_string())
            .collect::<Vec<_>>()
    );

    let again = settled(&worker, 3);
    assert_eq!(again.lane.len(), 2);
    worker.shutdown(WAIT).unwrap();
    let mut restarted = fixture.start();
    restarted.wait_ready(WAIT).unwrap();
    synced(&restarted);
    let cached = settled(&restarted, 4);
    assert_eq!(cached.history, HistoryStatus::Ready);
    assert_eq!(cached.lane.len(), 2);
    assert_eq!(
        lastfm_requests(&fixture).len(),
        expected.len(),
        "finished seasons are not fetched again"
    );
    restarted.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn a_private_or_missing_lastfm_profile_says_so_and_recovers() {
    let (library, _) = history_library();
    let fixture = Fixture::new(Library {
        lastfm_error: Some(17),
        ..library
    });
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    listen_as(&worker, "listener");
    let private = settled(&worker, 1);
    assert_eq!(
        private.history,
        HistoryStatus::Failed("listener's Last.fm listening history is private".into())
    );
    assert!(private.lane.is_empty());

    fixture.edit_library(|library| library.lastfm_error = Some(6));
    let missing = settled(&worker, 2);
    assert_eq!(
        missing.history,
        HistoryStatus::Failed("Last.fm has no user named listener".into())
    );

    fixture.edit_library(|library| library.lastfm_error = None);
    let recovered = settled(&worker, 3);
    assert_eq!(recovered.history, HistoryStatus::Ready);
    assert_eq!(recovered.lane.len(), 2);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn without_a_username_or_api_key_lastfm_is_never_asked() {
    let (library, _) = history_library();
    let fixture = Fixture::new(Library {
        lastfm_key: false,
        ..library
    });
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    assert_eq!(settled(&worker, 1).history, HistoryStatus::Off);
    listen_as(&worker, "listener");
    assert_eq!(settled(&worker, 2).history, HistoryStatus::Unavailable);
    assert!(lastfm_requests(&fixture).is_empty());
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn a_lastfm_api_key_entered_in_settings_is_used_and_kept_privately() {
    let (library, _) = history_library();
    let fixture = Fixture::new(Library {
        lastfm_key: false,
        ..library
    });
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    listen_as(&worker, "listener");
    assert_eq!(settled(&worker, 1).history, HistoryStatus::Unavailable);

    worker
        .service()
        .send(Command::LastFmKey(format!("  {LASTFM_KEY} \n")));
    let found = settled(&worker, 2);
    assert_eq!(found.history, HistoryStatus::Ready);
    assert_eq!(found.lane.len(), 2);
    assert_eq!(
        fixture.paths.lastfm_key().unwrap().as_deref(),
        Some(LASTFM_KEY)
    );
    let in_catalog = std::fs::read_dir(&fixture.paths.data)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().is_file())
        .any(|entry| {
            std::fs::read(entry.path())
                .unwrap()
                .windows(LASTFM_KEY.len())
                .any(|bytes| bytes == LASTFM_KEY.as_bytes())
        });
    assert!(!in_catalog, "the key stays out of the catalog");

    worker.service().send(Command::LastFmKey(" ".into()));
    assert_eq!(
        settled(&worker, 3).history,
        HistoryStatus::Ready,
        "an empty field keeps the saved key"
    );
    worker.shutdown(WAIT).unwrap();
    let mut restarted = fixture.start();
    restarted.wait_ready(WAIT).unwrap();
    synced(&restarted);
    assert_eq!(settled(&restarted, 4).history, HistoryStatus::Ready);
    restarted.shutdown(WAIT).unwrap();
    fixture.finish();
}
