use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde_json::{Value, json};
use sonora::{catalog::Catalog, model::*, platform::Account, server::Server};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Fixture {
    seen: Mutex<Vec<(String, HashMap<String, String>)>>,
}
async fn endpoint(
    State(state): State<Arc<Fixture>>,
    Path(method): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    state
        .seen
        .lock()
        .unwrap()
        .push((method.clone(), params.clone()));
    let payload = match method.as_str() {
        "getArtist.view" if params["id"] == "combined" => {
            json!({"artist":{"id":"combined","name":"Main & Other","coverArt":"wrong"}})
        }
        "getArtist.view" => json!({"artist":{"id":"main","name":"Main","coverArt":"artist-cover"}}),
        "search3.view" => {
            json!({"searchResult3":{"artist":[{"id":"other","name":"Mainly"},{"id":"main","name":"Main","coverArt":"artist-cover"}]}})
        }
        "getArtistInfo2.view" => {
            json!({"artistInfo2":{"biography":"<p>Main &amp; friends.</p><script>bad</script>","lastFmUrl":"https://www.last.fm/music/Main", "largeImageUrl":"https://images.example.com/portrait.jpg", "similarArtist":[{"id":"other","name":"Other"},{"id":"-1","name":"New"}]}})
        }
        "getTopSongs.view" => {
            json!({"topSongs":{"song":[{"id":"owned","title":"Owned","artist":"Main"},{"id":"missing","title":"Missing","artist":"Main"}]}})
        }
        "scrobble.view" => json!({}),
        "getSong.view" => json!({"song":{"id":"owned","playCount":8}}),
        _ => json!({}),
    };
    let mut envelope = json!({"status":"ok","version":"1.16.1"});
    envelope
        .as_object_mut()
        .unwrap()
        .extend(payload.as_object().unwrap().clone());
    Json(json!({"subsonic-response":envelope}))
}
#[tokio::test]
async fn artist_metadata_uses_verified_ids_owned_rankings_and_only_the_connected_server() {
    let state = Arc::new(Fixture::default());
    let app = Router::new()
        .route("/rest/{method}", get(endpoint))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut server = Server::new(
        Account {
            url: format!("http://{address}"),
            username: "fixture-user".into(),
        },
        "fixture-password".into(),
    )
    .unwrap();
    server.extensions.insert("topSongsByArtistId".into());
    let mut profile = server
        .artist_profile("Main", "combined", true)
        .await
        .unwrap();
    assert_eq!(profile.id, "main");
    assert_eq!(profile.biography, "Main & friends.");
    assert_eq!(
        profile.portrait,
        "remote:https://images.example.com/portrait.jpg"
    );
    assert!(profile.complete);
    let mut db = Catalog::memory().unwrap();
    db.put_tracks(
        &[
            Track {
                id: "owned".into(),
                artist: "Main".into(),
                album_artist: "Main".into(),
                album_id: "album".into(),
                play_count: Some(3),
                ..Default::default()
            },
            Track {
                id: "other-song".into(),
                artist: "Other".into(),
                album_artist: "Other".into(),
                album_id: "other".into(),
                ..Default::default()
            },
        ],
        1,
    )
    .unwrap();
    db.hydrate_artist_profile(&mut profile).unwrap();
    assert_eq!(profile.popular.len(), 1);
    assert_eq!(profile.popular[0].id, "owned");
    assert_eq!(profile.popular[0].play_count, Some(3));
    assert!(profile.similar[0].in_library);
    assert!(!profile.similar[1].in_library);
    assert!(profile.similar[1].id.is_empty());
    server
        .scrobble("owned", false, 1700000000000)
        .await
        .unwrap();
    server.scrobble("owned", true, 1700000000000).await.unwrap();
    assert_eq!(server.song("owned").await.unwrap().play_count, Some(8));
    let seen = state.seen.lock().unwrap();
    let info = seen
        .iter()
        .find(|(method, _)| method == "getArtistInfo2.view")
        .unwrap();
    assert_eq!(info.1["id"], "main");
    assert_eq!(info.1["includeNotPresent"], "true");
    let top = seen
        .iter()
        .find(|(method, _)| method == "getTopSongs.view")
        .unwrap();
    assert_eq!(top.1["id"], "main");
    let reports = seen
        .iter()
        .filter(|(method, _)| method == "scrobble.view")
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].1["submission"], "false");
    assert_eq!(reports[1].1["submission"], "true");
    assert_eq!(reports[1].1["time"], "1700000000000");
    task.abort();
}

#[tokio::test]
async fn an_unconfirmed_submission_is_not_retried_by_the_http_client() {
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = attempts.clone();
    let app = Router::new().route(
        "/rest/scrobble.view",
        get(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let server = Server::new(
        Account {
            url: format!("http://{address}"),
            username: "fixture-user".into(),
        },
        "fixture-password".into(),
    )
    .unwrap();
    assert!(server.scrobble("track", true, 1700000000000).await.is_err());
    assert_eq!(attempts.load(std::sync::atomic::Ordering::Relaxed), 1);
    task.abort();
}
