use crate::fixture::{Fixture, Library, Request, WAIT, event, synced};
use serde_json::json;
use sonora::{
    lyrics::{LyricLine, Lyrics},
    model::Track,
    service::{Command, Event, test_support::Worker},
};
use std::sync::Arc;

fn track() -> Arc<Track> {
    Arc::new(crate::fixture::song("song", "album"))
}

fn by_id() -> Request {
    Request::new("getLyricsBySongId.view", &[("id", "song")])
}

fn by_title() -> Request {
    Request::new(
        "getLyrics.view",
        &[("artist", "Fixture Artist"), ("title", "Song song")],
    )
}

fn lyrics(worker: &Worker) -> Result<Arc<Lyrics>, String> {
    worker.service().send(Command::Lyrics(track()));
    let Event::Lyrics(_, result) = event(
        worker,
        |e| matches!(e, Event::Lyrics(id, _) if id == "song"),
    ) else {
        unreachable!()
    };
    result
}

fn count(fixture: &Fixture, request: &Request) -> usize {
    fixture.requests().iter().filter(|r| *r == request).count()
}

fn timed() -> Lyrics {
    Lyrics {
        synced: true,
        offset: -250,
        line: vec![
            LyricLine {
                start: Some(1000),
                value: "First timed line".into(),
            },
            LyricLine {
                start: Some(2500),
                value: "Second timed line".into(),
            },
        ],
    }
}

#[test]
fn timed_lyrics_are_preferred_cached_and_survive_restart() {
    let fixture = Fixture::new(Library {
        structured_lyrics: json!([
            {"synced":false,"line":[{"value":"Plain version"}]},
            {"synced":true,"offset":-250,"line":[
                {"start":1000,"value":"First timed line"},
                {"start":2500,"value":"Second timed line"}
            ]}
        ]),
        ..Default::default()
    });
    let mut worker = fixture.start();
    synced(&worker);
    assert_eq!(*lyrics(&worker).unwrap(), timed());
    assert_eq!(*lyrics(&worker).unwrap(), timed());
    assert_eq!(
        count(&fixture, &by_id()),
        1,
        "the second request must use the cache"
    );
    worker.shutdown(WAIT).unwrap();

    let mut restarted = fixture.start();
    synced(&restarted);
    assert_eq!(*lyrics(&restarted).unwrap(), timed());
    assert_eq!(count(&fixture, &by_id()), 1);
    assert_eq!(count(&fixture, &by_title()), 0);
    restarted.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn missing_structured_lyrics_fall_back_to_artist_and_title() {
    let fixture = Fixture::new(Library {
        structured_lyrics: json!([{"line":[{"value":"  "}]}]),
        plain_lyrics: "Legacy line\nNext line",
        ..Default::default()
    });
    let mut worker = fixture.start();
    synced(&worker);
    assert_eq!(
        *lyrics(&worker).unwrap(),
        Lyrics::plain("Legacy line\nNext line")
    );
    assert_eq!(
        (count(&fixture, &by_id()), count(&fixture, &by_title())),
        (1, 1)
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn failed_lyrics_are_reported_without_credentials_and_retried() {
    let fixture = Fixture::new(Library {
        plain_lyrics: "Recovered line",
        ..Default::default()
    });
    let mut worker = fixture.start();
    synced(&worker);
    fixture.edit_library(|library| library.offline = true);
    let error = lyrics(&worker).unwrap_err();
    assert!(!error.is_empty());
    assert!(!error.contains("synthetic-password") && !error.contains("?u="));
    fixture.edit_library(|library| library.offline = false);
    assert_eq!(*lyrics(&worker).unwrap(), Lyrics::plain("Recovered line"));
    assert_eq!(
        count(&fixture, &by_title()),
        2,
        "a failure must not be cached"
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn lyrics_from_a_previous_account_are_neither_published_nor_cached() {
    let fixture = Fixture::new(Library {
        plain_lyrics: "Current account",
        ..Default::default()
    });
    let mut worker = fixture.start();
    synced(&worker);
    worker.service().send(Command::LyricsLoaded(
        "another-account".into(),
        "song".into(),
        Ok(Lyrics::plain("Previous account")),
    ));
    let epoch = worker.service().query(Default::default());
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let e = worker.events().recv_deadline(deadline).unwrap();
        assert!(
            !matches!(e, Event::Lyrics(..)),
            "stale lyrics must not publish"
        );
        if matches!(e, Event::View(request, _) if request == epoch) {
            break;
        }
    }
    assert_eq!(*lyrics(&worker).unwrap(), Lyrics::plain("Current account"));
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}
