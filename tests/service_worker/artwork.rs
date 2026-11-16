use crate::fixture::{Fixture, Library, Request, WAIT, event, synced};
use sonora::{
    artwork::Priority,
    service::{Command, Event},
};
use std::time::{Duration, Instant};

fn artwork_request() -> Request {
    Request::new("getCoverArt.view", &[("id", "cover"), ("size", "512")])
}
fn clock(w: &sonora::service::test_support::Worker, elapsed: Duration) -> Option<Duration> {
    let (tx, rx) = flume::bounded(1);
    w.service().send(Command::ArtworkTime(elapsed, tx));
    rx.recv_timeout(WAIT).unwrap()
}

#[test]
fn download_cache_backdrop_and_stale_account_completions() {
    let f = Fixture::new(Library::default());
    let mut w = f.start();
    synced(&w);
    w.service()
        .send(Command::Artwork("cover".into(), Some(Priority::Visible)));
    let Event::Artwork(_, path, thumbnails) = event(&w, |e| matches!(e, Event::Artwork(..))) else {
        unreachable!()
    };
    assert!(path.is_file() && thumbnails);
    assert!(path.starts_with(f.paths.cache.join(f.account.namespace())));
    let bytes = std::fs::read(&path).unwrap();
    w.service()
        .send(Command::Artwork("cover".into(), Some(Priority::Visible)));
    let Event::Artwork(_, cached, _) = event(&w, |e| matches!(e, Event::Artwork(..))) else {
        unreachable!()
    };
    assert_eq!(cached, path);
    assert_eq!(std::fs::read(cached).unwrap(), bytes);
    assert_eq!(
        f.requests()
            .iter()
            .filter(|r| **r == artwork_request())
            .count(),
        1
    );
    w.service().send(Command::Backdrop("cover".into()));
    let Event::Backdrop(_, backdrop) =
        event(&w, |e| matches!(e, Event::Backdrop(id, _) if id == "cover"))
    else {
        unreachable!()
    };
    assert!(sonora::backdrop::backdrop_path(&path).exists());
    w.service().send(Command::Backdrop("cover".into()));
    event(&w, |e| matches!(e, Event::Backdrop(id, _) if id == "cover"));
    // Results from a previous account must not publish, even for the same cover ID.
    w.service().send(Command::ArtworkFinished(
        "another-account".into(),
        "stale".into(),
        Ok(path),
    ));
    w.service().send(Command::BackdropFinished(
        "another-account".into(),
        "stale".into(),
        Ok((*backdrop).clone()),
    ));
    let epoch = w.service().query(Default::default());
    let deadline = Instant::now() + WAIT;
    loop {
        let e = w.events().recv_deadline(deadline).unwrap();
        assert!(!matches!(&e, Event::Artwork(id, ..) | Event::Backdrop(id, _) if id == "stale"));
        if matches!(e, Event::View(request, _) if request == epoch) {
            break;
        }
    }
    w.shutdown(WAIT).unwrap();
    f.finish();
}

#[test]
fn failed_artwork_backs_off_exhausts_budget_and_refresh_retries() {
    let f = Fixture::new(Library {
        fail_artwork: true,
        ..Default::default()
    });
    let mut w = f.start();
    synced(&w);
    w.service()
        .send(Command::Artwork("cover".into(), Some(Priority::Visible)));
    for attempt in 1..=2 {
        let deadline = Instant::now() + WAIT;
        let retry = loop {
            if let Some(retry) = clock(&w, Duration::ZERO) {
                break retry;
            }
            assert!(
                Instant::now() < deadline,
                "artwork completion must schedule backoff"
            );
        };
        assert!(retry > Duration::from_secs((1 << attempt) - 1));
        for _ in 0..10 {
            w.service()
                .send(Command::Artwork("cover".into(), Some(Priority::Visible)));
        }
        clock(&w, Duration::ZERO);
        assert_eq!(
            f.requests()
                .iter()
                .filter(|r| **r == artwork_request())
                .count(),
            attempt
        );
        clock(&w, Duration::from_secs(1 << attempt));
    }
    event(
        &w,
        |e| matches!(e, Event::ArtworkFailed(id) if id == "cover"),
    );
    clock(&w, Duration::from_secs(600));
    assert_eq!(
        f.requests()
            .iter()
            .filter(|r| **r == artwork_request())
            .count(),
        3
    );
    f.edit_library(|l| l.fail_artwork = false);
    w.service().send(Command::Refresh);
    let mut refreshed = false;
    let mut downloaded = false;
    let deadline = Instant::now() + WAIT;
    while !refreshed || !downloaded {
        match w.events().recv_deadline(deadline).unwrap() {
            Event::Status(s) if s.starts_with("Up to date") => refreshed = true,
            Event::Artwork(id, ..) if id == "cover" => downloaded = true,
            Event::Error(_) => panic!("unexpected artwork refresh error"),
            _ => {}
        }
    }
    assert_eq!(
        f.requests()
            .iter()
            .filter(|r| **r == artwork_request())
            .count(),
        4
    );
    w.shutdown(WAIT).unwrap();
    f.finish();
}
