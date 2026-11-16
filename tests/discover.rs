use chrono::{DateTime, NaiveDate, Utc};
use sonora::{
    discover::{Candidate, Dismissal, Hidden, Pick, SHELF_LIMIT, rediscover, todays_order},
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

fn ids(picks: &[Pick]) -> Vec<&str> {
    picks.iter().map(|pick| pick.album.id.as_str()).collect()
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
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    assert_eq!(
        ids(&shelves.old_rotation.ranked),
        ["heavier-old", "heavy-old"]
    );
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
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    assert_eq!(ids(&shelves.old_rotation.ranked), ["just-outside"]);
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
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    assert_eq!(
        ids(&shelves.forgotten_favorites.ranked),
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
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    assert_eq!(ids(&shelves.old_rotation.ranked), ["both"]);
    assert!(shelves.forgotten_favorites.ranked.is_empty());
}

#[test]
fn shelves_stop_at_the_limit_and_an_empty_library_has_none() {
    let candidates = (0..SHELF_LIMIT + 5)
        .map(|ix| candidate(album(&format!("loved-{ix:02}"), 0, None), 2))
        .collect::<Vec<_>>();
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    assert_eq!(shelves.forgotten_favorites.ranked.len(), SHELF_LIMIT);
    assert_eq!(shelves.forgotten_favorites.today.len(), SHELF_LIMIT);
    let empty = rediscover(&[], &Hidden::default(), now());
    assert!(empty.old_rotation.ranked.is_empty() && empty.forgotten_favorites.ranked.is_empty());
}

#[test]
fn each_pick_explains_why_it_is_on_the_shelf() {
    let mut candidates = filler(9);
    candidates.push(candidate(
        album("rotation", 44, Some("2026-03-10T00:00:00Z")),
        0,
    ));
    let mut favorite = album("favorite", 1, Some("2025-10-01T00:00:00Z"));
    favorite.starred = Some("2025-01-01T00:00:00Z".into());
    candidates.push(candidate(favorite, 0));
    let mut rated = album("rated", 0, None);
    rated.user_rating = Some(5);
    candidates.push(candidate(rated, 3));
    candidates.push(candidate(album("loved", 1, Some(OLD)), 2));
    candidates.push(candidate(
        album("almost-a-year", 1, Some("2025-11-20T00:00:00Z")),
        2,
    ));
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    let reason = |picks: &[Pick], id: &str| {
        picks
            .iter()
            .find(|pick| pick.album.id == id)
            .unwrap()
            .reason_text(now())
    };
    assert_eq!(
        reason(&shelves.old_rotation.ranked, "rotation"),
        "44 plays · last played 8 months ago"
    );
    let forgotten = &shelves.forgotten_favorites.ranked;
    assert_eq!(
        reason(forgotten, "favorite"),
        "Favorite album · last played over a year ago"
    );
    assert_eq!(
        reason(forgotten, "rated"),
        "Rated 5 stars · 3 loved songs · never played"
    );
    assert_eq!(
        reason(forgotten, "loved"),
        "2 loved songs · last played 8 months ago"
    );
    assert_eq!(
        reason(forgotten, "almost-a-year"),
        "2 loved songs · last played 11 months ago"
    );
}

#[test]
fn todays_order_is_stable_within_a_day_and_changes_the_next() {
    let candidates = (0..20)
        .map(|ix| candidate(album(&format!("loved-{ix:02}"), 0, None), 2))
        .collect::<Vec<_>>();
    let ranked = rediscover(&candidates, &Hidden::default(), now())
        .forgotten_favorites
        .ranked;
    let monday = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
    let tuesday = monday.succ_opt().unwrap();
    let first = todays_order(&ranked, monday, "forgotten-favorites");
    assert_eq!(
        ids(&first),
        ids(&todays_order(&ranked, monday, "forgotten-favorites"))
    );
    let next = todays_order(&ranked, tuesday, "forgotten-favorites");
    assert_ne!(ids(&first)[..6], ids(&next)[..6]);
    let mut all = ids(&next);
    all.sort();
    assert_eq!(all, ids(&ranked));
    assert_ne!(
        ids(&first),
        ids(&ranked),
        "today's row is not just the ranking"
    );
}

#[test]
fn not_now_hides_for_thirty_days_and_never_hides_for_good() {
    let mut candidates = filler(9);
    candidates.push(candidate(album("snoozed", 50, Some(OLD)), 0));
    candidates.push(candidate(album("never", 1, Some(OLD)), 2));
    candidates.push(candidate(album("kept", 1, Some(OLD)), 2));
    let mut hidden = Hidden::default();
    hidden.hide("snoozed", Dismissal::NotNow, now());
    hidden.hide("never", Dismissal::Never, now());
    let shelves = rediscover(&candidates, &hidden, now());
    assert!(shelves.old_rotation.ranked.is_empty());
    assert_eq!(ids(&shelves.forgotten_favorites.ranked), ["kept"]);
    let later = now() + chrono::Duration::days(31);
    let shelves = rediscover(&candidates, &hidden, later);
    assert_eq!(ids(&shelves.old_rotation.ranked), ["snoozed"]);
    assert_eq!(ids(&shelves.forgotten_favorites.ranked), ["kept"]);
}
