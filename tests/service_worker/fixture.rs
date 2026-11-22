use axum::{
    Json, Router,
    extract::{Path, Query, RawQuery, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};
use sonora::{
    catalog::{Catalog, LibraryView},
    lastfm::LastFm,
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
    song_ids: Vec<String>,
}

impl Request {
    pub fn new(endpoint: &str, params: &[(&str, &str)]) -> Self {
        Self {
            endpoint: endpoint.into(),
            params: params
                .iter()
                .filter(|(key, _)| *key != "songId")
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            song_ids: params
                .iter()
                .filter(|(key, _)| *key == "songId")
                .map(|(_, value)| value.to_string())
                .collect(),
        }
    }
}

/// A fixture Last.fm chart entry: artist, album and plays.
pub type ChartEntry = (&'static str, &'static str, u64);

pub struct Library {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub playlists: Vec<Playlist>,
    pub fail_song_offset: Option<usize>,
    pub fail_playlist: bool,
    pub scanning: bool,
    pub hold_metadata: bool,
    pub offline: bool,
    pub hold_sync: bool,
    pub hold_scrobble: bool,
    pub fail_artwork: bool,
    pub fail_enrichment: bool,
    pub releases: Vec<(&'static str, &'static str, &'static str)>,
    pub artist_mbid: &'static str,
    pub prolific: bool,
    /// Unix seconds the fixture Last.fm profile was created.
    pub lastfm_registered: i64,
    /// Album charts by season start: artist, album and plays.
    pub lastfm_charts: Vec<(i64, Vec<ChartEntry>)>,
    /// A Last.fm error code every Last.fm request answers with.
    pub lastfm_error: Option<u32>,
    /// Whether the worker is built with a Last.fm API key.
    pub lastfm_key: bool,
    /// Whether the account may start a scan, as Navidrome administrators can.
    pub scan_admin: bool,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            tracks: vec![song("song", "album"), song("other-song", "other-album")],
            albums: vec![album("album"), album("other-album")],
            playlists: vec![],
            fail_song_offset: None,
            fail_playlist: false,
            scanning: false,
            hold_metadata: false,
            offline: false,
            hold_sync: false,
            hold_scrobble: false,
            fail_artwork: false,
            fail_enrichment: false,
            releases: vec![("missing-release", "Missing Release", "2020-01-01")],
            artist_mbid: "mb-artist",
            prolific: false,
            lastfm_registered: 0,
            lastfm_charts: vec![],
            lastfm_error: None,
            lastfm_key: true,
            scan_admin: false,
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
    sync_gate: flume::Receiver<()>,
    scrobble_gate: flume::Receiver<()>,
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
    RawQuery(raw_query): RawQuery,
) -> Response {
    let mut query = BTreeMap::new();
    let mut song_ids = Vec::new();
    for (key, value) in url::form_urlencoded::parse(raw_query.as_deref().unwrap_or("").as_bytes()) {
        if endpoint == "createPlaylist.view" && key == "songId" {
            song_ids.push(value.into_owned());
        } else if query.insert(key.into_owned(), value.into_owned()).is_some() {
            return state.unexpected("repeated parameter");
        }
    }
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
            | "startScan.view"
            | "getPlaylists.view"
            | "getPlaylist.view"
            | "createPlaylist.view"
            | "updatePlaylist.view"
            | "search3.view"
            | "star.view"
            | "unstar.view"
            | "setRating.view"
            | "getArtist.view"
            | "getTopSongs.view"
            | "getArtistInfo2.view"
            | "scrobble.view"
            | "getSong.view"
            | "getCoverArt.view"
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
                | "submission"
                | "time"
                | "size"
                | "includeNotPresent"
                | "playlistId"
                | "songIndexToRemove"
        )
    }) {
        return state.unexpected("unknown endpoint or parameter");
    }
    let request = Request {
        endpoint: endpoint.clone(),
        params: query.clone(),
        song_ids,
    };
    state.record(request.clone());
    let (offline, hold_sync, hold_scrobble, fail_artwork) = {
        let data = state.data.lock().unwrap();
        (
            data.library.offline,
            data.library.hold_sync,
            data.library.hold_scrobble,
            data.library.fail_artwork,
        )
    };
    if offline {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if endpoint == "getScanStatus.view" && hold_sync {
        let _ = state.sync_gate.recv_async().await;
    }
    if endpoint == "scrobble.view" {
        let succeeds = {
            let mut data = state.data.lock().unwrap();
            match data.expected_mutations.pop_front() {
                Some((expected, succeeds)) if expected == request => succeeds,
                _ => {
                    data.unexpected.push("unexpected scrobble");
                    false
                }
            }
        };
        if hold_scrobble {
            let _ = state.scrobble_gate.recv_async().await;
        }
        return envelope(if succeeds {
            json!({"status":"ok"})
        } else {
            json!({"status":"failed","error":{"code":70,"message":"Fixture scrobble rejected"}})
        });
    }
    if endpoint == "getCoverArt.view" {
        if query != Request::new("", &[("id", "cover"), ("size", "512")]).params {
            return state.unexpected("invalid cover request");
        }
        if fail_artwork {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let image = image::RgbaImage::from_pixel(32, 32, image::Rgba([120, 40, 80, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        return bytes.into_inner().into_response();
    }
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
        "startScan.view" if query.is_empty() && data.library.scan_admin => {
            json!({"status":"ok","scanStatus":{"scanning":false}})
        }
        "startScan.view" if query.is_empty() => {
            json!({"status":"failed","error":{"code":50,"message":"Not an administrator"}})
        }
        "getPlaylists.view" if query.is_empty() => {
            json!({"status":"ok","playlists":{"playlist":data.library.playlists}})
        }
        "getPlaylist.view" if data.library.fail_playlist => {
            json!({"status":"failed","error":{"code":70,"message":"Fixture playlist unavailable"}})
        }
        "getPlaylist.view" => {
            let playlist = data
                .library
                .playlists
                .iter()
                .find(|playlist| Some(&playlist.id) == query.get("id"));
            json!({"status":"ok","playlist":playlist})
        }
        "createPlaylist.view" => match data.expected_mutations.pop_front() {
            Some((expected, succeeds)) if request == expected => {
                if succeeds {
                    let entries: Vec<_> = request
                        .song_ids
                        .iter()
                        .map(|id| {
                            Arc::new(
                                data.library
                                    .tracks
                                    .iter()
                                    .find(|track| &track.id == id)
                                    .unwrap()
                                    .clone(),
                            )
                        })
                        .collect();
                    let playlist = data
                        .library
                        .playlists
                        .iter_mut()
                        .find(|playlist| Some(&playlist.id) == query.get("playlistId"))
                        .unwrap();
                    playlist.entry = entries.into();
                    json!({"status":"ok","playlist":playlist})
                } else {
                    json!({"status":"failed","error":{"code":70,"message":"Fixture reorder rejected"}})
                }
            }
            _ => {
                data.unexpected
                    .push("unexpected playlist replacement or song order");
                json!({"status":"failed","error":{"code":70,"message":"Unexpected mutation"}})
            }
        },
        "updatePlaylist.view" => match data.expected_mutations.pop_front() {
            Some((expected, succeeds)) if request == expected => {
                if succeeds {
                    let playlist = data
                        .library
                        .playlists
                        .iter_mut()
                        .find(|playlist| Some(&playlist.id) == query.get("playlistId"))
                        .unwrap();
                    let index: usize = query["songIndexToRemove"].parse().unwrap();
                    Arc::make_mut(&mut playlist.entry).remove(index);
                    playlist.song_count = playlist.entry.len() as u32;
                    playlist.duration = playlist.entry.iter().map(|track| track.duration).sum();
                    json!({"status":"ok"})
                } else {
                    json!({"status":"failed","error":{"code":70,"message":"Fixture removal rejected"}})
                }
            }
            _ => {
                data.unexpected
                    .push("unexpected playlist mutation or parameters");
                json!({"status":"failed","error":{"code":70,"message":"Unexpected mutation"}})
            }
        },
        "getArtist.view" if query == BTreeMap::from([("id".into(), "artist".into())]) => {
            json!({"status":"ok","artist":{"id":"artist","name":"Fixture Artist","musicBrainzId":data.library.artist_mbid}})
        }
        "getArtistInfo2.view"
            if query
                == Request::new(
                    "",
                    &[
                        ("id", "artist"),
                        ("count", "12"),
                        ("includeNotPresent", "true"),
                    ],
                )
                .params =>
        {
            if data.library.fail_enrichment {
                json!({"status":"failed", "error":{"code":70,"message":"Fixture enrichment failed"}})
            } else {
                json!({"status":"ok", "artistInfo2":{"biography":"Fixture biography"}})
            }
        }
        "getTopSongs.view"
            if query
                == Request::new("", &[("artist", "Fixture Artist"), ("count", "50")]).params =>
        {
            if data.library.fail_enrichment {
                json!({"status":"failed", "error":{"code":70,"message":"Fixture enrichment failed"}})
            } else {
                json!({"status":"ok","topSongs":{"song":[]}})
            }
        }
        // A profile lookup by name for an artist the server does not have.
        "search3.view"
            if query
                == Request::new(
                    "",
                    &[
                        ("query", "Fixture Artist"),
                        ("artistCount", "500"),
                        ("albumCount", "0"),
                        ("songCount", "0"),
                    ],
                )
                .params =>
        {
            json!({"status":"ok","searchResult3":{"artist":[]}})
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
        "getSong.view" if query == Request::new("", &[("id", "song")]).params => {
            json!({"status":"ok", "song":data.library.tracks[0]})
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

pub const LASTFM_KEY: &str = "fixture-lastfm-key";

async fn lastfm(
    State(state): State<ServerState>,
    method: Method,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    let param = |key: &str| query.get(key).cloned().unwrap_or_default();
    if method != Method::GET
        || param("api_key") != LASTFM_KEY
        || param("format") != "json"
        || param("user") != "listener"
    {
        return state.unexpected("invalid Last.fm request");
    }
    let (registered, charts, error) = {
        let data = state.data.lock().unwrap();
        (
            data.library.lastfm_registered,
            data.library.lastfm_charts.clone(),
            data.library.lastfm_error,
        )
    };
    let (endpoint, params) = match param("method").as_str() {
        "user.getinfo" if query.len() == 4 => ("lastfm/getinfo", vec![]),
        "user.getweeklyalbumchart" if query.len() == 6 => (
            "lastfm/chart",
            vec![("from", param("from")), ("to", param("to"))],
        ),
        _ => return state.unexpected("unknown Last.fm method"),
    };
    let params = params
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    state.record(Request::new(endpoint, &params));
    if let Some(code) = error {
        let message = match code {
            6 => "User not found",
            17 => "Login: User required to be logged in",
            _ => "Fixture error",
        };
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": code, "message": message})),
        )
            .into_response();
    }
    if endpoint == "lastfm/getinfo" {
        return Json(json!({"user": {"name": "listener", "registered": {
            "unixtime": registered.to_string(), "#text": registered
        }}}))
        .into_response();
    }
    let from: i64 = param("from").parse().unwrap_or_default();
    let albums = charts
        .iter()
        .find(|(start, _)| *start == from)
        .map(|(_, albums)| albums.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|(artist, name, plays)| {
            json!({"artist": {"mbid": "", "#text": artist}, "name": name, "mbid": "",
                   "playcount": plays.to_string(), "@attr": {"rank": "1"}})
        })
        .collect::<Vec<_>>();
    // Last.fm sends a lone chart entry as an object rather than a one-item list.
    let album = match albums.as_slice() {
        [one] => one.clone(),
        _ => json!(albums),
    };
    Json(json!({"weeklyalbumchart": {"album": album, "@attr": {"user": "listener"}}}))
        .into_response()
}

async fn metadata(
    State(state): State<ServerState>,
    Path(endpoint): Path<String>,
    method: Method,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    if method == Method::GET
        && endpoint == "release"
        && query
            == Request::new(
                "",
                &[
                    ("release-group", "missing-release"),
                    ("inc", "media url-rels"),
                    ("status", "official"),
                    ("limit", "100"),
                    ("fmt", "json"),
                ],
            )
            .params
    {
        state.record(Request::new("metadata/editions", &[]));
        return Json(json!({"releases":[{"id":"edition", "date":"2020-01-01", "media":[{"track-count":1}]}]})).into_response();
    }
    if method == Method::GET
        && endpoint == "release/edition"
        && query
            == Request::new(
                "",
                &[
                    (
                        "inc",
                        "recordings artist-credits labels url-rels release-group-level-rels genres",
                    ),
                    ("fmt", "json"),
                ],
            )
            .params
    {
        state.record(Request::new("metadata/details", &[]));
        return Json(json!({"id":"edition", "media":[{"position":1, "tracks":[{"number":"1", "title":"Missing song", "length":120000}]}]})).into_response();
    }
    if method == Method::GET
        && endpoint == "artist"
        && query
            == Request::new(
                "",
                &[
                    (
                        "query",
                        "artist:\"Fixture Artist\" OR alias:\"Fixture Artist\"",
                    ),
                    ("limit", "5"),
                    ("fmt", "json"),
                ],
            )
            .params
    {
        state.record(Request::new("metadata/artist-search", &[]));
        return Json(json!({"artists":[{"id":"mb-artist","name":"Fixture Artist"}]}))
            .into_response();
    }
    if method == Method::GET
        && endpoint == "release-group"
        && query
            == Request::new(
                "",
                &[("artist", "mb-artist"), ("limit", "100"), ("fmt", "json")],
            )
            .params
    {
        state.record(Request::new("metadata/artist-check", &[]));
        return Json(json!({"release-groups":[{"id":"owned","title":"Release album"}]}))
            .into_response();
    }
    if method == Method::GET
        && endpoint == "release-group"
        && query
            == Request::new(
                "",
                &[
                    ("artist", "mb-artist"),
                    ("release-group-status", "website-default"),
                    ("limit", "100"),
                    ("offset", "0"),
                    ("fmt", "json"),
                ],
            )
            .params
    {
        state.record(Request::new("metadata/release-groups", &[]));
        let releases = state.data.lock().unwrap().library.releases.clone();
        let groups = releases
            .iter()
            .map(|(id, title, date)| {
                json!({"id":id,"title":title,"primary-type":"Album","first-release-date":date})
            })
            .collect::<Vec<_>>();
        return Json(json!({"release-group-count":groups.len(),"release-groups":groups}))
            .into_response();
    }
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
    let (hold, releases, prolific) = {
        let data = state.data.lock().unwrap();
        (
            data.library.hold_metadata,
            data.library.releases.clone(),
            data.library.prolific,
        )
    };
    if hold {
        let _ = state.metadata_gate.recv_async().await;
    }
    let releases = releases
        .iter()
        .map(|(id, title, date)| {
            json!({"id":format!("edition-{id}"),"status":"Official",
                "release-group":{"id":id,"title":title,
                    "primary-type":"Album","first-release-date":date}})
        })
        .collect::<Vec<_>>();
    let count = if prolific { 1001 } else { releases.len() };
    Json(json!({"release-count":count,"releases":releases})).into_response()
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
    release_sync: flume::Sender<()>,
    release_scrobble: flume::Sender<()>,
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
        let (release_sync, sync_gate) = flume::unbounded();
        let (release_scrobble, scrobble_gate) = flume::unbounded();
        let state = ServerState {
            data: Arc::new(Mutex::new(StateData {
                library,
                requests: vec![],
                expected_mutations: VecDeque::new(),
                unexpected: vec![],
            })),
            received,
            metadata_gate,
            sync_gate,
            scrobble_gate,
        };
        let app = Router::new()
            .route("/rest/{endpoint}", any(navidrome))
            .route("/metadata/{*endpoint}", any(metadata))
            .route("/lastfm/", any(lastfm))
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
            release_sync,
            release_scrobble,
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
        self.start_player(false)
    }

    pub fn start_paused(&self) -> Worker {
        self.start_player(true)
    }

    fn start_player(&self, paused: bool) -> Worker {
        let metadata =
            MusicBrainz::with_endpoints(&format!("{}metadata/", self.account.url), Duration::ZERO)
                .unwrap();
        let key = self.state.data.lock().unwrap().library.lastfm_key;
        let lastfm = LastFm::with_endpoints(
            &format!("{}lastfm/", self.account.url),
            key.then(|| LASTFM_KEY.to_string()),
            Duration::ZERO,
        )
        .unwrap();
        if paused {
            Worker::start_paused(self.paths.clone(), metadata, lastfm).unwrap()
        } else {
            Worker::start(self.paths.clone(), metadata, lastfm).unwrap()
        }
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

    pub fn resume_sync(&self) {
        self.edit_library(|library| library.hold_sync = false);
        let _ = self.release_sync.send(());
    }

    pub fn resume_scrobble(&self) {
        self.edit_library(|library| library.hold_scrobble = false);
        let _ = self.release_scrobble.send(());
    }

    fn stop(&mut self) {
        if self.thread.is_some() {
            let _ = self.release_metadata.try_send(());
            self.resume_sync();
            self.resume_scrobble();
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
            Event::Discover(..) => "Discover",
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
