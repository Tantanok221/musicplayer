//! Self-update against a loopback GitHub API: signed manifests, checksums,
//! commit ordering, and replacing the installed Linux executable or macOS app
//! bundle. Bundle scenarios use a zip extractor and verifier standing in for
//! `ditto` and `codesign`, so they run on every platform; the macOS-only
//! scenario at the end uses the real tools.
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
    update::{BundleTools, Target, UpdateState, UpdateStatus, Updater, swap_dirs},
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;

const CURRENT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NEWER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NEW_BINARY: &[u8] = b"new sonora build";

#[derive(Clone)]
struct Release {
    target: Target,
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
    signed_for(Target::LinuxX86_64, commit, binary)
}

fn signed_for(target: Target, commit: &str, binary: &[u8]) -> Release {
    let manifest = format!(
        "sonora-update-v1\ntarget={}\ncommit={commit}\nsha256={}\n",
        target.name(),
        Sha256::digest(binary)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
    .into_bytes();
    Release {
        target,
        signature: key().sign(&manifest).to_bytes().to_vec(),
        manifest,
        binary: binary.to_vec(),
        comparison: Some("ahead"),
    }
}

async fn latest(State(api): State<Api>) -> Json<serde_json::Value> {
    let target = api.release.lock().unwrap().target;
    let asset = |name: &str| {
        serde_json::json!({
            "name": name,
            "browser_download_url": format!("{}assets/{name}", api.base),
        })
    };
    Json(serde_json::json!({
        "tag_name": "sonora-bbbbbbb",
        "assets": [
            asset(&target.asset()),
            asset(&target.manifest()),
            asset(&target.signature()),
        ],
    }))
}

async fn asset(State(api): State<Api>, UrlPath(name): UrlPath<String>) -> Vec<u8> {
    let release = api.release.lock().unwrap().clone();
    let target = release.target;
    if name == target.asset() {
        *api.binary_downloads.lock().unwrap() += 1;
        release.binary
    } else if name == target.manifest() {
        release.manifest
    } else {
        assert_eq!(name, target.signature());
        release.signature
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

/// Serves `release` from a loopback GitHub API; returns its API root.
async fn serve(release: Release) -> (String, Api, tokio::task::JoinHandle<()>) {
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
    (base, api, task)
}

fn recording(updater: Updater) -> (Updater, Arc<Mutex<Vec<PathBuf>>>) {
    let relaunched = Arc::new(Mutex::new(Vec::new()));
    let record = relaunched.clone();
    let updater = updater.with_relaunch(Arc::new(move |path| {
        record.lock().unwrap().push(path.to_owned());
        Ok(())
    }));
    (updater, relaunched)
}

async fn setup(release: Release) -> (Setup, Updater) {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("sonora");
    std::fs::write(&executable, b"old sonora build").unwrap();
    let (base, api, task) = serve(release).await;
    let (updater, relaunched) =
        recording(Updater::new(&base, CURRENT, executable.clone(), key().verifying_key()).unwrap());
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
    let mut worker = fixture.start_with_updater(Ok(updater));
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
        let mut worker = fixture.start_with_updater(Ok(updater));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn builds_that_cannot_update_say_why_and_never_check() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let reason = "Move Sonora to Applications to enable updates";
    let mut worker = fixture.start_with_updater(Err(Some(reason.into())));
    worker.wait_ready(WAIT).unwrap();
    let status = update_status(&worker, |_| true);
    assert_eq!(status.state, UpdateState::Unavailable(Some(reason.into())));

    worker.service().send(Command::CheckForUpdates);
    worker.service().send(Command::RestartToUpdate);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    while let Ok(event) = worker.events().recv_deadline(deadline) {
        assert!(
            !matches!(event, Event::Update(_) | Event::Relaunching),
            "{event:?}"
        );
    }
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

/// A zipped `Sonora.app` like the release workflow's, whose executable holds
/// `version`. Without `executable`, the app has no `Contents/MacOS/sonora`.
fn app_zip(version: &[u8], executable: bool) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut buffer);
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file(
        "Sonora.app/Contents/Info.plist",
        options.unix_permissions(0o644),
    )
    .unwrap();
    zip.write_all(b"app.sonora.player").unwrap();
    if executable {
        zip.start_file(
            "Sonora.app/Contents/MacOS/sonora",
            options.unix_permissions(0o755),
        )
        .unwrap();
        zip.write_all(version).unwrap();
    }
    zip.finish().unwrap();
    buffer.into_inner()
}

/// Stands in for `ditto -x -k`.
fn unzip(archive: &Path, into: &Path) -> anyhow::Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    zip.extract(into)?;
    Ok(())
}

struct Bundle {
    _dir: tempfile::TempDir,
    applications: PathBuf,
    app: PathBuf,
    api: Api,
    relaunched: Arc<Mutex<Vec<PathBuf>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Bundle {
    fn executable(&self) -> Vec<u8> {
        std::fs::read(self.app.join("Contents/MacOS/sonora")).unwrap()
    }

    /// What is in the Applications folder besides the installed app.
    fn leftovers(&self) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(&self.applications)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "Sonora.app")
            .collect();
        names.sort();
        names
    }
}

/// An installed `Applications/Sonora.app` with a file only the old version
/// has, updated by `tools`.
async fn bundle_setup(release: Release, tools: BundleTools) -> (Bundle, Updater) {
    let dir = tempfile::tempdir().unwrap();
    let applications = dir.path().join("Applications");
    let app = applications.join("Sonora.app");
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    std::fs::write(app.join("Contents/MacOS/sonora"), b"old sonora build").unwrap();
    std::fs::write(app.join("Contents/old-only"), b"").unwrap();
    let (base, api, task) = serve(release).await;
    let updater =
        Updater::bundle(&base, CURRENT, app.clone(), key().verifying_key(), tools).unwrap();
    let (updater, relaunched) = recording(updater);
    (
        Bundle {
            _dir: dir,
            applications,
            app,
            api,
            relaunched,
            task,
        },
        updater,
    )
}

fn fake_tools() -> BundleTools {
    BundleTools {
        extract: Arc::new(unzip),
        verify: Arc::new(|_| Ok(())),
        swap: Arc::new(swap_dirs),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_newer_release_replaces_the_whole_app_bundle() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let zip = app_zip(NEW_BINARY, true);
    let (bundle, updater) =
        bundle_setup(signed_for(Target::MacosAarch64, NEWER, &zip), fake_tools()).await;
    // A staging folder from an install that crashed doesn't get in the way.
    let stale = bundle
        .applications
        .join(".sonora-update.crashed/unpacked/Sonora.app");
    std::fs::create_dir_all(&stale).unwrap();
    let mut worker = fixture.start_with_updater(Ok(updater));
    worker.wait_ready(WAIT).unwrap();

    worker.service().send(Command::CheckForUpdates);
    let status = update_status(&worker, finished);
    assert_eq!(status.state, UpdateState::Ready(NEWER.into()));
    assert_eq!(bundle.executable(), NEW_BINARY);
    assert!(bundle.app.join("Contents/Info.plist").exists());
    assert!(
        !bundle.app.join("Contents/old-only").exists(),
        "the whole bundle is replaced"
    );
    assert_eq!(bundle.leftovers(), Vec::<String>::new());

    // Restarting opens the installed bundle, not the executable inside it.
    worker.service().send(Command::RestartToUpdate);
    super::fixture::event(&worker, |event| matches!(event, Event::Relaunching));
    assert_eq!(
        *bundle.relaunched.lock().unwrap(),
        std::slice::from_ref(&bundle.app)
    );
    assert_eq!(*bundle.api.binary_downloads.lock().unwrap(), 1);

    worker.shutdown(WAIT).unwrap();
    bundle.task.abort();
    fixture.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_bundle_update_leaves_the_installed_app_alone() {
    let zip = app_zip(NEW_BINARY, true);
    let mut tampered = signed_for(Target::MacosAarch64, NEWER, &zip);
    tampered.binary = app_zip(b"something else", true);
    let unverified = BundleTools {
        verify: Arc::new(|_| anyhow::bail!("Its code signature is invalid")),
        ..fake_tools()
    };
    let unswappable = BundleTools {
        swap: Arc::new(|_, _| anyhow::bail!("The volume is full")),
        ..fake_tools()
    };
    let no_executable = app_zip(NEW_BINARY, false);
    let cases = [
        (
            "checksum",
            tampered,
            fake_tools(),
            "doesn't match its checksum",
        ),
        (
            "verification",
            signed_for(Target::MacosAarch64, NEWER, &zip),
            unverified,
            "failed verification",
        ),
        (
            "swap",
            signed_for(Target::MacosAarch64, NEWER, &zip),
            unswappable,
            "Couldn't replace the installed app",
        ),
        (
            "executable",
            signed_for(Target::MacosAarch64, NEWER, &no_executable),
            fake_tools(),
            "has no Sonora executable",
        ),
        (
            "platform",
            signed_for(Target::LinuxX86_64, NEWER, &zip),
            fake_tools(),
            "for another platform",
        ),
    ];
    for (case, mut release, tools, error) in cases {
        // A Linux manifest is served under the macOS asset names.
        release.target = Target::MacosAarch64;
        let fixture = Fixture::new(Library::default());
        fixture.seed();
        let (bundle, updater) = bundle_setup(release, tools).await;
        let mut worker = fixture.start_with_updater(Ok(updater));
        worker.wait_ready(WAIT).unwrap();
        worker.service().send(Command::CheckForUpdates);
        let state = update_status(&worker, finished).state;
        assert!(
            matches!(&state, UpdateState::Failed(message) if message.contains(error)),
            "{case}: {state:?}"
        );
        assert_eq!(bundle.executable(), b"old sonora build", "{case}");
        assert!(bundle.app.join("Contents/old-only").exists(), "{case}");
        assert_eq!(bundle.leftovers(), Vec::<String>::new(), "{case}");
        worker.service().send(Command::RestartToUpdate);
        worker.service().send(Command::CheckForUpdates);
        update_status(&worker, finished);
        assert!(bundle.relaunched.lock().unwrap().is_empty(), "{case}");
        worker.shutdown(WAIT).unwrap();
        bundle.task.abort();
        fixture.finish();
    }
}

/// `ditto`, `codesign`, `plutil` and the atomic swap on real ad-hoc signed
/// bundles.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_macos_tools_install_a_signed_bundle_and_refuse_a_broken_one() {
    fn run(program: &str, args: &[&std::ffi::OsStr]) {
        let status = std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{program} {args:?}");
    }
    /// An ad-hoc signed `Sonora.app` in `dir` whose resources hold `version`.
    fn signed_app(dir: &Path, version: &str, identifier: &str) -> PathBuf {
        let app = dir.join("Sonora.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        std::fs::copy("/usr/bin/true", app.join("Contents/MacOS/sonora")).unwrap();
        std::fs::write(app.join("Contents/Resources/version"), version).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{identifier}</string>
<key>CFBundleExecutable</key><string>sonora</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
"#
            ),
        )
        .unwrap();
        run(
            "/usr/bin/codesign",
            &[
                "--force".as_ref(),
                "--sign".as_ref(),
                "-".as_ref(),
                app.as_os_str(),
            ],
        );
        app
    }
    fn zipped(app: &Path) -> Vec<u8> {
        let zip = app.with_extension("zip");
        run(
            "/usr/bin/ditto",
            &[
                "-c".as_ref(),
                "-k".as_ref(),
                "--keepParent".as_ref(),
                app.as_os_str(),
                zip.as_os_str(),
            ],
        );
        std::fs::read(zip).unwrap()
    }

    let builds = tempfile::tempdir().unwrap();
    let good = zipped(&signed_app(
        &builds.path().join("good"),
        "new",
        "app.sonora.player",
    ));
    let broken_app = signed_app(&builds.path().join("broken"), "new", "app.sonora.player");
    // Changing a sealed resource after signing breaks the signature.
    std::fs::write(broken_app.join("Contents/Resources/version"), "evil").unwrap();
    let broken = zipped(&broken_app);
    let other = zipped(&signed_app(
        &builds.path().join("other"),
        "new",
        "com.example.other",
    ));

    let cases = [
        ("broken signature", broken, false),
        ("another app", other, false),
        ("signed Sonora", good, true),
    ];
    for (case, zip, installs) in cases {
        let installed = tempfile::tempdir().unwrap();
        let app = signed_app(installed.path(), "old", "app.sonora.player");
        let (base, _api, task) = serve(signed_for(Target::MacosAarch64, NEWER, &zip)).await;
        let updater = Updater::bundle(
            &base,
            CURRENT,
            app.clone(),
            key().verifying_key(),
            BundleTools::macos(),
        )
        .unwrap();
        let fixture = Fixture::new(Library::default());
        fixture.seed();
        let mut worker = fixture.start_with_updater(Ok(updater));
        worker.wait_ready(WAIT).unwrap();
        worker.service().send(Command::CheckForUpdates);
        let state = update_status(&worker, finished).state;
        let version = std::fs::read_to_string(app.join("Contents/Resources/version")).unwrap();
        if installs {
            assert_eq!(state, UpdateState::Ready(NEWER.into()), "{case}");
            assert_eq!(version, "new", "{case}");
        } else {
            assert!(
                matches!(&state, UpdateState::Failed(message) if message.contains("failed verification")),
                "{case}: {state:?}"
            );
            assert_eq!(version, "old", "{case}");
        }
        run(
            "/usr/bin/codesign",
            &[
                "--verify".as_ref(),
                "--deep".as_ref(),
                "--strict".as_ref(),
                app.as_os_str(),
            ],
        );
        let entries: Vec<_> = std::fs::read_dir(installed.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["Sonora.app"], "{case}");
        worker.shutdown(WAIT).unwrap();
        task.abort();
        fixture.finish();
    }
}
