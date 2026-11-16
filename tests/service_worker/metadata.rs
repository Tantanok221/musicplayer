use crate::fixture::{Fixture, Library, Request, WAIT, event, synced};
use sonora::service::{Command, Event};

#[test]
fn worker_metadata_uses_injected_musicbrainz_and_listenbrainz_endpoints() {
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
    fixture.wait_request(&Request::new("listens/mb-artist", &[]));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}
