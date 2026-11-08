use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde_json::{Value, json};
use sonora::{
    catalog::Catalog,
    model::*,
    platform::Account,
    server::Server,
    sorting::{Preset, Scope},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

type Requests = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;
async fn endpoint(
    State(requests): State<Requests>,
    Path(endpoint): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(query.get("u").map(String::as_str), Some("fixture"));
    assert!(!query.contains_key("p"));
    let params = query
        .into_iter()
        .filter(|(key, _)| matches!(key.as_str(), "id" | "albumId" | "rating"))
        .collect::<HashMap<_, _>>();
    let rejected = params.values().any(|v| v == "rejected");
    requests.lock().unwrap().push((endpoint, params));
    Json(
        json!({"subsonic-response":if rejected { json!({"status":"failed","error":{"code":70,"message":"Album feedback rejected"}}) } else { json!({"status":"ok"}) }}),
    )
}

#[tokio::test]
async fn album_favorites_use_album_ids_and_ratings_support_set_and_clear() {
    let requests = Requests::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/rest/{endpoint}", get(endpoint))
        .with_state(requests.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let server = Server::new(
        Account {
            url: format!("http://{address}"),
            username: "fixture".into(),
        },
        "fixture-password".into(),
    )
    .unwrap();
    server.album_star("album", true).await.unwrap();
    server.album_star("album", false).await.unwrap();
    server.rating("album", 4).await.unwrap();
    server.rating("album", 0).await.unwrap();
    let error = server
        .album_star("rejected", true)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Album feedback rejected"));
    assert!(!error.contains("fixture-password") && !error.contains("?u="));
    let recorded = requests.lock().unwrap();
    assert_eq!(
        recorded[0],
        (
            "star.view".into(),
            HashMap::from([("albumId".into(), "album".into())])
        )
    );
    assert_eq!(
        recorded[1],
        (
            "unstar.view".into(),
            HashMap::from([("albumId".into(), "album".into())])
        )
    );
    assert_eq!(
        recorded[2],
        (
            "setRating.view".into(),
            HashMap::from([("id".into(), "album".into()), ("rating".into(), "4".into())])
        )
    );
    assert_eq!(recorded[3].1.get("rating").map(String::as_str), Some("0"));
    assert!(!recorded[3].1.contains_key("albumId"));
    task.abort();
}

fn seed(db: &mut Catalog) {
    db.put_tracks(
        &[
            Track {
                id: "one".into(),
                title: "Song one".into(),
                album_id: "album".into(),
                album: "Release".into(),
                track: Some(1),
                user_rating: Some(1),
                ..Default::default()
            },
            Track {
                id: "two".into(),
                title: "Song two".into(),
                album_id: "album".into(),
                album: "Release".into(),
                track: Some(2),
                ..Default::default()
            },
            Track {
                id: "other".into(),
                title: "Other".into(),
                album_id: "other-album".into(),
                album: "Other".into(),
                track: Some(1),
                ..Default::default()
            },
        ],
        17,
    )
    .unwrap();
    db.put_albums(
        &[
            Album {
                id: "album".into(),
                name: "Release".into(),
                artist: "Artist".into(),
                year: Some(2024),
                user_rating: Some(2),
                record_labels: vec![NamedId {
                    name: "Label".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            Album {
                id: "other-album".into(),
                name: "Other".into(),
                artist: "Artist".into(),
                user_rating: Some(3),
                ..Default::default()
            },
        ],
        17,
    )
    .unwrap();
}

#[test]
fn album_feedback_is_persistent_without_rewriting_song_feedback_or_sync_generation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite");
    {
        let mut db = Catalog::open(&path).unwrap();
        seed(&mut db);
        let revision = db.revision();
        let a = db
            .update_album_feedback("album", Some(true), Some(5))
            .unwrap()
            .unwrap();
        assert!(a.starred.is_some());
        assert_eq!(a.user_rating, Some(5));
        assert!(db.revision() > revision);
        assert_eq!(a.name, "Release");
        assert_eq!(a.year, Some(2024));
        assert_eq!(a.record_labels[0].name, "Label");
        let song = db.by_ids(&["one".into()]).unwrap().pop().unwrap();
        assert_eq!(song.user_rating, Some(1));
        assert!(song.starred.is_none());
        db.finish_sync(17).unwrap();
        assert_eq!(db.count().unwrap(), 3);
        assert!(db.album("album").unwrap().is_some());
    }
    let mut db = Catalog::open(&path).unwrap();
    assert_eq!(db.album("album").unwrap().unwrap().user_rating, Some(5));
    let cleared = db
        .update_album_feedback("album", Some(false), Some(0))
        .unwrap()
        .unwrap();
    assert!(cleared.starred.is_none());
    assert_eq!(cleared.user_rating, Some(0));
    assert_eq!(db.by_ids(&["one".into()]).unwrap()[0].user_rating, Some(1));
    let revision = db.revision();
    assert!(
        db.update_album_feedback("missing", Some(true), None)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        db.revision(),
        revision,
        "missing albums must not create phantom cache entries"
    );
}

#[test]
fn album_favorites_include_the_whole_album_but_song_favorites_stay_independent() {
    let mut db = Catalog::memory().unwrap();
    seed(&mut db);
    db.update_album_feedback("album", Some(true), None).unwrap();
    let mut q = LibraryQuery {
        favorites: true,
        ordering: Some(Preset::Album.spec(Scope::Albums)),
        ..Default::default()
    };
    assert_eq!(db.matching_ids(&q).unwrap(), ["one", "two"]);
    assert_eq!(db.view(&q).unwrap().albums.len(), 1);
    assert_eq!(
        db.album_tracks_matching(&q, "album").unwrap().len(),
        2,
        "a favorite album keeps its full track list"
    );
    q.ordering = Some(Preset::Title.spec(Scope::Tracks));
    assert!(db.matching_ids(&q).unwrap().is_empty());
    db.update_album_feedback("album", Some(false), None)
        .unwrap();
    q.ordering = Some(Preset::Album.spec(Scope::Albums));
    assert!(db.matching_ids(&q).unwrap().is_empty());
}

#[test]
fn album_ratings_update_sort_keys_and_clear_without_losing_other_metadata() {
    let mut db = Catalog::memory().unwrap();
    seed(&mut db);
    let q = LibraryQuery {
        ordering: Some(Preset::Rating.spec(Scope::Albums)),
        ..Default::default()
    };
    assert_eq!(db.view(&q).unwrap().albums[0].id, "other-album");
    db.update_album_feedback("album", None, Some(5)).unwrap();
    assert_eq!(db.view(&q).unwrap().albums[0].id, "album");
    db.update_album_feedback("album", None, Some(0)).unwrap();
    assert_eq!(db.view(&q).unwrap().albums[0].id, "other-album");
    assert_eq!(
        db.album("album").unwrap().unwrap().record_labels[0].name,
        "Label"
    );
}
