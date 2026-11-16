use chrono::{DateTime, Utc};
use sonora::{
    discover::{Candidate, SHELF_LIMIT, rediscover},
    model::Album,
};
use std::sync::Arc;

const NOW: &str = "2026-11-16T12:00:00Z";
const RECENT: &str = "2026-10-01T00:00:00Z";
const OLD: &str = "2026-03-01T00:00:00Z";

fn now() -> DateTime<Utc> {
    NOW.parse().unwrap()
}

fn album(id: &str, plays: u64, played: Option<&str>) -> Album {
    Album {
        id: id.into(),
        name: id.into(),
        play_count: Some(plays),
        played: played.map(Into::into),
        ..Default::default()
    }
}

fn candidate(album: Album, loved_songs: usize) -> Candidate {
    Candidate {
        album: Arc::new(album),
        loved_songs,
    }
}

fn ids(albums: &[Arc<Album>]) -> Vec<&str> {
    albums.iter().map(|album| album.id.as_str()).collect()
}

fn filler(count: usize) -> Vec<Candidate> {
    (0..count)
        .map(|ix| candidate(album(&format!("filler-{ix}"), 1, Some(OLD)), 0))
        .collect()
}

#[test]
fn old_rotation_keeps_the_most_played_tenth_not_played_for_six_months() {
    let mut candidates = filler(26);
    candidates.push(candidate(album("heavy-old", 50, Some(OLD)), 0));
    candidates.push(candidate(album("heavier-old", 80, Some(OLD)), 0));
    candidates.push(candidate(album("heavy-recent", 90, Some(RECENT)), 0));
    candidates.push(candidate(album("light-old", 2, Some(OLD)), 0));
    let shelves = rediscover(&candidates, now());
    assert_eq!(ids(&shelves.old_rotation), ["heavier-old", "heavy-old"]);
}

#[test]
fn six_month_cutoff_is_inclusive_of_older_plays_only() {
    let mut candidates = filler(8);
    candidates.push(candidate(
        album("just-inside", 40, Some("2026-05-20T00:00:00Z")),
        0,
    ));
    candidates.push(candidate(
        album("just-outside", 40, Some("2026-05-10T00:00:00Z")),
        0,
    ));
    let shelves = rediscover(&candidates, now());
    assert_eq!(ids(&shelves.old_rotation), ["just-outside"]);
}

#[test]
fn forgotten_favorites_rank_album_favorites_then_loved_songs_then_oldest_play() {
    let mut starred = album("starred", 0, None);
    starred.starred = Some("2025-01-01T00:00:00Z".into());
    let mut rated = album("rated", 1, Some("2026-01-01T00:00:00Z"));
    rated.user_rating = Some(4);
    let mut rated_low = album("rated-low", 1, Some(OLD));
    rated_low.user_rating = Some(3);
    let mut recent_favorite = album("recent-favorite", 1, Some(RECENT));
    recent_favorite.starred = Some("2025-01-01T00:00:00Z".into());
    let candidates = vec![
        candidate(album("one-loved-song", 1, Some(OLD)), 1),
        candidate(album("three-loved", 1, Some(OLD)), 3),
        candidate(album("two-loved-older", 1, Some("2025-09-01T00:00:00Z")), 2),
        candidate(album("two-loved", 1, Some(OLD)), 2),
        candidate(rated_low, 0),
        candidate(recent_favorite, 5),
        candidate(rated, 0),
        candidate(starred, 0),
    ];
    let shelves = rediscover(&candidates, now());
    assert_eq!(
        ids(&shelves.forgotten_favorites),
        [
            "starred",
            "rated",
            "three-loved",
            "two-loved-older",
            "two-loved"
        ]
    );
}

#[test]
fn an_album_on_both_shelves_is_only_old_rotation() {
    let mut candidates = filler(9);
    let mut both = album("both", 60, Some(OLD));
    both.starred = Some("2025-01-01T00:00:00Z".into());
    candidates.push(candidate(both, 4));
    let shelves = rediscover(&candidates, now());
    assert_eq!(ids(&shelves.old_rotation), ["both"]);
    assert!(shelves.forgotten_favorites.is_empty());
}

#[test]
fn shelves_stop_at_the_limit_and_an_empty_library_has_none() {
    let candidates = (0..SHELF_LIMIT + 5)
        .map(|ix| candidate(album(&format!("loved-{ix:02}"), 0, None), 2))
        .collect::<Vec<_>>();
    let shelves = rediscover(&candidates, now());
    assert_eq!(shelves.forgotten_favorites.len(), SHELF_LIMIT);
    let empty = rediscover(&[], now());
    assert!(empty.old_rotation.is_empty() && empty.forgotten_favorites.is_empty());
}
