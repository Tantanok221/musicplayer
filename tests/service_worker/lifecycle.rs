use crate::fixture::{Fixture, Library, Request, WAIT, synced};
use sonora::service::{Command, Event};

#[test]
fn readiness_preserves_playback_published_before_catalog_startup() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start_paused();
    worker.wait_ready(WAIT).unwrap();
    assert!(matches!(
        worker.events().recv_timeout(WAIT).unwrap(),
        Event::Playback(_)
    ));
    crate::fixture::event(&worker, |event| matches!(event, Event::Playlists(_)));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn shutdown_finishes_the_worker_and_runtime_with_metadata_still_pending() {
    let fixture = Fixture::new(Library {
        hold_metadata: true,
        ..Library::default()
    });
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Discography(
        "Fixture Artist".into(),
        "artist".into(),
    ));
    fixture.wait_request(&Request::new("metadata/release", &[]));
    worker.shutdown(WAIT).unwrap();
    assert!(
        worker.events().is_disconnected(),
        "no timer/request may retain the worker's event sender"
    );
    fixture.finish();
}
