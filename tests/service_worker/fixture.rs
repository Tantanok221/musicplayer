use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};
use sonora::{
    catalog::{Catalog, LibraryView},
    model::{Album, LibraryQuery, Playlist, Track},
    musicbrainz::MusicBrainz,
    platform::{Account, Paths, ServerCredentials},
    service::{Command, Event, Mutation, test_support::Worker},
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub endpoint: String,
    pub params: BTreeMap<String, String>,
}

impl Request {
    pub fn new(endpoint: &str, params: &[(&str, &str)]) -> Self {
        Self {
            endpoint: endpoint.into(),
            params: params
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }
}

pub struct Library {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub playlists: Vec<Playlist>,
    pub fail_song_offset: Option<usize>,
    pub scanning: bool,
    pub hold_metadata: bool,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            tracks: vec![song("song", "album"), song("other-song", "other-album")],
            albums: vec![album("album"), album("other-album")],
            playlists: vec![],
            fail_song_offset: None,
            scanning: false,
            hold_metadata: false,
        }
    }
}

pub fn song(id: &str, album_id: &str) -> Track {
    Track {
        id: id.into(),
        title: format!("Song {id}"),
        album_id: album_id.into(),
        album: format!("Release {album_id}"),
        artist: "Fixture Artist".into(),
        artist_id: "artist".into(),
        album_artist: "Fixture Artist".into(),
        user_rating: Some(1),
        ..Default::default()
    }
}

pub fn album(id: &str) -> Album {
    Album {
        id: id.into(),
        name: format!("Release {id}"),
        artist: "Fixture Artist".into(),
        artist_id: "artist".into(),
        user_rating: Some(2),
        ..Default::default()
    }
}

struct StateData {
    library: Library,
    requests: Vec<Request>,
    expected_mutations: VecDeque<(Request, bool)>,
    unexpected: Vec<&'static str>,
}

#[derive(Clone)]
struct ServerState {
    data: Arc<Mutex<StateData>>,
    received: flume::Sender<Request>,
    metadata_gate: flume::Receiver<()>,
}

impl ServerState {
    fn record(&self, request: Request) {
        self.data.lock().unwrap().requests.push(request.clone());
        let _ = self.received.send(request);
    }

    fn unexpected(&self, reason: &'static str) -> Response {
        self.data.lock().unwrap().unexpected.push(reason);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Unexpected fixture request",
        )
            .into_response()
    }
}

fn envelope(body: Value) -> Response {
    Json(json!({"subsonic-response":body})).into_response()
}

async fn navidrome(
    State(state): State<ServerState>,
    Path(endpoint): Path<String>,
    method: Method,
    Query(mut query): Query<BTreeMap<String, String>>,
) -> Response {
    let authenticated = query.remove("u").as_deref() == Some("fixture")
        && query.remove("t").is_some_and(|token| token.len() == 32)
        && query.remove("s").is_some_and(|salt| !salt.is_empty())
        && query.remove("v").as_deref() == Some("1.16.1")
        && query.remove("c").as_deref() == Some("Sonora")
        && query.remove("f").as_deref() == Some("json");
    if !authenticated || method != Method::GET || query.contains_key("p") {
        return state.unexpected("invalid method or authentication");
    }
    if !matches!(
        endpoint.as_str(),
        "ping.view"
            | "getOpenSubsonicExtensions.view"
            | "getScanStatus.view"
            | "getPlaylists.view"
            | "search3.view"
            | "star.view"
            | "unstar.view"
            | "setRating.view"
            | "getArtist.view"
            | "getTopSongs.view"
    ) || query.keys().any(|key| {
        !matches!(
            key.as_str(),
            "query"
                | "artistCount"
                | "albumCount"
                | "songCount"
                | "songOffset"
                | "albumOffset"
                | "id"
                | "albumId"
                | "rating"
                | "artist"
                | "count"
        )
    }) {
        return state.unexpected("unknown endpoint or parameter");
    }
    let request = Request {
        endpoint: endpoint.clone(),
        params: query.clone(),
    };
    state.record(request.clone());
    let mut data = state.data.lock().unwrap();
    let body = match endpoint.as_str() {
        "ping.view" if query.is_empty() => {
            json!({"status":"ok","type":"Navidrome","serverVersion":"fixture"})
        }
        "getOpenSubsonicExtensions.view" if query.is_empty() => {
            json!({"status":"ok","openSubsonicExtensions":[]})
        }
        "getScanStatus.view" if query.is_empty() => {
            json!({"status":"ok","scanStatus":{"scanning":data.library.scanning}})
        }
        "getPlaylists.view" if query.is_empty() => {
            json!({"status":"ok","playlists":{"playlist":data.library.playlists}})
        }
        "getArtist.view" if query == BTreeMap::from([("id".into(), "artist".into())]) => {
            json!({"status":"ok","artist":{"id":"artist","name":"Fixture Artist","musicBrainzId":"mb-artist"}})
        }
        "getTopSongs.view"
            if query
                == Request::new("", &[("artist", "Fixture Artist"), ("count", "50")]).params =>
        {
            json!({"status":"ok","topSongs":{"song":[]}})
        }
        "search3.view" => {
            let (kind, offset_key) = if query.get("songCount").map(String::as_str) == Some("500") {
                ("song", "songOffset")
            } else {
                ("album", "albumOffset")
            };
            let offset = query
                .get(offset_key)
                .and_then(|value| value.parse::<usize>().ok());
            let expected = BTreeMap::from([
                ("query".into(), "".into()),
                ("artistCount".into(), "0".into()),
                (
                    "albumCount".into(),
                    if kind == "album" { "500" } else { "0" }.into(),
                ),
                (
                    "songCount".into(),
                    if kind == "song" { "500" } else { "0" }.into(),
                ),
                (offset_key.into(), offset.unwrap_or(0).to_string()),
            ]);
            if offset.is_none() || query != expected {
                data.unexpected.push("invalid sync page parameters");
                return envelope(
                    json!({"status":"failed","error":{"code":70,"message":"Unexpected page"}}),
                );
            }
            let offset = offset.unwrap();
            if kind == "song" && data.library.fail_song_offset == Some(offset) {
                json!({"status":"failed","error":{"code":70,"message":"Fixture later page failed"}})
            } else {
                let items = if kind == "song" {
                    serde_json::to_value(
                        data.library
                            .tracks
                            .iter()
                            .skip(offset)
                            .take(500)
                            .collect::<Vec<_>>(),
                    )
                    .unwrap()
                } else {
                    serde_json::to_value(
                        data.library
                            .albums
                            .iter()
                            .skip(offset)
                            .take(500)
                            .collect::<Vec<_>>(),
                    )
                    .unwrap()
                };
                json!({"status":"ok","searchResult3":{kind:items}})
            }
        }
        "star.view" | "unstar.view" | "setRating.view" => {
            match data.expected_mutations.pop_front() {
                Some((expected, succeeds)) if request == expected => {
                    if succeeds {
                        json!({"status":"ok"})
                    } else {
                        json!({"status":"failed","error":{"code":70,"message":"Fixture feedback rejected"}})
                    }
                }
                _ => {
                    data.unexpected.push("unexpected mutation or parameters");
                    json!({"status":"failed","error":{"code":70,"message":"Unexpected mutation"}})
                }
            }
        }
        _ => {
            data.unexpected.push("unexpected parameters");
            json!({"status":"failed","error":{"code":70,"message":"Unexpected parameters"}})
        }
    };
    envelope(body)
}

async fn metadata(
    State(state): State<ServerState>,
    Path(endpoint): Path<String>,
    method: Method,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    if method != Method::GET
        || endpoint != "release"
        || query
            != Request::new(
                "",
                &[
                    ("artist", "mb-artist"),
                    ("inc", "release-groups"),
                    ("limit", "100"),
                    ("offset", "0"),
                    ("fmt", "json"),
                ],
            )
            .params
    {
        return state.unexpected("unexpected metadata request");
    }
    state.record(Request::new("metadata/release", &[]));
    let hold = state.data.lock().unwrap().library.hold_metadata;
    if hold {
        let _ = state.metadata_gate.recv_async().await;
    }
    Json(json!({"release-count":1,"releases":[{
        "id":"edition","status":"Official",
        "release-group":{"id":"missing-release","title":"Missing Release",
            "primary-type":"Album","first-release-date":"2020-01-01"}
    }]}))
    .into_response()
}

async fn listens(
    State(state): State<ServerState>,
    method: Method,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    if method != Method::GET || !query.is_empty() {
        return state.unexpected("unexpected ListenBrainz request");
    }
    state.record(Request::new("listens/mb-artist", &[]));
    Json(json!([{"release_group_mbid":"missing-release","total_listen_count":42}])).into_response()
}

async fn unexpected(State(state): State<ServerState>) -> Response {
    state.unexpected("unregistered route")
}

pub struct Fixture {
    _dir: tempfile::TempDir,
    pub paths: Paths,
    pub account: Account,
    state: ServerState,
    requests: flume::Receiver<Request>,
    stop: flume::Sender<()>,
    release_metadata: flume::Sender<()>,
    finished: flume::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl Fixture {
    pub fn new(library: Library) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            data: dir.path().join("data"),
            cache: dir.path().join("cache"),
        };
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::create_dir_all(&paths.cache).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let account = Account {
            url: format!("http://{}/", listener.local_addr().unwrap()),
            username: "fixture".into(),
        };
        paths.save_account(&account).unwrap();
        paths
            .save_credentials(
                &account,
                &ServerCredentials::from_password("synthetic-password").unwrap(),
            )
            .unwrap();
        let (received, requests) = flume::unbounded();
        let (release_metadata, metadata_gate) = flume::bounded(1);
        let state = ServerState {
            data: Arc::new(Mutex::new(StateData {
                library,
                requests: vec![],
                expected_mutations: VecDeque::new(),
                unexpected: vec![],
            })),
            received,
            metadata_gate,
        };
        let app = Router::new()
            .route("/rest/{endpoint}", any(navidrome))
            .route("/metadata/{endpoint}", any(metadata))
            .route(
                "/listens/popularity/top-release-groups-for-artist/mb-artist",
                any(listens),
            )
            .fallback(unexpected)
            .with_state(state.clone());
        let (stop, stopped) = flume::bounded(1);
        let (done, finished) = flume::bounded(1);
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = stopped.recv_async().await;
                    })
                    .await
                    .unwrap();
            });
            drop(runtime);
            let _ = done.send(());
        });
        Self {
            _dir: dir,
            paths,
            account,
            state,
            requests,
            stop,
            release_metadata,
            finished,
            thread: Some(thread),
        }
    }

    pub fn catalog(&self) -> Catalog {
        Catalog::open(
            &self
                .paths
                .data
                .join(format!("{}.sqlite", self.account.namespace())),
        )
        .unwrap()
    }

    pub fn seed(&self) {
        let data = self.state.data.lock().unwrap();
        let mut catalog = self.catalog();
        catalog.put_tracks(&data.library.tracks, 1).unwrap();
        catalog.put_albums(&data.library.albums, 1).unwrap();
        catalog
            .save_playlist_summaries(&data.library.playlists, true)
            .unwrap();
    }

    pub fn edit_library(&self, edit: impl FnOnce(&mut Library)) {
        edit(&mut self.state.data.lock().unwrap().library);
    }

    pub fn start(&self) -> Worker {
        let metadata = MusicBrainz::with_endpoints(
            &format!("{}metadata/", self.account.url),
            &format!("{}listens/", self.account.url),
            Duration::ZERO,
        )
        .unwrap();
        Worker::start(self.paths.clone(), metadata).unwrap()
    }

    pub fn expect_mutation(&self, request: Request, succeeds: bool) {
        self.state
            .data
            .lock()
            .unwrap()
            .expected_mutations
            .push_back((request, succeeds));
    }

    pub fn wait_request(&self, expected: &Request) {
        let deadline = Instant::now() + WAIT;
        loop {
            let request = self
                .requests
                .recv_deadline(deadline)
                .expect("expected safe HTTP request summary");
            if &request == expected {
                return;
            }
        }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.state.data.lock().unwrap().requests.clone()
    }

    fn stop(&mut self) {
        if self.thread.is_some() {
            let _ = self.release_metadata.try_send(());
            let _ = self.stop.try_send(());
            self.finished
                .recv_timeout(WAIT)
                .expect("loopback server shutdown");
            self.thread
                .take()
                .unwrap()
                .join()
                .expect("loopback server thread");
        }
    }

    pub fn finish(mut self) {
        self.stop();
        let data = self.state.data.lock().unwrap();
        let mut counts = BTreeMap::<&str, usize>::new();
        for request in &data.requests {
            *counts.entry(&request.endpoint).or_default() += 1;
        }
        let mutations: Vec<_> = data
            .requests
            .iter()
            .filter(|request| {
                matches!(
                    request.endpoint.as_str(),
                    "star.view" | "unstar.view" | "setRating.view"
                )
            })
            .collect();
        eprintln!(
            "scenario={} HTTP counts={counts:?} mutations={mutations:?}",
            scenario()
        );
        assert!(data.unexpected.is_empty(), "{:?}", data.unexpected);
        assert!(
            data.expected_mutations.is_empty(),
            "expected mutation never reached HTTP"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Release a gated request even on a failed assertion.
        self.stop();
    }
}

pub fn event(worker: &Worker, wanted: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + WAIT;
    loop {
        let event = worker
            .events()
            .recv_deadline(deadline)
            .expect("worker event before deadline");
        // Never dump Event::Ready (preferences may contain endpoint settings).
        let name = match &event {
            Event::Ready(..) => "Ready",
            Event::View(..) => "View",
            Event::AlbumUpdated(..) => "AlbumUpdated",
            Event::TrackUpdated(..) => "TrackUpdated",
            Event::MutationDone(..) => "MutationDone",
            Event::MutationFailed(..) => "MutationFailed",
            Event::Error(..) => "Error",
            Event::Status(..) => "Status",
            Event::Connection(..) => "Connection",
            Event::Discography(..) => "Discography",
            _ => "other event",
        };
        eprintln!("scenario={} worker event={name}", scenario());
        if wanted(&event) {
            return event;
        }
        assert!(
            !matches!(event, Event::Error(_) | Event::MutationFailed(..)),
            "unexpected worker failure"
        );
    }
}

fn scenario() -> String {
    std::thread::current()
        .name()
        .unwrap_or("service-worker")
        .to_string()
}

pub fn synced(worker: &Worker) {
    event(
        worker,
        |event| matches!(event, Event::Status(message) if message.starts_with("Up to date")),
    );
}

pub fn view(worker: &Worker) -> LibraryView {
    let epoch = worker.service().query(LibraryQuery::default());
    let Event::View(_, view) = event(
        worker,
        |event| matches!(event, Event::View(request, _) if *request == epoch),
    ) else {
        unreachable!()
    };
    view
}

pub fn mutate(worker: &Worker, mutation: Mutation, succeeds: bool) -> Arc<Album> {
    worker.service().send(Command::Mutate(mutation.clone()));
    let Event::AlbumUpdated(album) = event(worker, |event| matches!(event, Event::AlbumUpdated(_)))
    else {
        unreachable!()
    };
    let terminal = worker
        .events()
        .recv_timeout(WAIT)
        .expect("mutation terminal event");
    match terminal {
        Event::MutationDone(actual) if succeeds => assert_eq!(actual, mutation),
        Event::MutationFailed(actual, error) if !succeeds => {
            assert_eq!(actual, mutation);
            assert!(error.contains("Fixture feedback rejected") && error.contains("Retry"));
            assert!(!error.contains("synthetic-password") && !error.contains("?u="));
        }
        _ => panic!("wrong mutation terminal event or event ordering"),
    }
    album
}
