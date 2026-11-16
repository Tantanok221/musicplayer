use crate::fixture::{Fixture, Library, Request, WAIT, event, synced};
use sonora::{
    artists::cache_key,
    discography::Discography,
    service::{Command, Event},
};

#[test]
fn worker_metadata_uses_only_the_injected_musicbrainz_endpoint() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Discography(
        "Fixture Artist".into(),
        "artist".into(),
    ));
    let Event::Discography(_, result) = event(
        &worker,
        |event| matches!(event, Event::Discography(_, result) if !result.loading),
    ) else {
        unreachable!()
    };
    assert_eq!(result.missing.len(), 1);
    assert_eq!(result.missing[0].id, "missing-release");
    fixture.wait_request(&Request::new("metadata/release", &[]));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn missing_releases_are_listed_newest_first() {
    let fixture = Fixture::new(Library {
        releases: vec![
            ("popular-old", "Popular Old", "1999"),
            ("undated", "Undated", ""),
            ("newest", "Newest", "2024-03-01"),
            ("newer", "Newer", "2020-05"),
        ],
        ..Library::default()
    });
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Discography(
        "Fixture Artist".into(),
        "artist".into(),
    ));
    let Event::Discography(_, result) = event(
        &worker,
        |event| matches!(event, Event::Discography(_, result) if !result.loading),
    ) else {
        unreachable!()
    };
    assert_eq!(
        result
            .missing
            .iter()
            .map(|release| release.id.as_str())
            .collect::<Vec<_>>(),
        ["newest", "newer", "popular-old", "undated"]
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

fn missing_count(fixture: &Fixture, wanted: usize) {
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Discography(
        "Fixture Artist".into(),
        "artist".into(),
    ));
    event(
        &worker,
        |event| matches!(event, Event::Discography(_, result) if !result.loading && result.missing.len() == wanted),
    );
    worker.shutdown(WAIT).unwrap();
}

fn expire_discography(fixture: &Fixture) {
    let key = format!("discography:v1:{}", cache_key("Fixture Artist", "artist"));
    let catalog = fixture.catalog();
    let mut cached: Option<Discography> = catalog.state(&key).unwrap();
    cached.as_mut().unwrap().fetched_ms = 1;
    catalog.set_state(&key, &cached).unwrap();
    fixture.edit_library(|library| {
        library
            .releases
            .push(("added-release", "Added Release", "2025-01-01"))
    });
}

fn count(fixture: &Fixture, endpoint: &str) -> usize {
    fixture
        .requests()
        .iter()
        .filter(|request| request.endpoint == endpoint)
        .count()
}

#[test]
fn artist_found_by_search_is_reused_when_the_discography_refreshes() {
    let fixture = Fixture::new(Library {
        artist_mbid: "",
        ..Library::default()
    });
    missing_count(&fixture, 1);
    assert_eq!(count(&fixture, "metadata/artist-search"), 1);
    expire_discography(&fixture);
    missing_count(&fixture, 2);
    assert_eq!(count(&fixture, "metadata/artist-search"), 1);
    assert_eq!(count(&fixture, "metadata/artist-check"), 1);
    assert_eq!(count(&fixture, "metadata/release"), 2);
    fixture.finish();
}

#[test]
fn prolific_artist_refresh_goes_straight_to_the_release_group_overview() {
    let fixture = Fixture::new(Library {
        prolific: true,
        ..Library::default()
    });
    missing_count(&fixture, 1);
    assert_eq!(count(&fixture, "metadata/release"), 1);
    assert_eq!(count(&fixture, "metadata/release-groups"), 1);
    expire_discography(&fixture);
    missing_count(&fixture, 2);
    assert_eq!(count(&fixture, "metadata/release"), 1);
    assert_eq!(count(&fixture, "metadata/release-groups"), 2);
    fixture.finish();
}
