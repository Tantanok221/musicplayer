use sonora::{artists::*, catalog::Catalog, listening::ListenReport, model::*};

#[test]
fn biographies_links_and_cache_age_are_safe_and_predictable() {
    assert_eq!(
        plain_biography(
            "<p>A &amp; B</p><script>alert('x')</script><style>bad</style><p>Japanese &#x97F3;&#x697D;.</p>"
        ),
        "A & B Japanese 音楽."
    );
    assert_eq!(
        plain_biography("<a href='javascript:bad'>Artist</a>"),
        "Artist"
    );
    assert!(profile_url("javascript:alert(1)").is_none());
    assert!(profile_url("https://example.com/?token=fixture").is_none());
    assert!(lastfm_url("A+B / 音楽").contains("A%2BB%20%2F%20"));
    let now = 1700000000000;
    let profile = ArtistProfile {
        fetched_ms: now,
        complete: true,
        ..Default::default()
    };
    assert!(profile.fresh(true, now + 1000));
    assert!(!profile.fresh(true, now + 86_400_000));
    let missing = ArtistProfile {
        fetched_ms: now,
        ..Default::default()
    };
    assert!(missing.fresh(true, now + 1000));
    assert!(!missing.fresh(true, now + 3_600_000));
    let portrait_only = ArtistProfile {
        id: "artist".into(),
        ..missing
    };
    assert!(portrait_only.fresh(false, now + 1000));
    assert!(!portrait_only.fresh(true, now + 1000));
}

#[test]
fn durable_submission_ledger_never_resends_attempted_or_uncertain_plays() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("library.sqlite");
    let report = ListenReport {
        id: u64::MAX,
        account: "account".into(),
        track_id: "song".into(),
        started_ms: 1700000000000,
        submission: true,
    };
    {
        let db = Catalog::open(&path).unwrap();
        db.queue_scrobble(&report).unwrap();
        db.queue_scrobble(&report).unwrap();
        assert_eq!(db.pending_scrobbles("account").unwrap().len(), 1);
        assert!(db.attempt_scrobble(report.id).unwrap());
        assert!(!db.attempt_scrobble(report.id).unwrap());
    }
    let db = Catalog::open(&path).unwrap();
    db.queue_scrobble(&report).unwrap();
    assert!(db.pending_scrobbles("account").unwrap().is_empty());
    db.finish_scrobble(report.id, false).unwrap();
    assert!(!db.attempt_scrobble(report.id).unwrap());
    assert!(db.pending_scrobbles("account").unwrap().is_empty());
    let other = Catalog::memory().unwrap();
    assert!(other.pending_scrobbles("other-account").unwrap().is_empty());
    let next = ListenReport { id: 2, ..report };
    db.queue_scrobble(&next).unwrap();
    assert!(db.attempt_scrobble(2).unwrap());
    db.finish_scrobble(2, true).unwrap();
    db.queue_scrobble(&next).unwrap();
    assert!(db.pending_scrobbles("account").unwrap().is_empty());
}

#[test]
fn artist_rankings_include_guests_preserve_cache_and_use_fresh_server_counts() {
    let mut db = Catalog::memory().unwrap();
    let tracks = [
        ("one", "debut", "Main", "Main", 2),
        ("two", "debut", "Main", "Main", 8),
        ("guest", "other", "Other & Main", "Other", 12),
        ("none", "single", "Main", "Main", 0),
        ("wrong", "wrong", "Mainly", "Mainly", 99),
    ]
    .into_iter()
    .map(|(id, album, artist, album_artist, plays)| Track {
        id: id.into(),
        title: id.into(),
        album_id: album.into(),
        album: album.into(),
        artist: artist.into(),
        album_artist: album_artist.into(),
        genre: "Rock".into(),
        play_count: Some(plays),
        ..Default::default()
    })
    .collect::<Vec<_>>();
    db.put_tracks(&tracks, 7).unwrap();
    let page = db.artist_page("Main", "").unwrap();
    assert_eq!(
        page.most_played
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["guest", "two", "one"]
    );
    assert_eq!(page.albums.len(), 2);
    assert_eq!(page.genres, ["Rock"]);
    let profile = ArtistProfile {
        name: "Main".into(),
        biography: "Biography".into(),
        fetched_ms: 1700000000000,
        complete: true,
        ..Default::default()
    };
    let key = cache_key("Main", "main");
    db.save_artist_profile(&key, &profile).unwrap();
    assert!(
        db.artist_profile(&cache_key("Other", "other"))
            .unwrap()
            .is_none()
    );
    db.invalidate_artist_profiles().unwrap();
    let cached = db.artist_profile(&key).unwrap().unwrap();
    assert_eq!(cached.biography, "Biography");
    assert!(!cached.fresh(true, 1700000000000));
    let fresh = Track {
        id: "one".into(),
        play_count: Some(30),
        ..Default::default()
    };
    db.refresh_play_count(&fresh).unwrap();
    assert_eq!(db.artist_page("Main", "").unwrap().most_played[0].id, "one");
    assert_eq!(db.by_ids(&["one".into()]).unwrap()[0].album, "debut");
    db.finish_sync(7).unwrap();
    assert_eq!(
        db.count().unwrap(),
        tracks.len(),
        "Refreshing a count must preserve the sync generation"
    );
}

#[test]
fn album_popularity_keeps_artist_rank_and_limit() {
    let track = |id: &str, album: &str| {
        std::sync::Arc::new(Track {
            id: id.into(),
            album_id: album.into(),
            ..Default::default()
        })
    };
    let now = 1700000000000;
    let top = TopSongs::from_tracks(
        &[
            track("b2", "b"),
            track("a3", "a"),
            track("a1", "a"),
            track("a2", "a"),
            track("a4", "a"),
        ],
        now,
    );
    assert_eq!(top.for_album("a", 3), ["a3", "a1", "a2"]);
    assert_eq!(top.for_album("b", 3), ["b2"]);
    assert!(top.for_album("c", 3).is_empty());
    assert!(top.fresh(now + 1000));
    assert!(!top.fresh(now + 86_400_000));
    assert!(!TopSongs::default().fresh(now));
}
