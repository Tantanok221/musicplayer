use crate::fixture::{Fixture, Library, Request, WAIT, album, event, song, synced, view};
use sonora::{
    artists::cache_key,
    model::{LibraryQuery, NamedId},
    service::{Command, Event},
};

#[test]
fn artist_browser_includes_guest_credits_on_later_tracks() {
    let guest = NamedId {
        id: "guest".into(),
        name: "Guest Artist".into(),
    };
    let mut second = song("02", "album");
    second.album_artists.push(guest.clone());
    let mut third = song("03", "album");
    third.album_artists.push(guest.clone());
    let f = Fixture::new(Library {
        tracks: vec![song("01", "album"), second, third],
        albums: vec![album("album")],
        ..Default::default()
    });
    let mut w = f.start();
    synced(&w);

    let mut query = LibraryQuery::default();
    query.selection.artists.push(guest.name.clone());
    let request = w.service().query(query);
    let Event::View(_, filtered) = event(&w, |e| matches!(e, Event::View(id, _) if *id == request))
    else {
        unreachable!()
    };
    assert_eq!(filtered.track_count, 2);

    let browser = view(&w).browser;
    let facet = browser
        .artists
        .iter()
        .find(|facet| facet.label == guest.name)
        .expect("the artist browser should list the guest credited on tracks 02 and 03");
    assert_eq!(facet.detail, guest.id);
    assert_eq!(facet.count, 1, "repeated credits count as one album");

    w.shutdown(WAIT).unwrap();
    f.finish();
}

#[test]
fn artist_page_survives_enrichment_failure_and_release_details_cache_and_hide_persist() {
    let f = Fixture::new(Library {
        fail_enrichment: true,
        ..Default::default()
    });
    let mut w = f.start();
    synced(&w);
    w.service().send(Command::Artist(
        17,
        "Fixture Artist".into(),
        "artist".into(),
    ));
    let Event::Artist(request, page) = event(&w, |e| matches!(e, Event::Artist(..))) else {
        unreachable!()
    };
    assert_eq!(request, 17);
    assert_eq!(page.albums.len(), 2);
    assert_eq!(page.tracks.len(), 2);
    let key = cache_key("Fixture Artist", "artist");
    let Event::ArtistProfile(_, profile) = event(&w, |e| matches!(e, Event::ArtistProfile(..)))
    else {
        unreachable!()
    };
    assert!(!profile.complete);
    assert_eq!(view(&w).track_count, 2);
    w.service().send(Command::Discography(
        "Fixture Artist".into(),
        "artist".into(),
    ));
    event(
        &w,
        |e| matches!(e, Event::Discography(_, d) if !d.loading && d.missing.len() == 1),
    );
    w.service()
        .send(Command::ReleaseDetails("missing-release".into()));
    let Event::ReleaseDetails(_, Ok(details)) =
        event(&w, |e| matches!(e, Event::ReleaseDetails(..)))
    else {
        unreachable!()
    };
    assert_eq!(details.tracks[0].title, "Missing song");
    w.service()
        .send(Command::ReleaseDetails("missing-release".into()));
    event(&w, |e| matches!(e, Event::ReleaseDetails(_, Ok(_))));
    assert_eq!(
        f.requests()
            .iter()
            .filter(|r| **r == Request::new("metadata/details", &[]))
            .count(),
        1
    );
    w.service().send(Command::HideRelease(
        "Fixture Artist".into(),
        "artist".into(),
        "missing-release".into(),
    ));
    event(
        &w,
        |e| matches!(e, Event::Discography(_, d) if !d.loading && d.missing.is_empty()),
    );
    assert_eq!(
        f.catalog()
            .state::<Vec<String>>(&format!("discography-hidden:{key}"))
            .unwrap(),
        ["missing-release"]
    );
    // An explicit refresh invalidates the incomplete profile so enrichment can recover.
    f.edit_library(|l| l.fail_enrichment = false);
    w.service().send(Command::Refresh);
    synced(&w);
    w.service().send(Command::ArtistProfile(
        "Fixture Artist".into(),
        "artist".into(),
        true,
    ));
    event(
        &w,
        |e| matches!(e, Event::ArtistProfile(_, p) if p.complete && p.biography == "Fixture biography"),
    );
    w.shutdown(WAIT).unwrap();
    let mut restarted = f.start();
    synced(&restarted);
    restarted.service().send(Command::Discography(
        "Fixture Artist".into(),
        "artist".into(),
    ));
    event(
        &restarted,
        |e| matches!(e, Event::Discography(_, d) if !d.loading && d.missing.is_empty()),
    );
    restarted
        .service()
        .send(Command::ReleaseDetails("missing-release".into()));
    event(
        &restarted,
        |e| matches!(e, Event::ReleaseDetails(_, Ok(d)) if d.tracks[0].title == "Missing song"),
    );
    assert_eq!(
        f.requests()
            .iter()
            .filter(|r| **r == Request::new("metadata/details", &[]))
            .count(),
        1
    );
    restarted.shutdown(WAIT).unwrap();
    f.finish();
}
