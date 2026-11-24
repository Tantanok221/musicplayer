//! Season bounds and refresh rules for kept listening history.
use chrono::{DateTime, Utc};
use sonora::history::{ChartAlbum, History, SeasonChart, seasons_between, stale_seasons};

const NOW: &str = "2026-11-16T12:00:00Z";

fn now() -> DateTime<Utc> {
    NOW.parse().unwrap()
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

fn history(seasons: &[(&str, Vec<ChartAlbum>)]) -> History {
    let mut history = History {
        user: "listener".into(),
        registered: Some(at("2020-07-15T00:00:00Z").timestamp()),
        ..Default::default()
    };
    for (key, albums) in seasons {
        let span = seasons_between(at("2020-01-01T00:00:00Z"), now())
            .into_iter()
            .find(|span| span.key == *key)
            .unwrap();
        history.seasons.insert(
            span.key.clone(),
            SeasonChart {
                from: span.from,
                to: span.to,
                fetched_ms: now().timestamp_millis(),
                albums: albums.clone(),
            },
        );
    }
    history
}

#[test]
fn seasons_run_from_the_first_scrobble_to_now_with_their_bounds() {
    let spans = seasons_between(at("2020-07-15T00:00:00Z"), at("2021-01-10T00:00:00Z"));
    let keys = spans
        .iter()
        .map(|span| (span.key.as_str(), span.label.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            ("2020-summer", "Summer 2020"),
            ("2020-autumn", "Autumn 2020"),
            ("2020-winter", "Winter 2020–21"),
        ]
    );
    assert_eq!(spans[0].from, at("2020-06-01T00:00:00Z").timestamp());
    assert_eq!(spans[0].to, at("2020-09-01T00:00:00Z").timestamp());
    assert_eq!(spans[2].from, at("2020-12-01T00:00:00Z").timestamp());
    assert_eq!(spans[2].to, at("2021-03-01T00:00:00Z").timestamp());
}

#[test]
fn only_missing_or_unfinished_seasons_are_fetched_again() {
    let mut known = history(&[("2025-summer", vec![]), ("2026-autumn", vec![])]);
    let stale = |history: &History, now: DateTime<Utc>| {
        stale_seasons(history, now)
            .into_iter()
            .map(|span| span.key)
            .collect::<Vec<_>>()
    };
    let missing = stale(&known, now());
    assert!(missing.contains(&"2020-summer".to_string()));
    assert!(
        !missing.contains(&"2025-summer".to_string()),
        "a finished season stays"
    );
    assert!(
        !missing.contains(&"2026-autumn".to_string()),
        "just fetched"
    );
    assert_eq!(missing.len(), 24);

    let later = now() + chrono::Duration::hours(13);
    assert!(stale(&known, later).contains(&"2026-autumn".to_string()));
    let summer = known.seasons.get_mut("2025-summer").unwrap();
    summer.fetched_ms = at("2025-08-20T00:00:00Z").timestamp_millis();
    assert!(
        stale(&known, now()).contains(&"2025-summer".to_string()),
        "a season fetched before it ended is fetched again"
    );
    known.registered = None;
    assert!(
        stale(&known, now()).is_empty(),
        "nothing to fetch before the profile is known"
    );
}
