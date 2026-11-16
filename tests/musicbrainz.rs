use axum::{
    Json, Router,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use serde_json::{Value, json};
use sonora::{discography::fold, musicbrainz::MusicBrainz};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

static BUSY: AtomicUsize = AtomicUsize::new(0);

fn release(id: &str, title: &str, status: &str, group: Value) -> Value {
    json!({"id": id, "title": title, "status": status, "release-group": group})
}
fn rg(id: &str, title: &str, primary: &str, secondary: &[&str]) -> Value {
    json!({"id": id, "title": title, "primary-type": primary, "secondary-types": secondary,
           "first-release-date": "2011-05-10"})
}

async fn releases(
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    assert!(
        headers["user-agent"]
            .to_str()
            .unwrap()
            .starts_with("Sonora/")
    );
    assert_eq!(query["fmt"], "json");
    if let Some(group) = query.get("release-group") {
        assert_eq!(group, "rg-album");
        if query["inc"] == "media url-rels" {
            return Ok(Json(json!({"releases": [
                {"id": "r-first", "date": "2011-05-10", "media": [{"track-count": 11}]},
                {"id": "r-digital", "date": "2015-01-01", "media": [{"track-count": 11}], "relations": [
                    {"url": {"resource": "https://open.spotify.com/album/digital"}},
                    {"url": {"resource": "https://tidal.com/album/1"}},
                    {"url": {"resource": "https://www.amazon.com/dp/1"}}
                ]}
            ]})));
        }
        assert_eq!(query["inc"], "media");
        return Ok(Json(json!({"releases": [
            {"id": "r-late", "date": "2012-01-01", "media": [{"track-count": 11}]},
            {"id": "r-first", "date": "2011-05-10", "media": [{"track-count": 11}]},
            {"id": "r-undated", "media": [{"track-count": 11}]},
            {"id": "r-deluxe", "date": "2010-01-01", "media": [{"track-count": 11}, {"track-count": 9}]},
            {"id": "r-empty", "date": "2009-01-01", "media": []}
        ]})));
    }
    match query["artist"].as_str() {
        "busy" => {
            // The first request is turned away, as MusicBrainz does when rate limited.
            if BUSY.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            Ok(Json(json!({"release-count": 0, "releases": []})))
        }
        "prolific" => Ok(Json(json!({"release-count": 5000, "releases": [
            release("r", "Ignored", "Official", rg("rg-ignored", "Ignored", "Album", &[]))
        ]}))),
        "artist" => {
            assert_eq!(query["inc"], "release-groups");
            assert_eq!(query["limit"], "100");
            let album = rg("rg-album", "残響リファレンス", "Album", &[]);
            let page = match query["offset"].as_str() {
                "0" => vec![
                    release("r1", "残響リファレンス", "Official", album.clone()),
                    release("r2", "Zankyō Reference", "Pseudo-Release", album),
                ],
                "2" => vec![
                    release(
                        "r3",
                        "Basement Tape",
                        "Bootleg",
                        rg("rg-bootleg", "Basement Tape", "Album", &["Live"]),
                    ),
                    release(
                        "r4",
                        "Skyfall",
                        "Official",
                        rg("rg-ep", "Skyfall", "EP", &[]),
                    ),
                ],
                _ => vec![],
            };
            Ok(Json(json!({"release-count": 4, "releases": page})))
        }
        _ => Err(StatusCode::NOT_FOUND),
    }
}

async fn release_lookup(
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    // The earliest dated edition with the usual track count is the one shown.
    assert_eq!(id, "r-first");
    assert_eq!(
        query["inc"],
        "recordings artist-credits labels url-rels release-group-level-rels genres"
    );
    let credit = |name: &str| json!([{"name": name, "joinphrase": ""}]);
    Ok(Json(json!({
        "id": "r-first",
        "date": "2011-05-10",
        "country": "JP",
        "artist-credit": credit("Artist"),
        "label-info": [{"catalog-number": "CAT-1", "label": {"name": "Label"}}],
        "genres": [{"name": "rock", "count": 1}, {"name": "j-pop", "count": 5}],
        "relations": [
            {"type": "free streaming", "url": {"resource": "https://open.spotify.com/album/x"}},
            {"type": "discogs", "url": {"resource": "https://www.discogs.com/release/1"}}
        ],
        "release-group": {"relations": [
            {"type": "streaming", "url": {"resource": "https://music.apple.com/album/1"}},
            {"type": "free streaming", "url": {"resource": "https://open.spotify.com/album/x"}}
        ]},
        "media": [
            {"position": 1, "tracks": [
                {"number": "1", "title": "Opening", "length": 61_400, "artist-credit": credit("Artist")},
                {"number": "2", "title": "Duet", "length": null,
                 "recording": {"length": 180_000},
                 "artist-credit": [{"name": "Artist", "joinphrase": " feat. "}, {"name": "Guest", "joinphrase": ""}]}
            ]},
            {"position": 2, "tracks": [
                {"number": "", "title": "Hidden"}
            ]}
        ]
    })))
}

async fn groups(Query(query): Query<HashMap<String, String>>) -> Json<Value> {
    Json(match query["artist"].as_str() {
        "prolific" => {
            assert_eq!(query["release-group-status"], "website-default");
            json!({"release-group-count": 1, "release-groups": [rg("rg-big", "Big Album", "Album", &[])]})
        }
        "namesake" => json!({"release-groups": [rg("x", "Unrelated", "Album", &[])]}),
        "right" => json!({"release-groups": [rg("y", "Senpai Club", "Album", &[])]}),
        "twin-a" | "twin-b" => json!({"release-groups": [rg("z", "Shared Title", "Album", &[])]}),
        _ => json!({"release-groups": []}),
    })
}

async fn artists(Query(query): Query<HashMap<String, String>>) -> Json<Value> {
    Json(if query["query"].contains("C.H.S") {
        json!({"artists": [
            {"id": "namesake", "name": "CHS"},
            {"id": "right", "name": "C.H.S"},
            {"id": "other", "name": "Someone Else"}
        ]})
    } else if query["query"].contains("Twins") {
        json!({"artists": [{"id": "twin-a", "name": "Twins"},
                           {"id": "twin-b", "name": "Other", "aliases": [{"name": "TWINS"}]}]})
    } else {
        json!({"artists": []})
    })
}

async fn client() -> MusicBrainz {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/ws/2/release", get(releases))
                .route("/ws/2/release/{id}", get(release_lookup))
                .route("/ws/2/release-group", get(groups))
                .route("/ws/2/artist", get(artists)),
        )
        .await
        .unwrap();
    });
    MusicBrainz::with_endpoints(&format!("http://{address}/ws/2/"), Duration::from_millis(1))
        .unwrap()
}

#[tokio::test]
async fn discography_pages_releases_into_groups_with_editions() {
    let client = client().await;
    let found = client.discography("artist", false).await.unwrap();
    assert_eq!(found.artist, "artist");
    assert!(found.fetched_ms > 0);
    let ids = found
        .groups
        .iter()
        .map(|group| group.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["rg-album", "rg-bootleg", "rg-ep"]);
    let album = &found.groups[0];
    assert!(album.official);
    assert_eq!(album.releases, ["r1", "r2"]);
    assert_eq!(album.titles, ["Zankyō Reference"]);
    assert_eq!(album.date, "2011-05-10");
    assert!(!found.groups[1].official);
    assert_eq!(found.groups[1].secondary, ["Live"]);
    assert_eq!(found.groups[2].primary, "EP");
}

#[tokio::test]
async fn prolific_artists_fall_back_to_the_default_overview() {
    let found = client().await.discography("prolific", false).await.unwrap();
    assert_eq!(found.groups.len(), 1);
    assert_eq!(found.groups[0].id, "rg-big");
    assert!(found.groups[0].official && found.groups[0].releases.is_empty());
    assert!(found.overview);
}

#[tokio::test]
async fn rate_limit_is_retried_once_and_errors_hide_the_address() {
    let client = client().await;
    assert!(
        client
            .discography("busy", false)
            .await
            .unwrap()
            .groups
            .is_empty()
    );
    let error = format!(
        "{:#}",
        client.discography("unknown", false).await.unwrap_err()
    );
    assert_eq!(error, "MusicBrainz returned HTTP 404");
    let offline = MusicBrainz::with_endpoints("http://127.0.0.1:1/ws/2/", Duration::ZERO).unwrap();
    let error = format!(
        "{:#}",
        offline.discography("artist", false).await.unwrap_err()
    );
    assert!(error.starts_with("Cannot reach MusicBrainz"));
    assert!(!error.contains("127.0.0.1"), "{error}");
}

#[tokio::test]
async fn namesakes_resolve_only_through_a_shared_release() {
    let client = client().await;
    let library = [fold("Senpai Club")].into();
    assert_eq!(
        client
            .resolve_artist("C.H.S", &library)
            .await
            .unwrap()
            .as_deref(),
        Some("right")
    );
    // Nothing in common with any candidate, an unknown name, and an empty library.
    let unrelated = [fold("Nothing Alike")].into();
    assert_eq!(
        client.resolve_artist("C.H.S", &unrelated).await.unwrap(),
        None
    );
    assert_eq!(
        client.resolve_artist("Nobody", &library).await.unwrap(),
        None
    );
    assert_eq!(
        client
            .resolve_artist("C.H.S", &Default::default())
            .await
            .unwrap(),
        None
    );
    // Two candidates that fit equally well are not guessed between.
    let shared = [fold("Shared Title")].into();
    assert_eq!(client.resolve_artist("Twins", &shared).await.unwrap(), None);
}

#[tokio::test]
async fn track_count_is_the_most_common_official_edition() {
    assert_eq!(
        client().await.track_count("rg-album").await.unwrap(),
        Some(11)
    );
}

#[tokio::test]
async fn release_details_read_the_usual_edition() {
    let details = client().await.release_details("rg-album").await.unwrap();
    assert_eq!(details.release, "r-first");
    assert_eq!(details.country, "JP");
    assert_eq!(
        (details.label.as_str(), details.catalog_number.as_str()),
        ("Label", "CAT-1")
    );
    assert_eq!(details.genres, ["j-pop", "rock"]);
    let tracks: Vec<_> = details
        .tracks
        .iter()
        .map(|track| {
            (
                track.disc,
                track.number.as_str(),
                track.title.as_str(),
                track.seconds,
                track.artist.as_str(),
            )
        })
        .collect();
    assert_eq!(
        tracks,
        [
            (1, "1", "Opening", Some(61), ""),
            (1, "2", "Duet", Some(180), "Artist feat. Guest"),
            (2, "1", "Hidden", None, ""),
        ]
    );
    let links: Vec<_> = details
        .links
        .iter()
        .map(|link| (link.service.as_str(), link.url.as_str()))
        .collect();
    assert_eq!(
        links,
        [
            ("Spotify", "https://open.spotify.com/album/x"),
            ("Apple Music", "https://music.apple.com/album/1"),
            ("Spotify", "https://open.spotify.com/album/digital"),
            ("Tidal", "https://tidal.com/album/1"),
        ]
    );
    assert!(details.fetched_ms > 0);
}

#[tokio::test]
async fn requests_are_spaced_from_their_start_not_their_end() {
    let started = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let log = started.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/ws/2/release",
                get(move |Query(query): Query<HashMap<String, String>>| {
                    let log = log.clone();
                    async move {
                        log.lock().unwrap().push(tokio::time::Instant::now());
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        let offset = &query["offset"];
                        Json(json!({"release-count": 3, "releases": [release(
                            &format!("r{offset}"),
                            "Title",
                            "Official",
                            rg(&format!("rg{offset}"), "Title", "Album", &[]),
                        )]}))
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let client = MusicBrainz::with_endpoints(
        &format!("http://{address}/ws/2/"),
        Duration::from_millis(500),
    )
    .unwrap();
    assert_eq!(
        client
            .discography("artist", false)
            .await
            .unwrap()
            .groups
            .len(),
        3
    );
    let started = started.lock().unwrap();
    assert_eq!(started.len(), 3);
    for pair in started.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= Duration::from_millis(490) && gap < Duration::from_millis(750),
            "requests started {gap:?} apart"
        );
    }
}
