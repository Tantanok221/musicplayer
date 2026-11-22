use crate::fixture::{Fixture, Library, Request, WAIT, synced};
use axum::{
    Json, Router,
    extract::State,
    http::{Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::json;
use sonora::{
    model::Preferences,
    service::{Command, DownloadRequest, Event, test_support::Worker},
    slskd::{Candidate, Completeness, DownloadJob, JobState, RemoteFile},
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const FILE: &str = "Album\\track.flac";
const DOWNLOADS: &str = "downloads:v1";

#[derive(Clone)]
struct Replies {
    reject: bool,
    state: &'static str,
    requests: Arc<Mutex<Vec<(Method, String)>>>,
}

async fn reply(State(state): State<Replies>, method: Method, uri: Uri) -> Response {
    state
        .requests
        .lock()
        .unwrap()
        .push((method.clone(), uri.path().into()));
    match (method, uri.path()) {
        (Method::POST, "/api/v0/users/peer/directory") => Json(json!([])).into_response(),
        (Method::POST, "/api/v0/transfers/downloads/batches") => if state.reject {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::CREATED
        }
        .into_response(),
        (Method::GET, "/api/v0/transfers/downloads/peer") => {
            Json(json!({"directories":[{"files":[{"filename":FILE,"state":state.state}]}]}))
                .into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

struct Stub {
    url: String,
    replies: Replies,
    stop: flume::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Stub {
    fn new(reject: bool, state: &'static str) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let replies = Replies {
            reject,
            state,
            requests: Arc::default(),
        };
        let app = Router::new()
            .fallback(any(reply))
            .with_state(replies.clone());
        let (stop, stopped) = flume::bounded(1);
        let thread = std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async move {
                    axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), app)
                        .with_graceful_shutdown(async move {
                            let _ = stopped.recv_async().await;
                        })
                        .await
                        .unwrap();
                });
        });
        Self {
            url,
            replies,
            stop,
            thread: Some(thread),
        }
    }

    fn configure(&self, fixture: &Fixture) {
        let catalog = fixture.catalog();
        let mut prefs: Preferences = catalog.state("preferences").unwrap();
        prefs.downloads.enabled = true;
        prefs.downloads.url = self.url.clone();
        catalog.set_state("preferences", &prefs).unwrap();
    }

    fn count(&self, method: Method, path: &str) -> usize {
        self.replies
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, p)| *m == method && p == path)
            .count()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        self.thread.take().unwrap().join().unwrap();
    }
}

fn job() -> DownloadJob {
    DownloadJob {
        release: "release".into(),
        username: "peer".into(),
        files: vec![FILE.into()],
        ..Default::default()
    }
}

fn request() -> Command {
    Command::Download(Box::new(DownloadRequest {
        artist: "Artist".into(),
        artist_id: "artist".into(),
        release: "release".into(),
        title: "Album".into(),
        expected: Some(1),
        candidate: Candidate {
            username: "peer".into(),
            directory: "Album".into(),
            files: vec![RemoteFile {
                filename: FILE.into(),
                size: 123,
                ..Default::default()
            }],
            tracks: 1,
            locked: 0,
            size: 123,
            seconds: 0,
            format: "FLAC".into(),
            lossless: true,
            completeness: Completeness::Exact,
            free_slot: true,
            queue: 0,
            speed: 0,
        },
    }))
}

fn wait(worker: &Worker, wanted: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let event = worker
            .events()
            .recv_deadline(deadline)
            .expect("download lifecycle event before deadline");
        assert!(
            !matches!(event, Event::Error(_)),
            "unexpected service error"
        );
        if wanted(&event) {
            return event;
        }
    }
}

fn downloads(worker: &Worker, state: JobState) -> DownloadJob {
    let Event::Downloads(jobs) = wait(
        worker,
        |event| matches!(event, Event::Downloads(jobs) if jobs.iter().any(|job| job.release == "release" && job.state == state)),
    ) else {
        unreachable!()
    };
    jobs.iter()
        .find(|job| job.release == "release")
        .unwrap()
        .clone()
}

#[test]
fn download_finishing_scans_navidrome_and_refreshes_ui() {
    let fixture = Fixture::new(Library::default());
    let stub = Stub::new(false, "Completed, Succeeded");
    stub.configure(&fixture);
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(request());
    downloads(&worker, JobState::Active);
    let completed = downloads(&worker, JobState::Downloaded);
    assert_eq!(completed.progress.done, 1);
    wait(
        &worker,
        |e| matches!(e, Event::Status(s) if s.contains("Navidrome is scanning")),
    );
    fixture.wait_request(&Request::new("startScan.view", &[]));
    wait(&worker, |e| matches!(e, Event::ArtworkRetrying));
    synced(&worker);
    assert_eq!(
        stub.count(Method::POST, "/api/v0/transfers/downloads/batches"),
        1
    );
    assert!(stub.count(Method::GET, "/api/v0/transfers/downloads/peer") > 0);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn rejected_download_surfaces_failure_and_leaves_no_active_job() {
    let fixture = Fixture::new(Library::default());
    let stub = Stub::new(true, "Queued, Remotely");
    stub.configure(&fixture);
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(request());
    wait(
        &worker,
        |e| matches!(e, Event::DownloadNotStarted(id, error) if id == "release" && error.contains("500")),
    );
    worker.service().send(Command::ResumeDownloads);
    wait(
        &worker,
        |e| matches!(e, Event::Downloads(jobs) if jobs.is_empty()),
    );
    assert!(
        fixture
            .catalog()
            .state::<Vec<DownloadJob>>(DOWNLOADS)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stub.count(Method::GET, "/api/v0/transfers/downloads/peer"),
        0
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn persisted_active_download_resumes_after_worker_restart() {
    let fixture = Fixture::new(Library::default());
    let stub = Stub::new(false, "Completed, Errored");
    stub.configure(&fixture);
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::DownloadUpdate(fixture.account.namespace(), job()));
    downloads(&worker, JobState::Active);
    worker.shutdown(WAIT).unwrap();
    let mut restarted = fixture.start();
    restarted.wait_ready(WAIT).unwrap();
    downloads(&restarted, JobState::Active);
    restarted.service().send(Command::ResumeDownloads);
    downloads(&restarted, JobState::Failed);
    assert!(stub.count(Method::GET, "/api/v0/transfers/downloads/peer") > 0);
    assert_eq!(
        stub.count(Method::POST, "/api/v0/transfers/downloads/batches"),
        0
    );
    restarted.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn another_accounts_download_update_is_ignored() {
    let fixture = Fixture::new(Library::default());
    let stub = Stub::new(false, "Completed, Succeeded");
    stub.configure(&fixture);
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::DownloadUpdate("another-account".into(), job()));
    worker.service().send(Command::ResumeDownloads);
    let Event::Downloads(jobs) = wait(&worker, |e| matches!(e, Event::Downloads(_))) else {
        unreachable!()
    };
    assert!(jobs.is_empty());
    assert!(
        fixture
            .catalog()
            .state::<Vec<DownloadJob>>(DOWNLOADS)
            .unwrap()
            .is_empty()
    );
    assert!(stub.replies.requests.lock().unwrap().is_empty());
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn terminal_transfer_failure_after_requeues_fails_job() {
    let fixture = Fixture::new(Library::default());
    let stub = Stub::new(false, "Completed, Errored");
    stub.configure(&fixture);
    let exhausted = DownloadJob {
        requeued: 3,
        ..job()
    };
    fixture
        .catalog()
        .set_state(DOWNLOADS, &vec![exhausted])
        .unwrap();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    let failed = downloads(&worker, JobState::Failed);
    assert_eq!(failed.requeued, 3);
    assert_eq!(failed.progress.failed, 1);
    assert_eq!(failed.progress.failure, "errored");
    assert_eq!(
        fixture
            .catalog()
            .state::<Vec<DownloadJob>>(DOWNLOADS)
            .unwrap(),
        vec![failed]
    );
    assert_eq!(
        stub.count(Method::POST, "/api/v0/transfers/downloads/batches"),
        0
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn dismiss_download_removes_persisted_job() {
    let fixture = Fixture::new(Library::default());
    let stub = Stub::new(false, "Queued, Remotely");
    stub.configure(&fixture);
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker
        .service()
        .send(Command::DownloadUpdate(fixture.account.namespace(), job()));
    downloads(&worker, JobState::Active);
    worker
        .service()
        .send(Command::DismissDownload("release".into()));
    wait(
        &worker,
        |e| matches!(e, Event::Downloads(jobs) if jobs.is_empty()),
    );
    assert!(
        fixture
            .catalog()
            .state::<Vec<DownloadJob>>(DOWNLOADS)
            .unwrap()
            .is_empty()
    );
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}
