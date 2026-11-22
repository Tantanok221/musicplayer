use crate::fixture::{Fixture, Library, WAIT, event, synced};
use serde_json::Value;
use sonora::{
    catalog::Catalog,
    discover::History,
    model::{Playlist, Preferences, SavedQueue},
    playback::PlayerCommand,
    service::{Command, Event},
};

const PREFERENCES: &str = include_str!("../fixtures/saved_state/preferences.json");
const QUEUE: &str = include_str!("../fixtures/saved_state/queue.json");
const PLAYLIST: &str = include_str!("../fixtures/saved_state/playlist.json");
const HISTORY: &str = include_str!("../fixtures/saved_state/history.json");

fn json(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap()
}

fn assert_saved_fields(actual: &Value, expected: &Value) {
    match expected {
        Value::Object(fields) => {
            for (key, value) in fields {
                assert!(actual.get(key).is_some(), "missing saved field: {key}");
                assert_saved_fields(&actual[key], value);
            }
        }
        Value::Array(values) => {
            assert_eq!(actual.as_array().unwrap().len(), values.len());
            for (actual, expected) in actual.as_array().unwrap().iter().zip(values) {
                assert_saved_fields(actual, expected);
            }
        }
        _ => assert_eq!(actual, expected),
    }
}

fn seed_state(catalog: &Catalog, key: &str, raw: &str) {
    catalog.set_state(key, &json(raw)).unwrap();
}

#[test]
fn golden_preferences_reach_the_worker_and_round_trip() {
    let fixture = Fixture::new(Library::default());
    seed_state(&fixture.catalog(), "preferences", PREFERENCES);
    let mut worker = fixture.start_paused();
    let (_, preferences) = worker.wait_ready(WAIT).unwrap();
    assert_eq!(preferences.volume, 0.25);
    assert_eq!(preferences.tabs.tabs[0].id, 7);
    assert_eq!(preferences.lastfm_user, "listener");
    assert_eq!(
        preferences.playlist_columns,
        sonora::playlists::PlaylistColumns::default()
    );
    assert_saved_fields(
        &serde_json::to_value(&preferences).unwrap(),
        &json(PREFERENCES),
    );
    worker.service().send(Command::Preferences(preferences));
    synced(&worker);
    worker.shutdown(WAIT).unwrap();
    let saved: Preferences = fixture.catalog().state("preferences").unwrap();
    assert_saved_fields(&serde_json::to_value(saved).unwrap(), &json(PREFERENCES));
    fixture.finish();
}

#[test]
fn golden_queue_restores_duplicate_occurrence_and_position() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    seed_state(&fixture.catalog(), "queue", QUEUE);
    let saved: SavedQueue = fixture.catalog().state("queue").unwrap();
    assert_eq!(serde_json::to_value(saved).unwrap(), json(QUEUE));
    let mut worker = fixture.start_paused();
    synced(&worker);
    worker
        .service()
        .send(Command::Player(PlayerCommand::ClearError));
    let Event::Playback(snapshot) = event(
        &worker,
        |event| matches!(event, Event::Playback(s) if s.queue.len() == 3),
    ) else {
        unreachable!()
    };
    assert_eq!(
        snapshot
            .queue
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        ["song", "other-song", "song"]
    );
    assert_eq!(snapshot.index, Some(2));
    assert_eq!(snapshot.position, 23.5);
    assert!(!snapshot.playing);
    worker.shutdown(WAIT).unwrap();
    let saved: SavedQueue = fixture.catalog().state("queue").unwrap();
    assert_eq!(serde_json::to_value(saved).unwrap(), json(QUEUE));
    fixture.finish();
}

#[test]
fn golden_catalog_playlist_and_history_round_trip_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.sqlite");
    drop(Catalog::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO playlists(id,data) VALUES('mix',?1)",
        [PLAYLIST],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO state(key,data) VALUES('lastfm-history:v1:listener',?1)",
        [HISTORY],
    )
    .unwrap();
    drop(conn);
    let catalog = Catalog::open(&path).unwrap();
    let playlist = catalog.playlist("mix").unwrap().unwrap();
    assert_eq!(playlist.entry.len(), 2);
    assert_eq!(playlist.entry[0].artist, "青葉市子");
    assert_saved_fields(&serde_json::to_value(&playlist).unwrap(), &json(PLAYLIST));
    let history: Option<History> = catalog.state("lastfm-history:v1:listener").unwrap();
    assert_eq!(
        history.as_ref().unwrap().seasons["2024-1"].albums[0].plays,
        17
    );
    assert_eq!(serde_json::to_value(&history).unwrap(), json(HISTORY));
    catalog.save_playlist(&playlist).unwrap();
    catalog
        .set_state("lastfm-history:v1:listener", &history)
        .unwrap();
    drop(catalog);
    let catalog = Catalog::open(&path).unwrap();
    assert_saved_fields(
        &serde_json::to_value(catalog.playlist("mix").unwrap()).unwrap(),
        &json(PLAYLIST),
    );
    assert_eq!(
        catalog
            .state::<Option<History>>("lastfm-history:v1:listener")
            .unwrap(),
        history
    );
    assert!(catalog.playlist("missing").unwrap().is_none());
}

#[test]
fn golden_defaults_and_missing_records_remain_compatible() {
    let catalog = Catalog::memory().unwrap();
    // Keep the old snapshot frozen: the new preference is an additive default.
    let legacy_defaults = include_str!("../fixtures/saved_state/preferences-default.json");
    let mut defaults = json(legacy_defaults);
    defaults["playlist_columns"] = json(include_str!(
        "../fixtures/saved_state/playlist-columns-default.json"
    ));
    for (actual, expected) in [
        (
            serde_json::to_value(catalog.state::<Preferences>("preferences").unwrap()).unwrap(),
            defaults.clone(),
        ),
        (
            serde_json::to_value(catalog.state::<SavedQueue>("queue").unwrap()).unwrap(),
            json(include_str!("../fixtures/saved_state/queue-empty.json")),
        ),
        (
            serde_json::to_value(Playlist::default()).unwrap(),
            json(include_str!("../fixtures/saved_state/playlist-empty.json")),
        ),
    ] {
        assert_eq!(actual, expected);
    }
    seed_state(&catalog, "preferences", legacy_defaults);
    assert_eq!(
        serde_json::to_value(catalog.state::<Preferences>("preferences").unwrap()).unwrap(),
        defaults
    );
    let empty = include_str!("../fixtures/saved_state/queue-empty.json");
    seed_state(&catalog, "queue", empty);
    assert_eq!(
        serde_json::to_value(catalog.state::<SavedQueue>("queue").unwrap()).unwrap(),
        json(empty)
    );
    assert!(
        catalog
            .state::<Option<History>>("lastfm-history:v1:listener")
            .unwrap()
            .is_none()
    );
    seed_state(&catalog, "preferences", "{}");
    assert_eq!(
        serde_json::to_value(catalog.state::<Preferences>("preferences").unwrap()).unwrap(),
        defaults
    );
}

#[test]
fn incompatible_saved_types_are_errors_not_defaults() {
    let catalog = Catalog::memory().unwrap();
    seed_state(&catalog, "preferences", r#"{"volume":"loud"}"#);
    assert!(catalog.state::<Preferences>("preferences").is_err());
    seed_state(
        &catalog,
        "queue",
        r#"{"ids":[],"index":null,"position":"later"}"#,
    );
    assert!(catalog.state::<SavedQueue>("queue").is_err());
    seed_state(&catalog, "lastfm-history:v1:listener", r#"{"seasons":[]}"#);
    assert!(
        catalog
            .state::<Option<History>>("lastfm-history:v1:listener")
            .is_err()
    );
}
