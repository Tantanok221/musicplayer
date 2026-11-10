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
        assert_eq!(query["inc"], "media");
        assert_eq!(group, "rg-album");
        return Ok(Json(json!({"releases": [
            {"media": [{"track-count": 11}]},
            {"media": [{"track-count": 11}]},
            {"media": [{"track-count": 11}, {"track-count": 9}]},
            {"media": []}
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

async fn listens(Path(artist): Path<String>) -> Result<Json<Value>, StatusCode> {
    if artist != "artist" {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(json!([
        {"release_group_mbid": "rg-ep", "total_listen_count": 42},
        {"release_group_mbid": "elsewhere", "total_listen_count": 7}
    ])))
}

async fn client() -> MusicBrainz {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/ws/2/release", get(releases))
                .route("/ws/2/release-group", get(groups))
                .route("/ws/2/artist", get(artists))
                .route(
                    "/1/popularity/top-release-groups-for-artist/{artist}",
                    get(listens),
                ),
        )
        .await
        .unwrap();
    });
    MusicBrainz::with_endpoints(
        &format!("http://{address}/ws/2/"),
        &format!("http://{address}/1/"),
        Duration::from_millis(1),
    )
    .unwrap()
}

#[tokio::test]
async fn discography_pages_releases_into_groups_with_editions_and_listens() {
    let client = client().await;
    let found = client.discography("artist").await.unwrap();
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
    assert_eq!(found.groups[2].listens, 42);
    assert_eq!(found.groups[2].primary, "EP");
}

#[tokio::test]
async fn prolific_artists_fall_back_to_the_default_overview() {
    let found = client().await.discography("prolific").await.unwrap();
    assert_eq!(found.groups.len(), 1);
    assert_eq!(found.groups[0].id, "rg-big");
    assert!(found.groups[0].official && found.groups[0].releases.is_empty());
}

#[tokio::test]
async fn rate_limit_is_retried_once_and_errors_hide_the_address() {
    let client = client().await;
    assert!(client.discography("busy").await.unwrap().groups.is_empty());
    let error = format!("{:#}", client.discography("unknown").await.unwrap_err());
    assert_eq!(error, "MusicBrainz returned HTTP 404");
    let offline = MusicBrainz::with_endpoints(
        "http://127.0.0.1:1/ws/2/",
        "http://127.0.0.1:1/1/",
        Duration::ZERO,
    )
    .unwrap();
    let error = format!("{:#}", offline.discography("artist").await.unwrap_err());
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
