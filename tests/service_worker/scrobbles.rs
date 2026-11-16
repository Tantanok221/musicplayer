use crate::fixture::{Fixture, Library, Request, WAIT, event, synced, view};
use sonora::{
    listening::ListenReport,
    service::{Command, Connection, Event},
};

fn report(f: &Fixture, id: u64) -> ListenReport {
    ListenReport {
        id,
        account: f.account.namespace(),
        track_id: "song".into(),
        started_ms: 1700000000000 + id as i64,
        submission: true,
    }
}
fn request(id: u64) -> Request {
    Request::new(
        "scrobble.view",
        &[
            ("id", "song"),
            ("submission", "true"),
            ("time", &(1700000000000 + id).to_string()),
        ],
    )
}
fn state(f: &Fixture, id: u64) -> String {
    rusqlite::Connection::open(
        f.paths
            .data
            .join(format!("{}.sqlite", f.account.namespace())),
    )
    .unwrap()
    .query_row(
        "SELECT state FROM listen_submissions WHERE id=?",
        [id.to_string()],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn listens_send_once_rejection_is_terminal_and_restart_does_not_duplicate_inflight() {
    let f = Fixture::new(Library::default());
    let mut w = f.start();
    synced(&w);
    f.expect_mutation(request(1), true);
    w.service().send(Command::Scrobble(report(&f, 1)));
    event(&w, |e| matches!(e, Event::TrackUpdated(_)));
    assert_eq!(state(&f, 1), "confirmed");
    w.service().send(Command::Scrobble(report(&f, 1)));
    view(&w);
    assert_eq!(f.requests().iter().filter(|r| **r == request(1)).count(), 1);
    f.expect_mutation(request(2), false);
    w.service().send(Command::Scrobble(report(&f, 2)));
    event(
        &w,
        |e| matches!(e, Event::Error(s) if s.contains("will not be sent again")),
    );
    assert_eq!(state(&f, 2), "uncertain");
    w.service().send(Command::Scrobble(report(&f, 2)));
    view(&w);
    f.edit_library(|l| l.hold_scrobble = true);
    f.expect_mutation(request(3), true);
    w.service().send(Command::Scrobble(report(&f, 3)));
    f.wait_request(&request(3));
    assert_eq!(state(&f, 3), "attempted");
    w.shutdown(WAIT).unwrap();
    f.resume_scrobble();
    let mut restarted = f.start();
    synced(&restarted);
    view(&restarted);
    for id in [1, 2, 3] {
        assert_eq!(
            f.requests().iter().filter(|r| **r == request(id)).count(),
            1
        );
    }
    assert_eq!(state(&f, 3), "attempted");
    restarted.shutdown(WAIT).unwrap();
    f.finish();
}

#[test]
fn offline_listen_stays_pending_until_reconnected_then_sends_once() {
    offline_listen(false);
    offline_listen(true);
}

fn offline_listen(initially_connected: bool) {
    let f = Fixture::new(Library {
        offline: !initially_connected,
        ..Default::default()
    });
    f.seed();
    let mut w = f.start();
    if initially_connected {
        synced(&w);
        f.edit_library(|l| l.offline = true);
        w.service().send(Command::CheckConnection(true));
    }
    event(&w, |e| {
        matches!(e, Event::Connection(Connection::Offline(_)))
    });
    w.service().send(Command::Scrobble(report(&f, 4)));
    view(&w);
    assert_eq!(state(&f, 4), "pending");
    assert!(!f.requests().iter().any(|r| r.endpoint == "scrobble.view"));
    f.expect_mutation(request(4), true);
    f.edit_library(|l| l.offline = false);
    w.service().send(Command::CheckConnection(true));
    let mut confirmed = false;
    let mut refreshed = false;
    let deadline = std::time::Instant::now() + WAIT;
    while !confirmed || !refreshed {
        match w.events().recv_deadline(deadline).unwrap() {
            Event::TrackUpdated(_) => confirmed = true,
            Event::Status(s) if s.starts_with("Up to date") => refreshed = true,
            Event::Error(_) => panic!("unexpected reconnect error"),
            _ => {}
        }
    }
    assert_eq!(state(&f, 4), "confirmed");
    w.shutdown(WAIT).unwrap();
    assert_eq!(f.requests().iter().filter(|r| **r == request(4)).count(), 1);
    f.finish();
}
