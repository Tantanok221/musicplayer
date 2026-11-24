//! Self-update against a loopback GitHub API: signed manifests, checksums,
//! commit ordering, and replacing the installed executable.
use super::fixture::{Fixture, Library, WAIT};
use axum::{
    Json, Router,
    extract::{Path as UrlPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use sonora::{
    service::{Command, Event},
    update::{UpdateState, UpdateStatus, Updater},
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

const CURRENT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NEWER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NEW_BINARY: &[u8] = b"new sonora build";

#[derive(Clone)]
struct Release {
    manifest: Vec<u8>,
    signature: Vec<u8>,
    binary: Vec<u8>,
    /// GitHub's comparison of the current commit with the release, or `None` for 404.
    comparison: Option<&'static str>,
}

#[derive(Clone)]
struct Api {
    base: String,
    release: Arc<Mutex<Release>>,
    binary_downloads: Arc<Mutex<usize>>,
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

fn signed(commit: &str, binary: &[u8]) -> Release {
    let manifest = format!(
        "sonora-update-v1\ntarget=linux-x86_64\ncommit={commit}\nsha256={}\n",
        Sha256::digest(binary)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
    .into_bytes();
    Release {
        signature: key().sign(&manifest).to_bytes().to_vec(),
        manifest,
        binary: binary.to_vec(),
        comparison: Some("ahead"),
    }
}

async fn latest(State(api): State<Api>) -> Json<serde_json::Value> {
    let asset = |name: &str| {
        serde_json::json!({
            "name": name,
            "browser_download_url": format!("{}assets/{name}", api.base),
        })
    };
    Json(serde_json::json!({
        "tag_name": "sonora-bbbbbbb",
        "assets": [
            asset("sonora-linux-x86_64"),
            asset("sonora-linux-x86_64.manifest"),
            asset("sonora-linux-x86_64.manifest.sig"),
        ],
    }))
}

async fn asset(State(api): State<Api>, UrlPath(name): UrlPath<String>) -> Vec<u8> {
    let release = api.release.lock().unwrap().clone();
    match name.as_str() {
        "sonora-linux-x86_64" => {
            *api.binary_downloads.lock().unwrap() += 1;
            release.binary
        }
        "sonora-linux-x86_64.manifest" => release.manifest,
        _ => release.signature,
    }
}

async fn compare(State(api): State<Api>, UrlPath(range): UrlPath<String>) -> impl IntoResponse {
    let release = api.release.lock().unwrap().clone();
    assert!(range.starts_with(&format!("{CURRENT}...")), "{range}");
    match release.comparison {
        Some(status) => Json(serde_json::json!({ "status": status })).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

struct Setup {
    _dir: tempfile::TempDir,
    executable: PathBuf,
    api: Api,
    relaunched: Arc<Mutex<Vec<PathBuf>>>,
    task: tokio::task::JoinHandle<()>,
}

async fn setup(release: Release) -> (Setup, Updater) {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("sonora");
    std::fs::write(&executable, b"old sonora build").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!(
        "http://{}/repos/owner/sonora/",
        listener.local_addr().unwrap()
    );
    let api = Api {
        base: base.clone(),
        release: Arc::new(Mutex::new(release)),
        binary_downloads: Arc::default(),
    };
    let router = Router::new()
        .route("/repos/owner/sonora/releases/latest", get(latest))
        .route("/repos/owner/sonora/assets/{name}", get(asset))
        .route("/repos/owner/sonora/compare/{range}", get(compare))
        .with_state(api.clone());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let relaunched = Arc::new(Mutex::new(Vec::new()));
    let record = relaunched.clone();
    let updater = Updater::new(&base, CURRENT, executable.clone(), key().verifying_key())
        .unwrap()
        .with_relaunch(Arc::new(move |path| {
            record.lock().unwrap().push(path.to_owned());
            Ok(())
        }));
    (
        Setup {
            _dir: dir,
            executable,
            api,
            relaunched,
            task,
        },
        updater,
    )
}

fn update_status(
    worker: &sonora::service::test_support::Worker,
    wanted: impl Fn(&UpdateState) -> bool,
) -> UpdateStatus {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        if let Event::Update(status) = worker
            .events()
            .recv_deadline(deadline)
            .expect("update status before deadline")
            && wanted(&status.state)
        {
            return status;
        }
    }
}

fn finished(state: &UpdateState) -> bool {
    !matches!(state, UpdateState::Checking | UpdateState::Idle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_newer_signed_release_replaces_the_executable_once() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let (setup, updater) = setup(signed(NEWER, NEW_BINARY)).await;
    let mut worker = fixture.start_with_updater(updater);
    worker.wait_ready(WAIT).unwrap();

    worker.service().send(Command::CheckForUpdates);
    let status = update_status(&worker, finished);
    assert_eq!(status.current, CURRENT);
    assert_eq!(status.state, UpdateState::Ready(NEWER.into()));
    assert_eq!(std::fs::read(&setup.executable).unwrap(), NEW_BINARY);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&setup.executable)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    // Checking again keeps the installed update without downloading it again.
    worker.service().send(Command::CheckForUpdates);
    assert_eq!(
        update_status(&worker, finished).state,
        UpdateState::Ready(NEWER.into())
    );
    assert_eq!(*setup.api.binary_downloads.lock().unwrap(), 1);

    // Restarting starts the installed executable, then the UI quits.
    worker.service().send(Command::RestartToUpdate);
    super::fixture::event(&worker, |event| matches!(event, Event::Relaunching));
    assert_eq!(
        *setup.relaunched.lock().unwrap(),
        std::slice::from_ref(&setup.executable)
    );

    worker.shutdown(WAIT).unwrap();
    setup.task.abort();
    fixture.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsigned_tampered_or_older_releases_leave_the_executable_alone() {
    let mut tampered = signed(NEWER, NEW_BINARY);
    tampered.binary = b"something else".to_vec();
    let mut forged = signed(NEWER, NEW_BINARY);
    forged.signature = SigningKey::from_bytes(&[9; 32])
        .sign(&forged.manifest)
        .to_bytes()
        .to_vec();
    let mut older = signed(NEWER, NEW_BINARY);
    older.comparison = Some("behind");
    let mut unknown = signed(NEWER, NEW_BINARY);
    unknown.comparison = None;
    let cases = [
        (tampered, false),
        (forged, false),
        (older, true),
        (unknown, true),
        (signed(CURRENT, NEW_BINARY), true),
    ];
    for (release, up_to_date) in cases {
        let fixture = Fixture::new(Library::default());
        fixture.seed();
        let (setup, updater) = setup(release).await;
        let mut worker = fixture.start_with_updater(updater);
        worker.wait_ready(WAIT).unwrap();
        worker.service().send(Command::CheckForUpdates);
        let state = update_status(&worker, finished).state;
        if up_to_date {
            assert_eq!(state, UpdateState::UpToDate);
        } else {
            assert!(matches!(state, UpdateState::Failed(_)), "{state:?}");
        }
        assert_eq!(
            std::fs::read(&setup.executable).unwrap(),
            b"old sonora build"
        );
        // Without an installed update there is nothing to restart into.
        worker.service().send(Command::RestartToUpdate);
        worker.service().send(Command::CheckForUpdates);
        update_status(&worker, finished);
        assert!(setup.relaunched.lock().unwrap().is_empty());
        assert!(
            worker
                .events()
                .try_iter()
                .all(|event| !matches!(event, Event::Relaunching))
        );
        worker.shutdown(WAIT).unwrap();
        setup.task.abort();
        fixture.finish();
    }
}
