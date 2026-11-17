use chrono::{DateTime, NaiveDate, Utc};
use sonora::{
    discover::{Dismissal, Hidden, Pick, SHELF_LIMIT, Visit, rediscover, todays_order},
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

fn candidate(album: Album, _: usize) -> Arc<Album> {
    Arc::new(album)
}

fn ids(picks: &[Pick]) -> Vec<&str> {
    picks.iter().map(|pick| pick.album.id.as_str()).collect()
}

fn filler(count: usize) -> Vec<Arc<Album>> {
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
fn old_rotation_stops_at_the_limit_and_an_empty_library_has_none() {
    let candidates = (0..SHELF_LIMIT + 5)
        .map(|ix| candidate(album(&format!("heavy-{ix:02}"), 50, Some(OLD)), 0))
        .collect::<Vec<_>>();
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    assert_eq!(shelves.old_rotation.ranked.len(), SHELF_LIMIT);
    assert_eq!(shelves.old_rotation.today.len(), SHELF_LIMIT);
    let empty = rediscover(&[], &Hidden::default(), now());
    assert!(empty.old_rotation.ranked.is_empty() && empty.lane.is_empty());
}

#[test]
fn each_pick_explains_why_it_is_on_the_shelf() {
    let mut candidates = filler(20);
    candidates.push(candidate(
        album("rotation", 44, Some("2026-03-10T00:00:00Z")),
        0,
    ));
    candidates.push(candidate(
        album("almost-a-year", 40, Some("2025-11-20T00:00:00Z")),
        0,
    ));
    candidates.push(candidate(
        album("long-ago", 41, Some("2024-06-01T00:00:00Z")),
        0,
    ));
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    let reason = |id: &str| {
        shelves
            .old_rotation
            .ranked
            .iter()
            .find(|pick| pick.album.id == id)
            .unwrap()
            .reason_text(now())
    };
    assert_eq!(reason("rotation"), "44 plays · last played 8 months ago");
    assert_eq!(
        reason("almost-a-year"),
        "40 plays · last played 11 months ago"
    );
    assert_eq!(reason("long-ago"), "41 plays · last played over a year ago");
}

#[test]
fn todays_order_is_stable_within_a_day_and_changes_the_next() {
    let candidates = (0..20)
        .map(|ix| candidate(album(&format!("heavy-{ix:02}"), 50, Some(OLD)), 0))
        .collect::<Vec<_>>();
    let ranked = rediscover(&candidates, &Hidden::default(), now())
        .old_rotation
        .ranked;
    let monday = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
    let tuesday = monday.succ_opt().unwrap();
    let first = todays_order(&ranked, monday, "old-rotation");
    assert_eq!(
        ids(&first),
        ids(&todays_order(&ranked, monday, "old-rotation"))
    );
    let next = todays_order(&ranked, tuesday, "old-rotation");
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
    let mut candidates = filler(20);
    candidates.push(candidate(album("snoozed", 50, Some(OLD)), 0));
    candidates.push(candidate(album("never", 49, Some(OLD)), 0));
    candidates.push(candidate(album("kept", 48, Some(OLD)), 0));
    let mut hidden = Hidden::default();
    hidden.hide("snoozed", Dismissal::NotNow, now());
    hidden.hide("never", Dismissal::Never, now());
    let shelves = rediscover(&candidates, &hidden, now());
    assert_eq!(ids(&shelves.old_rotation.ranked), ["kept"]);
    let later = now() + chrono::Duration::days(31);
    let shelves = rediscover(&candidates, &hidden, later);
    assert_eq!(ids(&shelves.old_rotation.ranked), ["snoozed", "kept"]);
}

#[test]
fn memory_lane_groups_old_rotation_by_the_season_it_was_last_played() {
    let mut candidates = filler(50);
    for (id, plays, played) in [
        ("spring-big", 50, "2026-04-10T00:00:00Z"),
        ("spring-small", 46, "2026-05-01T00:00:00Z"),
        ("winter-jan", 49, "2026-01-15T00:00:00Z"),
        ("winter-dec", 48, "2025-12-20T00:00:00Z"),
        ("long-ago", 47, "2025-08-01T00:00:00Z"),
    ] {
        candidates.push(candidate(album(id, plays, Some(played)), 0));
    }
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    let lane = shelves
        .lane
        .iter()
        .map(|season| {
            (
                season.key.as_str(),
                season.label.as_str(),
                ids(&season.shelf.ranked),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lane,
        [
            (
                "2026-spring",
                "Spring 2026",
                vec!["spring-big", "spring-small"]
            ),
            (
                "2025-winter",
                "Winter 2025–26",
                vec!["winter-jan", "winter-dec"]
            ),
            ("older", "A year ago and longer", vec!["long-ago"]),
        ]
    );
    let mut today = ids(&shelves.lane[1].shelf.today);
    today.sort();
    assert_eq!(today, ["winter-dec", "winter-jan"]);
}

#[test]
fn a_visit_remembers_expanded_shelves() {
    let mut visit = Visit::default();
    assert!(!visit.expanded("older"));
    visit.toggle("older");
    assert!(visit.expanded("older") && !visit.expanded("2026-spring"));
    visit.toggle("older");
    assert!(!visit.expanded("older"));
}
