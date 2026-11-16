use chrono::{DateTime, NaiveDate, Utc};
use sonora::{
    discover::{
        Candidate, Dismissal, Hidden, Pick, SHELF_LIMIT, Song, Visit, rediscover, todays_order,
    },
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
    let loved = (0..loved_songs)
        .map(|ix| Song {
            id: format!("{}-loved-{ix}", album.id),
            title: format!("Loved {ix}"),
            plays: 0,
        })
        .collect();
    Candidate {
        album: Arc::new(album),
        loved,
        top_song: None,
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
fn each_pick_reads_like_a_memory() {
    let mut candidates = filler(9);
    let mut rotation = candidate(album("rotation", 44, Some("2026-03-10T00:00:00Z")), 0);
    rotation.top_song = Some(Song {
        id: "low-tide".into(),
        title: "Low Tide".into(),
        plays: 19,
    });
    candidates.push(rotation);
    let mut saved = album("saved", 0, None);
    saved.starred = Some("2023-06-01T00:00:00Z".into());
    saved.created = Some("2023-06-01T00:00:00Z".into());
    candidates.push(candidate(saved, 0));
    candidates.push(candidate(album("loved", 1, Some(OLD)), 3));
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    let find = |id: &str| {
        shelves
            .old_rotation
            .ranked
            .iter()
            .chain(&shelves.forgotten_favorites.ranked)
            .find(|pick| pick.album.id == id)
            .unwrap()
            .clone()
    };
    let rotation = find("rotation");
    assert_eq!(
        rotation.memory(),
        "You played this 44 times. The last time was in March 2026."
    );
    assert_eq!(
        rotation.detail(),
        "The song you came back to most was \u{201c}Low Tide\u{201d}, 19 times."
    );
    let saved = find("saved");
    assert_eq!(
        saved.memory(),
        "You saved this as a favorite but never pressed play."
    );
    assert_eq!(saved.detail(), "In your library since June 2023.");
    let loved = find("loved");
    assert_eq!(
        loved.memory(),
        "You loved 3 songs on this album, then let it rest."
    );
    assert_eq!(
        loved.detail(),
        "Songs you loved: Loved 0 · Loved 1 · Loved 2"
    );
    assert_eq!(
        loved
            .loved
            .iter()
            .map(|song| song.id.as_str())
            .collect::<Vec<_>>(),
        ["loved-loved-0", "loved-loved-1", "loved-loved-2"]
    );
}

#[test]
fn todays_spotlight_draws_from_both_shelves_and_holds_for_the_day() {
    let mut candidates = filler(9);
    candidates.push(candidate(album("rotation", 44, Some(OLD)), 0));
    for ix in 0..6 {
        candidates.push(candidate(album(&format!("loved-{ix}"), 1, Some(OLD)), 2));
    }
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    let mut all = ids(&shelves.spotlight);
    all.sort();
    let mut expected = ids(&shelves.old_rotation.ranked);
    expected.extend(ids(&shelves.forgotten_favorites.ranked));
    expected.sort();
    assert_eq!(all, expected);
    let again = rediscover(&candidates, &Hidden::default(), now());
    assert_eq!(ids(&again.spotlight), ids(&shelves.spotlight));
}

#[test]
fn a_visit_cycles_the_spotlight_and_remembers_expanded_shelves() {
    let mut candidates = filler(9);
    candidates.push(candidate(album("one", 44, Some(OLD)), 0));
    candidates.push(candidate(album("two", 1, Some(OLD)), 3));
    let shelves = rediscover(&candidates, &Hidden::default(), now());
    let mut visit = Visit::default();
    assert!(visit.spotlight(&[]).is_none());
    let first = visit
        .spotlight(&shelves.spotlight)
        .unwrap()
        .album
        .id
        .clone();
    visit.another();
    let second = visit
        .spotlight(&shelves.spotlight)
        .unwrap()
        .album
        .id
        .clone();
    assert_ne!(first, second);
    visit.another();
    assert_eq!(visit.spotlight(&shelves.spotlight).unwrap().album.id, first);
    let fewer = &shelves.spotlight[..1];
    assert!(
        visit.spotlight(fewer).is_some(),
        "a shorter spotlight still has a pick"
    );

    assert!(!visit.expanded("loved"));
    visit.toggle("loved");
    assert!(visit.expanded("loved") && !visit.expanded("older"));
    visit.toggle("loved");
    assert!(!visit.expanded("loved"));
}
