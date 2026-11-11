use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde_json::{Value, json};
use sonora::slskd::{
    Completeness, Progress, RemoteFile, SearchResponse, Slskd, SlskdAuth, SlskdKey, Waiting,
    candidates, progress, waiting,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const KEY: &str = "0123456789abcdef-secret";

fn file(path: &str, size: u64) -> RemoteFile {
    RemoteFile {
        filename: path.into(),
        size,
        length: Some(200),
        ..Default::default()
    }
}
fn flac(path: &str, depth: u32, rate: u32) -> RemoteFile {
    RemoteFile {
        bit_depth: Some(depth),
        sample_rate: Some(rate),
        ..file(path, 30_000_000)
    }
}
fn mp3(path: &str, rate: u32) -> RemoteFile {
    RemoteFile {
        bit_rate: Some(rate),
        ..file(path, 8_000_000)
    }
}
fn response(username: &str, free: bool, files: Vec<RemoteFile>) -> SearchResponse {
    SearchResponse {
        username: username.into(),
        has_free_upload_slot: free,
        queue_length: if free { 0 } else { 12 },
        upload_speed: 1_000_000,
        files,
        locked_file_count: 0,
    }
}

#[test]
fn folders_are_grouped_described_and_ranked() {
    let responses = [
        response(
            "partial",
            true,
            vec![flac(r"@@a\Music\Artist\Album\01.flac", 16, 44100)],
        ),
        response(
            "lossy",
            true,
            vec![
                mp3(r"@@b\Artist - Album\01 one.mp3", 320),
                mp3(r"@@b\Artist - Album\02 two.mp3", 320),
                file(r"@@b\Artist - Album\cover.jpg", 100_000),
            ],
        ),
        response(
            "discs",
            false,
            vec![
                flac(r"@@c\Album [24-96]\CD1\01.flac", 24, 96000),
                flac(r"@@c\Album [24-96]\CD 2\01.flac", 24, 96000),
                flac(r"@@c\Other Album\01.flac", 16, 44100),
                flac(r"@@c\Other Album\02.flac", 24, 44100),
                mp3(r"@@c\Other Album\03.mp3", 192),
            ],
        ),
        response("artwork only", true, vec![file(r"@@d\Album\scan.png", 5)]),
    ];
    let found = candidates(&responses, Some(2));
    let summary = found
        .iter()
        .map(|c| {
            (
                c.username.as_str(),
                c.folder(),
                c.tracks,
                c.format.as_str(),
                c.completeness,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        summary,
        [
            (
                "discs",
                "Album [24-96]",
                2,
                "FLAC 24/96",
                Completeness::Exact
            ),
            ("lossy", "Artist - Album", 2, "MP3 320", Completeness::Exact),
            (
                "discs",
                "Other Album",
                3,
                "Mixed (FLAC, MP3)",
                Completeness::Extra
            ),
            ("partial", "Album", 1, "FLAC 16/44.1", Completeness::Partial),
        ]
    );
    // Artwork travels with the folder but is not a track.
    assert_eq!(found[1].files.len(), 3);
    assert_eq!(found[1].size, 16_100_000);
    assert_eq!(found[1].seconds, 400);
    assert_eq!(found[0].directory, r"@@c\Album [24-96]");
    assert!(!found[0].free_slot && found[0].queue == 12);
    // Without a known track count nothing is called complete or partial.
    assert!(
        candidates(&responses, None)
            .iter()
            .all(|c| c.completeness == Completeness::Unknown)
    );
    // Lossless files whose depth is not reported, or differs, get the bare format.
    let vague = [response(
        "vague",
        true,
        vec![
            file(r"x\A\1.flac", 1),
            flac(r"x\A\2.flac", 16, 44100),
            file(r"x\B\noext", 1),
        ],
    )];
    assert_eq!(candidates(&vague, None)[0].format, "FLAC");
    assert_eq!(candidates(&vague, None).len(), 1);
}

#[test]
fn progress_counts_each_file_once_by_its_latest_state() {
    let files = ["a", "b", "c", "d"].map(String::from);
    let downloads = json!({"directories": [{"files": [
        {"filename": "a", "state": "Completed, Succeeded"},
        {"filename": "b", "state": "Completed, Rejected"},
        {"filename": "b", "state": "Queued, Remotely", "placeInQueue": 7},
        {"filename": "c", "state": "InProgress"},
        {"filename": "unrelated", "state": "Completed, Errored"}
    ]}]});
    let now = progress(&downloads, &files);
    assert_eq!(
        now,
        Progress {
            total: 4,
            done: 1,
            queued: 1,
            active: 1,
            place: Some(7),
            ..Default::default()
        }
    );
    assert!(!now.finished());
    let failed = progress(
        &json!({"directories": [{"files": [
            {"filename": "a", "state": "Completed, Succeeded"},
            {"filename": "b", "state": "Completed, TimedOut"},
            {"filename": "c", "state": "Completed, Succeeded"},
            {"filename": "d", "state": "Completed, Succeeded"}
        ]}]}),
        &files,
    );
    assert!(failed.finished() && !failed.succeeded());
    assert_eq!(failed.failure, "timedout");
    assert!(!progress(&Value::Null, &[]).finished());
    // slskd had already cleared the folder's finished files when the last one arrived.
    let cleared = progress(
        &json!({"directories": [{"files": [{"filename": "d", "state": "Completed, Succeeded"}]}]}),
        &files,
    );
    assert!(cleared.succeeded());
    // While a file is still moving, cleared files are not assumed to have arrived.
    let moving = progress(
        &json!({"directories": [{"files": [{"filename": "d", "state": "InProgress"}]}]}),
        &files,
    );
    assert_eq!((moving.done, moving.finished()), (0, false));
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);
impl Log {
    fn push(&self, entry: impl Into<String>) {
        self.0.lock().unwrap().push(entry.into());
    }
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}
fn authorized(headers: &HeaderMap) -> Result<(), StatusCode> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    // The first session token has "expired"; only the second is honoured.
    (header("x-api-key") == Some(KEY) || header("authorization") == Some("Bearer token-2"))
        .then_some(())
        .ok_or(StatusCode::UNAUTHORIZED)
}

async fn serve(batches: bool) -> (String, Log) {
    let log = Log::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut router = Router::new()
        .route(
            "/slskd/api/v0/application",
            get(|headers: HeaderMap| async move {
                authorized(&headers)?;
                Ok::<_, StatusCode>(Json(json!({
                    "version": {"current": "0.26.2"},
                    "server": {"state": "Connected, LoggedIn"}
                })))
            }),
        )
        .route(
            "/slskd/api/v0/session",
            post(|State(log): State<Log>, Json(body): Json<Value>| async move {
                if body["username"] != "admin" || body["password"] != "hunter2 hunter2" {
                    return Err(StatusCode::UNAUTHORIZED);
                }
                log.push("sign in");
                let issued = log.0.lock().unwrap().iter().filter(|e| *e == "sign in").count();
                Ok(Json(json!({"token": format!("token-{issued}"), "tokenType": "Bearer"})))
            }),
        )
        .route(
            "/slskd/api/v0/searches",
            post(|State(log): State<Log>, Json(body): Json<Value>| async move {
                assert_eq!(body["id"].as_str().unwrap().len(), 36);
                log.push(format!("search {}", body["searchText"].as_str().unwrap()));
                StatusCode::CREATED
            }),
        )
        .route(
            "/slskd/api/v0/searches/{id}",
            get(|State(log): State<Log>| async move {
                let polls = log.0.lock().unwrap().iter().filter(|e| *e == "poll").count();
                log.push("poll");
                Json(json!({"isComplete": polls >= 1, "responseCount": polls + 1}))
            })
            .delete(|State(log): State<Log>| async move {
                log.push("delete");
                StatusCode::NO_CONTENT
            }),
        )
        .route(
            "/slskd/api/v0/searches/{id}/responses",
            get(|| async {
                Json(json!([{
                    "username": "peer", "hasFreeUploadSlot": true, "queueLength": 0,
                    "uploadSpeed": 5000, "fileCount": 1, "lockedFileCount": 2,
                    "files": [{"filename": "@@x\\Album\\01.flac", "size": 10, "extension": "",
                               "bitDepth": 16, "sampleRate": 44100, "length": 61, "isLocked": false}],
                    "lockedFiles": []
                }]))
            }),
        )
        .route(
            "/slskd/api/v0/users/{user}/directory",
            post(|Path(user): Path<String>, Json(body): Json<Value>| async move {
                assert_eq!(user, "peer / odd");
                assert_eq!(body["directory"], "@@x\\Album");
                Json(json!([
                    {"name": "@@x\\Album", "files": [{"filename": "01.flac", "size": 10},
                                                     {"filename": "cover.jpg", "size": 3}]},
                    {"name": "@@x\\Album\\CD2", "files": [{"filename": "01.flac", "size": 11}]}
                ]))
            }),
        )
        .route(
            "/slskd/api/v0/transfers/downloads/{user}",
            post(
                |State(log): State<Log>, Path(user): Path<String>, Json(body): Json<Value>| async move {
                    if user == "offline" {
                        return StatusCode::NOT_FOUND;
                    }
                    // Before batches existed, that path was read as a peer named "batches".
                    let Some(files) = body.as_array() else {
                        return StatusCode::BAD_REQUEST;
                    };
                    log.push(format!("legacy {user} {}", files.len()));
                    StatusCode::CREATED
                },
            )
            .get(|Path(user): Path<String>| async move {
                Json(json!({"username": user, "directories": []}))
            }),
        );
    if batches {
        router = router.route(
            "/slskd/api/v0/transfers/downloads/batches",
            post(
                |State(log): State<Log>, Json(body): Json<Value>| async move {
                    let files = body["files"].as_array().unwrap();
                    assert!(files[0]["filename"].is_string() && files[0]["size"].is_u64());
                    log.push(format!(
                        "batch {} {}",
                        body["username"].as_str().unwrap(),
                        files.len()
                    ));
                    if body["username"] == "refuses" {
                        return (
                            StatusCode::OK,
                            Json(json!({"failures": [{"filename": "a", "message": "no"}]})),
                        );
                    }
                    (StatusCode::CREATED, Json(json!({"failures": []})))
                },
            ),
        );
    }
    let state = log.clone();
    tokio::spawn(async move {
        axum::serve(listener, router.with_state(state))
            .await
            .unwrap();
    });
    (format!("http://{address}/slskd"), log)
}
fn client(url: &str) -> Slskd {
    Slskd::with_gap(url, SlskdKey::new(KEY).unwrap(), Duration::from_millis(1)).unwrap()
}

#[tokio::test]
async fn search_polls_to_completion_then_cleans_up() {
    let (url, log) = serve(true).await;
    let client = client(&url);
    assert_eq!(client.test().await.unwrap(), "Connected to slskd 0.26.2");
    let mut counts = Vec::new();
    let responses = client
        .search("  Artist   Album ", |count| counts.push(count))
        .await
        .unwrap();
    assert_eq!(counts, [1, 2]);
    assert_eq!(
        log.take(),
        ["search Artist Album", "poll", "poll", "delete"]
    );
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].locked_file_count, 2);
    let found = candidates(&responses, Some(1));
    assert_eq!(found[0].format, "FLAC 16/44.1");
    assert_eq!(found[0].locked, 2);
    assert_eq!(found[0].seconds, 61);
    assert!(client.search("   ", |_| {}).await.is_err());
}

#[tokio::test]
async fn folder_listing_gets_full_paths_and_downloads_are_queued() {
    let (url, log) = serve(true).await;
    let client = client(&url);
    let files = client.directory("peer / odd", r"@@x\Album").await.unwrap();
    let names = files
        .iter()
        .map(|f| f.filename.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            r"@@x\Album\01.flac",
            r"@@x\Album\cover.jpg",
            r"@@x\Album\CD2\01.flac"
        ]
    );
    client.enqueue("peer", &files).await.unwrap();
    assert_eq!(log.take(), ["batch peer 3"]);
    let refused = client.enqueue("refuses", &files).await.unwrap_err();
    assert_eq!(
        refused.to_string(),
        "The peer refused every file in this folder"
    );
    assert!(client.enqueue("peer", &[]).await.is_err());
    assert_eq!(
        client.downloads("peer").await.unwrap()["directories"],
        json!([])
    );
}

#[tokio::test]
async fn older_slskd_takes_the_plain_file_list() {
    let (url, log) = serve(false).await;
    let client = client(&url);
    let files = [file(r"@@x\Album\01.flac", 10)];
    client.enqueue("peer", &files).await.unwrap();
    assert_eq!(log.take(), ["legacy peer 1"]);
    let offline = client.enqueue("offline", &files).await.unwrap_err();
    assert_eq!(
        offline.to_string(),
        "That peer is offline. Pick another folder."
    );
}

#[tokio::test]
async fn errors_never_reveal_the_key_or_address() {
    let (url, _) = serve(true).await;
    let wrong = Slskd::new(&url, SlskdKey::new("wrong-key-0123456789").unwrap()).unwrap();
    let rejected = format!("{:#}", wrong.test().await.unwrap_err());
    assert_eq!(rejected, "slskd rejected the API key");
    let offline = client("http://127.0.0.1:1");
    let unreachable = format!("{:#}", offline.test().await.unwrap_err());
    assert!(unreachable.starts_with("Cannot reach slskd"));
    assert!(
        !unreachable.contains("127.0.0.1") && !unreachable.contains(KEY),
        "{unreachable}"
    );
    assert!(SlskdKey::new("short").is_err());
    assert!(Slskd::new("slskd.local:5030", SlskdKey::new(KEY).unwrap()).is_err());
    assert!(Slskd::new("http://user:pass@host/", SlskdKey::new(KEY).unwrap()).is_err());
}

#[tokio::test]
async fn web_login_is_exchanged_for_a_token_and_renewed_when_it_lapses() {
    let (url, log) = serve(true).await;
    let login = |password: &str| {
        Slskd::new(
            &url,
            SlskdAuth::from_fields("", "admin", password)
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    };
    let client = login("hunter2 hunter2");
    assert_eq!(client.test().await.unwrap(), "Connected to slskd 0.26.2");
    // The first token was turned away, so the client signed in once more and then kept it.
    assert_eq!(log.take(), ["sign in", "sign in"]);
    client.test().await.unwrap();
    assert!(log.take().is_empty());
    let wrong = format!("{:#}", login("wrong").test().await.unwrap_err());
    assert_eq!(wrong, "slskd rejected the username or password");
    assert!(!wrong.contains("wrong ") && !wrong.contains("admin"));
}

#[tokio::test]
async fn no_login_is_sent_when_none_is_saved() {
    let (url, _) = serve(true).await;
    let anonymous = Slskd::new(&url, SlskdAuth::None).unwrap();
    // The search route in this mock checks nothing, like an slskd with authentication off.
    assert_eq!(
        anonymous
            .search("Artist Album", |_| {})
            .await
            .unwrap()
            .len(),
        1
    );
    // One that does check says what to enter instead of blaming a key that was never given.
    let error = anonymous.test().await.unwrap_err().to_string();
    assert_eq!(
        error,
        "slskd asks for a login. Enter its API key, or its web username and password."
    );
}

#[test]
fn only_files_the_peer_lists_without_a_place_count_as_lost() {
    let files = ["a", "b", "c", "d"].map(String::from);
    let downloads = json!({"directories": [{"files": [
        {"id": "1", "filename": "a", "size": 5, "state": "Queued, Remotely"},
        {"id": "2", "filename": "b", "size": 6, "state": "Queued, Remotely", "placeInQueue": 3},
        {"id": "3", "filename": "c", "size": 7, "state": "Completed, Succeeded"},
        {"id": "4", "filename": "d", "size": 8, "state": "Queued, Remotely"},
        {"id": "5", "filename": "d", "size": 8, "state": "InProgress"},
        {"id": "6", "filename": "other", "size": 9, "state": "Queued, Remotely"}
    ]}]});
    assert_eq!(
        waiting(&downloads, &files),
        [Waiting {
            id: "1".into(),
            filename: "a".into(),
            size: 5
        }]
    );
}

#[tokio::test]
async fn lost_requests_are_cancelled_and_asked_for_again() {
    let log = Log::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new()
        .route(
            "/api/v0/users/{user}/info",
            get(|Path(user): Path<String>| async move {
                Json(json!({"hasFreeUploadSlot": user == "idle", "queueLength": 0}))
            }),
        )
        .route(
            "/api/v0/transfers/downloads/{user}/{id}",
            axum::routing::delete(
                |State(log): State<Log>,
                 Path((user, id)): Path<(String, String)>,
                 axum::extract::RawQuery(query): axum::extract::RawQuery| async move {
                    log.push(format!("cancel {user} {id} {}", query.unwrap_or_default()));
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .route(
            "/api/v0/transfers/downloads/batches",
            post(
                |State(log): State<Log>, Json(body): Json<Value>| async move {
                    let file = &body["files"][0];
                    log.push(format!("enqueue {} {}", file["filename"], file["size"]));
                    StatusCode::CREATED
                },
            ),
        )
        .with_state(log.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = client(&format!("http://{address}"));
    assert!(client.peer_idle("idle").await.unwrap());
    assert!(!client.peer_idle("busy").await.unwrap());
    let lost = [Waiting {
        id: "t1".into(),
        filename: r"@@x\Album\09.flac".into(),
        size: 42,
    }];
    client.requeue("idle", &lost).await.unwrap();
    assert_eq!(
        log.take(),
        [
            "cancel idle t1 remove=true",
            r#"enqueue "@@x\\Album\\09.flac" 42"#
        ]
    );
}
