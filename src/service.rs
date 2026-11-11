use crate::{
    artists::{ArtistPageData, ArtistProfile, TopSongs, cache_key},
    artwork::{ArtworkQueue, Priority},
    backdrop::{self, Backdrop},
    catalog::{Catalog, LibraryView},
    discography::{self, Discography, DiscographyView, ReleaseDetails},
    listening::ListenReport,
    lyrics::Lyrics,
    model::*,
    musicbrainz::MusicBrainz,
    platform::{Account, Paths},
    playback::{Placement, PlayerCommand, PlayerEvent, PlayerHandle},
    server::Server,
    slskd::{Candidate, DownloadJob, JobState, Slskd, SlskdAuth},
    sorting::{Field as SortField, Scope as SortScope},
    thumbnail,
};
use anyhow::Result;
use flume::{Receiver, Sender};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

const SIGN_IN_NOTICE: &str =
    "Sign in once to save your login locally. Sonora no longer uses Keychain.";

#[derive(Clone, Debug, PartialEq)]
pub enum Mutation {
    Favorite(String, bool),
    Rating(String, u8),
    AlbumFavorite(String, bool),
    AlbumRating(String, u8),
    CreatePlaylist(String, Vec<String>),
    ReplacePlaylist(String, Vec<String>),
    RenamePlaylist(String, String),
    DeletePlaylist(String),
}
impl Mutation {
    pub fn album_feedback_id(&self) -> Option<&str> {
        match self {
            Self::AlbumFavorite(id, _) | Self::AlbumRating(id, _) => Some(id),
            _ => None,
        }
    }
}
#[derive(Clone)]
pub enum Command {
    Connect(Account, String),
    Refresh,
    Initialize,
    Query(u64, LibraryQuery),
    Page(u64, u64, LibraryQuery, usize),
    Album(String),
    Artist(u64, String, String),
    ArtistProfile(String, String, bool),
    ArtistProfileLoaded(String, String, bool, Result<ArtistProfile, String>),
    TopSongs(String, String),
    TopSongsLoaded(String, String, Result<TopSongs, String>),
    /// Artist name and ID: list the releases the library lacks.
    Discography(String, String),
    DiscographyLoaded(String, String, String, String, Result<Discography, String>),
    /// Release group: its track list and listening links.
    ReleaseDetails(String),
    ReleaseDetailsLoaded(String, Result<ReleaseDetails, String>),
    /// Artist name, artist ID, release group: stop offering this release.
    HideRelease(String, String, String),
    /// API key, username, password as typed; all empty keeps what is saved.
    SlskdAuth(String, String, String),
    SlskdTest,
    /// Release group and search text.
    SoulseekSearch(String, String),
    Download(Box<DownloadRequest>),
    DownloadUpdate(String, DownloadJob),
    DismissDownload(String),
    ResumeDownloads,
    ScrobbleFinished(String, u64, Result<Option<Arc<Track>>, String>),
    GroupTracks(u64, LibraryQuery, String),
    PlayTrack(String),
    Playlist(String),
    /// Finds a cover in the disk cache; with a priority, downloads it when missing.
    Artwork(String, Option<Priority>),
    ArtworkFinished(String, String, Result<PathBuf, String>),
    Backdrop(String),
    BackdropFinished(String, String, Result<Backdrop, String>),
    Lyrics(Arc<Track>),
    LyricsLoaded(String, String, Result<Lyrics, String>),
    PlayIds(Vec<String>, usize),
    PlayAlbum(String),
    Enqueue(Vec<String>, Placement),
    EnqueueAlbum(String, Placement),
    Mutate(Mutation),
    Preferences(Preferences),
    Player(PlayerCommand),
    Persist(SavedQueue),
    Scrobble(ListenReport),
    Connected(Server, String),
    Tracks(i64, Vec<Track>),
    Albums(i64, Vec<Album>),
    Synced(i64, Vec<Playlist>, bool),
    Updated(String, Mutation),
    MutationFailed(String, Mutation, String),
    PlaylistLoaded(Playlist),
    PlaylistsLoaded(Vec<Playlist>),
    Error(String),
    SyncFailed(i64, String),
    Shutdown(Sender<()>),
}
#[derive(Clone)]
pub struct DownloadRequest {
    pub artist: String,
    pub artist_id: String,
    pub release: String,
    pub title: String,
    pub expected: Option<u32>,
    pub candidate: Candidate,
}
#[derive(Clone, Debug)]
pub enum SearchUpdate {
    /// Responses received so far.
    Searching(u64),
    /// Folders on offer, best first, and the release's usual track count.
    Results(Arc<Vec<Candidate>>, Option<u32>),
    Failed(String),
}
#[derive(Clone, Debug)]
pub enum Event {
    Ready(Option<Account>, Preferences),
    SignInRequired(String),
    Connected(Account, String),
    View(u64, LibraryView),
    Page(u64, u64, usize, Vec<Arc<Track>>),
    Album(String, Vec<Arc<Track>>),
    Artist(u64, ArtistPageData),
    ArtistProfile(String, Arc<ArtistProfile>),
    TopSongs(String, Arc<TopSongs>),
    Discography(String, Arc<DiscographyView>),
    SoulseekSearch(String, SearchUpdate),
    ReleaseDetails(String, Result<Arc<ReleaseDetails>, String>),
    Downloads(Arc<Vec<DownloadJob>>),
    SlskdStatus(Result<String, String>),
    GroupTracks(u64, String, Vec<Arc<Track>>),
    Playlists(Vec<Playlist>),
    Playlist(Playlist),
    /// A cover on disk, and whether its small copies exist beside it.
    Artwork(String, PathBuf, bool),
    ArtworkFailed(String),
    ArtworkRetrying,
    Backdrop(String, Arc<Backdrop>),
    Lyrics(String, Result<Arc<Lyrics>, String>),
    Playback(PlaybackSnapshot),
    Status(String),
    Error(String),
    MutationDone(Mutation),
    MutationFailed(Mutation, String),
    AlbumUpdated(Arc<Album>),
    TrackUpdated(Arc<Track>),
}
#[derive(Clone)]
pub struct Service {
    pub tx: Sender<Command>,
    pub player: PlayerHandle,
    query_epoch: Arc<AtomicU64>,
}
impl Service {
    pub fn start(paths: Paths, runtime: tokio::runtime::Handle) -> (Self, Receiver<Event>) {
        let (tx, rx) = flume::unbounded();
        let (events, event_rx) = flume::unbounded();
        let (player, player_events) = PlayerHandle::start(runtime.clone());
        let query_epoch = Arc::new(AtomicU64::new(0));
        let service = Self {
            tx: tx.clone(),
            player: player.clone(),
            query_epoch: query_epoch.clone(),
        };
        let event_tx = events.clone();
        let command_tx = tx.clone();
        std::thread::Builder::new()
            .name("sonora-player-events".into())
            .spawn(move || {
                while let Ok(event) = player_events.recv() {
                    match event {
                        PlayerEvent::Snapshot(snapshot) => {
                            let _ = event_tx.send(Event::Playback(snapshot));
                        }
                        PlayerEvent::Persist(queue) => {
                            let _ = command_tx.send(Command::Persist(queue));
                        }
                        PlayerEvent::Scrobble(report) => {
                            let _ = command_tx.send(Command::Scrobble(report));
                        }
                        PlayerEvent::Error(error) => {
                            let _ = event_tx.send(Event::Error(error));
                        }
                    }
                }
            })
            .expect("Could not start playback bridge");
        let worker = service.clone();
        std::thread::Builder::new()
            .name("sonora-catalog".into())
            .spawn(move || {
                if let Err(error) = run(paths, runtime, worker, rx, events.clone()) {
                    let _ = events.send(Event::Error(format!("{error:#}")));
                }
            })
            .expect("Could not start catalog thread");
        (service, event_rx)
    }
    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }
    pub fn query(&self, query: LibraryQuery) -> u64 {
        let epoch = self.query_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.send(Command::Query(epoch, query));
        epoch
    }
}
fn catalog_path(paths: &Paths, account: Option<&Account>) -> PathBuf {
    paths.data.join(format!(
        "{}.sqlite",
        account.map_or("unconnected".into(), Account::namespace)
    ))
}
const DOWNLOADS: &str = "downloads:v1";

fn discography_view(
    catalog: &Catalog,
    name: &str,
    id: &str,
    loading: bool,
) -> Result<DiscographyView> {
    let key = cache_key(name, id);
    let cached: Option<Discography> = catalog.state(&format!("discography:v1:{key}"))?;
    let Some(cached) = cached else {
        return Ok(DiscographyView {
            loading,
            missing: Vec::new(),
        });
    };
    let albums = catalog.artist_releases(name, id)?;
    let hidden: Vec<String> = catalog.state(&format!("discography-hidden:{key}"))?;
    let library = discography::Library {
        albums: &albums,
        release_ids: catalog.release_ids()?,
        track_titles: catalog
            .artist_tracks(name, id)?
            .iter()
            .map(|track| discography::fold(&track.title))
            .collect(),
        hidden: hidden.into_iter().collect(),
        downloading: catalog
            .state::<Vec<DownloadJob>>(DOWNLOADS)?
            .into_iter()
            .filter(|job| job.artist == key && job.state != JobState::Downloaded)
            .map(|job| job.release)
            .collect(),
    };
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    Ok(DiscographyView {
        loading: false,
        missing: discography::missing(&cached, &library, &today),
    })
}

/// Shows the artist's missing releases and forgets downloads the library now holds.
fn publish_discography(
    catalog: &Catalog,
    events: &Sender<Event>,
    name: &str,
    id: &str,
    loading: bool,
) -> Result<()> {
    let key = cache_key(name, id);
    let view = discography_view(catalog, name, id, loading)?;
    let mut jobs: Vec<DownloadJob> = catalog.state(DOWNLOADS)?;
    let before = jobs.len();
    jobs.retain(|job| {
        job.artist != key
            || job.state != JobState::Downloaded
            || view.loading
            || view.missing.iter().any(|release| release.id == job.release)
    });
    if jobs.len() != before {
        catalog.set_state(DOWNLOADS, &jobs)?;
        events.send(Event::Downloads(Arc::new(jobs)))?;
    }
    events.send(Event::Discography(key, Arc::new(view)))?;
    Ok(())
}

fn slskd_client(
    cache: &mut Option<(String, Slskd)>,
    catalog: &Catalog,
    paths: &Paths,
) -> Result<Slskd> {
    let downloads = catalog.state::<Preferences>("preferences")?.downloads;
    anyhow::ensure!(
        downloads.enabled && !downloads.url.trim().is_empty(),
        "Link slskd in Settings → Downloads first"
    );
    if let Some((url, client)) = cache
        && *url == downloads.url
    {
        return Ok(client.clone());
    }
    let client = Slskd::new(&downloads.url, paths.slskd_auth()?)?;
    *cache = Some((downloads.url, client.clone()));
    Ok(client)
}

/// Follows one queued folder until every file has finished or failed.
fn watch_download(
    runtime: &tokio::runtime::Handle,
    client: Slskd,
    tx: Sender<Command>,
    namespace: String,
    mut job: DownloadJob,
) {
    runtime.spawn(async move {
        // A request the peer dropped is asked for again after this long without change.
        const STALLED: std::time::Duration = std::time::Duration::from_secs(15 * 60);
        const MAX_REQUEUES: u32 = 3;
        let mut unreachable = 0;
        let mut changed = std::time::Instant::now();
        // Peers can keep a folder queued for a long time; give up after a day of polling.
        for _ in 0..17_280 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let Ok(downloads) = client.downloads(&job.username).await else {
                unreachable += 1;
                if unreachable > 60 {
                    break;
                }
                continue;
            };
            unreachable = 0;
            let progress = crate::slskd::progress(&downloads, &job.files);
            let finished = progress.finished();
            if finished {
                job.state = if progress.succeeded() {
                    JobState::Downloaded
                } else {
                    JobState::Failed
                };
            }
            if progress != job.progress {
                changed = std::time::Instant::now();
            } else if !finished
                && progress.active == 0
                && changed.elapsed() >= STALLED
                && job.requeued < MAX_REQUEUES
            {
                changed = std::time::Instant::now();
                let lost = crate::slskd::waiting(&downloads, &job.files);
                if !lost.is_empty()
                    && matches!(client.peer_idle(&job.username).await, Ok(true))
                    && client.requeue(&job.username, &lost).await.is_ok()
                {
                    job.requeued += 1;
                    if tx
                        .send(Command::DownloadUpdate(namespace.clone(), job.clone()))
                        .is_err()
                    {
                        return;
                    }
                }
            }
            if finished || progress != job.progress {
                job.progress = progress;
                if tx
                    .send(Command::DownloadUpdate(namespace.clone(), job.clone()))
                    .is_err()
                {
                    return;
                }
            }
            if finished {
                return;
            }
        }
        job.state = JobState::Failed;
        job.progress.failure = "slskd stopped reporting this download".into();
        let _ = tx.send(Command::DownloadUpdate(namespace, job));
    });
}

fn begin_sync(
    server: Server,
    tx: Sender<Command>,
    events: Sender<Event>,
    runtime: &tokio::runtime::Handle,
    generation: i64,
) {
    runtime.spawn(async move {
        let result:Result<(Vec<Playlist>,bool)>=async {
            let scanning_before=server.scan_status().await?;
            for kind in ["song","album"] {
                let mut offset=0;let mut seen=HashSet::new();
                loop {
                    let page=server.page(kind,offset).await?;
                    let count=page.as_array().map_or(0,Vec::len);
                    if count==0{break;}
                    for item in page.as_array().unwrap(){
                        let id=item["id"].as_str().unwrap_or_default();
                        if id.is_empty()||!seen.insert(id.to_string()){anyhow::bail!("Library changed during sync. Refresh again after the server scan finishes.");}
                    }
                    if kind=="song"{tx.send_async(Command::Tracks(generation,serde_json::from_value(page)?)).await?;}
                    else{tx.send_async(Command::Albums(generation,serde_json::from_value(page)?)).await?;}
                    offset+=count;
                    let _=events.send(Event::Status(format!("Syncing {} · {offset:} indexed",if kind=="song"{"tracks"}else{"albums"})));
                }
            }
            let playlists=server.playlists().await?;
            let scanning_after=server.scan_status().await?;
            Ok((playlists,!scanning_before&&!scanning_after))
        }.await;
        match result{Ok((playlists,can_delete))=>{let _=tx.send_async(Command::Synced(generation,playlists,can_delete)).await;}
            Err(error)=>{let _=tx.send_async(Command::SyncFailed(generation,format!("Refresh incomplete: {error:#}. Cached music is still available."))).await;}}
    });
}
/// Sends a cover once its small copies exist, making them off this thread when missing.
fn publish_artwork(
    runtime: &tokio::runtime::Handle,
    events: &Sender<Event>,
    id: String,
    path: PathBuf,
) {
    if thumbnail::ready(&path) {
        let _ = events.send(Event::Artwork(id, path, true));
        return;
    }
    let events = events.clone();
    runtime.spawn_blocking(move || {
        let thumbs = thumbnail::ensure(&path).is_ok();
        let _ = events.send(Event::Artwork(id, path, thumbs));
    });
}

fn run(
    paths: Paths,
    runtime: tokio::runtime::Handle,
    service: Service,
    rx: Receiver<Command>,
    events: Sender<Event>,
) -> Result<()> {
    let Service {
        player,
        tx,
        query_epoch,
        ..
    } = service;
    let mut account = paths.account()?;
    let mut catalog = Catalog::open(&catalog_path(&paths, account.as_ref()))?;
    let prefs: Preferences = catalog.state("preferences")?;
    player.send(PlayerCommand::Preferences(prefs.clone()));
    let queue: SavedQueue = catalog.state("queue")?;
    player.send(PlayerCommand::Restore(
        catalog.by_ids(&queue.ids)?,
        queue.index.unwrap_or(0),
        queue.position,
        queue.listen,
    ));
    events.send(Event::Ready(account.clone(), prefs))?;
    events.send(Event::Playlists(catalog.playlist_summaries()?))?;
    events.send(Event::Status(
        if account.is_some() {
            "Cached library · connecting…"
        } else {
            "Connect your Navidrome library in Settings"
        }
        .into(),
    ))?;
    if let Some(account) = account.clone() {
        let command_tx = tx.clone();
        let saved = paths.credentials(&account);
        let event_tx = events.clone();
        runtime.spawn(async move {
            let credentials = match saved {
                Ok(Some(credentials)) => credentials,
                result => {
                    let message = match result {
                        Err(error) => format!("{error:#}"),
                        _ => SIGN_IN_NOTICE.into(),
                    };
                    let _ = event_tx.send(Event::SignInRequired(message));
                    return;
                }
            };
            let result = Server::with_credentials(account, credentials);
            match result {
                Ok(mut server) => match server.connect().await {
                    Ok(label) => {
                        let _ = command_tx.send(Command::Connected(server, label));
                    }
                    Err(error) => {
                        let _ = command_tx.send(Command::Error(format!("Offline · {error:#}")));
                    }
                },
                Err(error) => {
                    let _ = command_tx.send(Command::Error(format!("Offline · {error:#}")));
                }
            }
        });
    }
    let mut server: Option<Server> = None;
    let mut syncing = false;
    let mut generation = 0i64;
    let mut current_query = LibraryQuery::default();
    let mut current_request = 0;
    let mut failed_generations = HashSet::new();
    let mut artwork_queue = ArtworkQueue::default();
    let mut profile_requests = HashSet::new();
    let mut profile_pending: VecDeque<(String, String, bool)> = VecDeque::new();
    let mut top_song_requests = HashSet::new();
    let mut submission_requests = HashSet::new();
    let musicbrainz = MusicBrainz::new()?;
    let mut discography_requests = HashSet::new();
    let mut release_requests: HashSet<String> = HashSet::new();
    let mut open_artist: Option<(u64, String, String)> = None;
    let mut slskd: Option<(String, Slskd)> = None;
    let mut watched: HashSet<String> = HashSet::new();
    let _ = tx.send(Command::ResumeDownloads);
    let mut published = 0;
    loop {
        if let Some(server) = server.clone() {
            for id in artwork_queue.take_ready(Instant::now()) {
                let server = server.clone();
                let cache = paths.cache.clone();
                let tx = tx.clone();
                let namespace = server.account.namespace();
                runtime.spawn(async move {
                    let result = server
                        .artwork(&id, &cache)
                        .await
                        .map_err(|error| format!("{error:#}"));
                    let _ = tx.send(Command::ArtworkFinished(namespace, id, result));
                });
            }
        }
        if let Some(server) = server.clone() {
            while profile_requests.len() < 4 {
                let Some((name, id, full)) = profile_pending.pop_front() else {
                    break;
                };
                let key = cache_key(&name, &id);
                let namespace = server.account.namespace();
                if !profile_requests.insert((namespace.clone(), key.clone(), full)) {
                    continue;
                }
                let tx = tx.clone();
                let server = server.clone();
                runtime.spawn(async move {
                    let result = server.artist_profile(&name, &id, full).await.map_err(|_| {
                        "Artist information is unavailable from Navidrome.".to_string()
                    });
                    let _ = tx.send(Command::ArtistProfileLoaded(namespace, key, full, result));
                });
            }
        }
        if let Some(server) = server.clone() {
            for report in catalog
                .pending_scrobbles(&server.account.namespace())?
                .into_iter()
                .take(4_usize.saturating_sub(submission_requests.len()))
            {
                if catalog.attempt_scrobble(report.id)? {
                    submission_requests.insert((report.account.clone(), report.id));
                    let tx = tx.clone();
                    let server = server.clone();
                    runtime.spawn(async move {
                        let result = match server.scrobble(&report.track_id, true, report.started_ms).await {
                            Ok(()) => Ok(server.song(&report.track_id).await.ok().map(Arc::new)),
                            Err(_) => Err("Play confirmation was lost or rejected. It will not be sent again automatically to avoid duplicates.".into()),
                        };
                        let _ = tx.send(Command::ScrobbleFinished(report.account, report.id, result));
                    });
                }
            }
        }
        // Completed requests wake this actor; only artwork backoff needs a timer.
        let retry = server
            .as_ref()
            .and_then(|_| artwork_queue.next_retry(Instant::now()));
        let received = match retry {
            Some(wait) => rx.recv_timeout(wait),
            None => rx.recv().map_err(|_| flume::RecvTimeoutError::Disconnected),
        };
        let command = match received {
            Ok(c) => c,
            Err(flume::RecvTimeoutError::Timeout) => continue,
            Err(flume::RecvTimeoutError::Disconnected) => break,
        };
        if let Command::Shutdown(ack) = command {
            let (reply, queue) = flume::bounded(1);
            player.send(PlayerCommand::SaveQueue(reply));
            if let Ok(queue) = queue.recv_timeout(std::time::Duration::from_secs(2)) {
                catalog.set_state("queue", &queue)?;
            }
            player.send(PlayerCommand::Shutdown);
            let _ = ack.send(());
            break;
        }
        let result: Result<()> = (|| {
            match command {
                Command::Initialize => {
                    events.send(Event::Ready(account.clone(), catalog.state("preferences")?))?;
                    events.send(Event::Playlists(catalog.playlist_summaries()?))?;
                    if let Some(account) = &account {
                        match paths.credentials(account) {
                            Ok(Some(_)) => {}
                            Ok(None) => {
                                events.send(Event::SignInRequired(SIGN_IN_NOTICE.into()))?
                            }
                            Err(error) => {
                                events.send(Event::SignInRequired(format!("{error:#}")))?
                            }
                        }
                    }
                }
                Command::Connect(account, password) => {
                    let command_tx = tx.clone();
                    let paths = paths.clone();
                    runtime.spawn(async move {
                        let result = async {
                            let mut server = Server::sign_in(&paths, account, &password)?;
                            let label = server.connect().await?;
                            if !password.is_empty() {
                                paths.save_credentials(&server.account, server.credentials())?;
                            }
                            Ok::<_, anyhow::Error>((server, label))
                        }
                        .await;
                        match result {
                            Ok((server, label)) => {
                                let _ = command_tx.send(Command::Connected(server, label));
                            }
                            Err(error) => {
                                let _ = command_tx
                                    .send(Command::Error(format!("Could not connect: {error:#}")));
                            }
                        }
                    });
                }
                Command::Connected(value, label) => {
                    if account.as_ref().map(Account::namespace) != Some(value.account.namespace()) {
                        if syncing {
                            anyhow::bail!(
                                "Wait for the current refresh to finish before changing accounts"
                            );
                        }
                        artwork_queue = ArtworkQueue::default();
                        profile_pending.clear();
                        catalog = Catalog::open(&catalog_path(&paths, Some(&value.account)))?;
                        let prefs: Preferences = catalog.state("preferences")?;
                        player.send(PlayerCommand::Preferences(prefs.clone()));
                        let queue: SavedQueue = catalog.state("queue")?;
                        player.send(PlayerCommand::Restore(
                            catalog.by_ids(&queue.ids)?,
                            queue.index.unwrap_or(0),
                            queue.position,
                            queue.listen,
                        ));
                        events.send(Event::Ready(Some(value.account.clone()), prefs))?;
                        slskd = None;
                        watched.clear();
                        open_artist = None;
                        let _ = tx.send(Command::ResumeDownloads);
                    }
                    paths.save_account(&value.account)?;
                    account = Some(value.account.clone());
                    player.send(PlayerCommand::Server(value.clone()));
                    server = Some(value.clone());
                    artwork_queue.retry_failed(Instant::now());
                    events.send(Event::Connected(value.account.clone(), label))?;
                    events.send(Event::View(current_request, catalog.view(&current_query)?))?;
                    if !syncing {
                        syncing = true;
                        generation = chrono::Utc::now().timestamp_micros();
                        begin_sync(value, tx.clone(), events.clone(), &runtime, generation);
                    }
                }
                Command::Refresh => {
                    catalog.invalidate_artist_profiles()?;
                    artwork_queue.retry_failed(Instant::now());
                    events.send(Event::ArtworkRetrying)?;
                    if let Some(server) = server.clone() {
                        if !syncing {
                            syncing = true;
                            generation = chrono::Utc::now().timestamp_micros();
                            begin_sync(server, tx.clone(), events.clone(), &runtime, generation);
                        }
                    } else {
                        anyhow::bail!("Open Settings to connect to your server");
                    }
                }
                Command::Tracks(batch_generation, tracks) => {
                    if generation == batch_generation && !failed_generations.contains(&generation) {
                        if let Err(error) = catalog.put_tracks(&tracks, generation) {
                            failed_generations.insert(generation);
                            syncing = false;
                            return Err(error);
                        }
                        published += tracks.len();
                        if published >= 5000 {
                            published = 0;
                            events.send(Event::View(
                                current_request,
                                catalog.view(&current_query)?,
                            ))?;
                        }
                    }
                }
                Command::Albums(batch_generation, albums) => {
                    if generation == batch_generation
                        && !failed_generations.contains(&generation)
                        && let Err(error) = catalog.put_albums(&albums, generation)
                    {
                        failed_generations.insert(generation);
                        syncing = false;
                        return Err(error);
                    }
                }
                Command::Synced(batch_generation, playlists, can_delete) => {
                    if generation == batch_generation && !failed_generations.contains(&generation) {
                        if can_delete {
                            catalog.finish_sync(generation)?;
                        }
                        catalog.save_playlist_summaries(&playlists, can_delete)?;
                        syncing = false;
                        events.send(Event::Playlists(catalog.playlist_summaries()?))?;
                        events.send(Event::View(current_request, catalog.view(&current_query)?))?;
                        events.send(Event::Status(if can_delete {
                            format!("Up to date · {} tracks", catalog.count()?)
                        } else {
                            "Server scan in progress · refresh again when complete".into()
                        }))?;
                        // A refresh can turn a missing release into an owned one.
                        let mut finished = catalog
                            .state::<Vec<DownloadJob>>(DOWNLOADS)?
                            .into_iter()
                            .filter(|job| job.state == JobState::Downloaded)
                            .map(|job| job.artist)
                            .collect::<Vec<_>>();
                        finished.dedup();
                        for key in finished {
                            // The key is the artist's normalised name and ID, which is all a lookup needs.
                            if let Some((name, id)) = key.split_once('\n') {
                                publish_discography(&catalog, &events, name, id, false)?;
                            }
                        }
                        if let Some((request, name, id)) = &open_artist {
                            events.send(Event::Artist(*request, catalog.artist_page(name, id)?))?;
                            publish_discography(&catalog, &events, name, id, false)?;
                        }
                    }
                }
                Command::Query(request, query) => {
                    if request == query_epoch.load(Ordering::Acquire) {
                        current_query = query;
                        current_request = request;
                        events.send(Event::View(request, catalog.view(&current_query)?))?;
                    }
                }
                Command::Page(request, revision, query, offset) => {
                    if request == query_epoch.load(Ordering::Acquire) {
                        if revision != catalog.revision() {
                            // A sync or edit changed the ordering since the first page.
                            events.send(Event::View(request, catalog.view(&query)?))?;
                        } else {
                            events.send(Event::Page(
                                request,
                                revision,
                                offset,
                                catalog.tracks(&query, offset, 500)?,
                            ))?;
                        }
                    }
                }
                Command::GroupTracks(request, query, id) => {
                    if request == query_epoch.load(Ordering::Acquire) {
                        events.send(Event::GroupTracks(
                            request,
                            id.clone(),
                            catalog.album_tracks_matching(&query, &id)?,
                        ))?;
                    }
                }
                Command::PlayTrack(id) => {
                    player.send(PlayerCommand::Replace(catalog.album_queue_from(&id)?, 0));
                }
                Command::Album(id) => {
                    events.send(Event::Album(id.clone(), catalog.album_tracks(&id)?))?;
                    if let Some(server) = server.clone() {
                        let tx = tx.clone();
                        let events = events.clone();
                        runtime.spawn(async move {
                            if let Ok((album, tracks)) = server.album(&id).await {
                                let _ = tx.send(Command::Albums(generation, vec![album]));
                                let _ = events.send(Event::Album(
                                    id,
                                    tracks.into_iter().map(Arc::new).collect(),
                                ));
                            }
                        });
                    }
                }
                Command::Artist(request, name, id) => {
                    events.send(Event::Artist(request, catalog.artist_page(&name, &id)?))?;
                    open_artist = Some((request, name.clone(), id.clone()));
                    let _ = tx.send(Command::Discography(name.clone(), id.clone()));
                    let _ = tx.send(Command::ArtistProfile(name, id, true));
                }
                Command::Discography(name, id) => {
                    let key = cache_key(&name, &id);
                    let cached: Option<Discography> =
                        catalog.state(&format!("discography:v1:{key}"))?;
                    let now = chrono::Utc::now().timestamp_millis();
                    let fetch = !cached.as_ref().is_some_and(|known| known.fresh(now))
                        && !discography::is_various_artists(&name, "");
                    let server = server.clone().filter(|_| fetch);
                    publish_discography(
                        &catalog,
                        &events,
                        &name,
                        &id,
                        cached.is_none() && server.is_some(),
                    )?;
                    if let Some(server) = server {
                        let namespace = server.account.namespace();
                        if discography_requests.insert((namespace.clone(), key.clone())) {
                            let titles: HashSet<String> = catalog
                                .artist_releases(&name, &id)?
                                .iter()
                                .map(|album| discography::fold(&album.name))
                                .collect();
                            let tx = tx.clone();
                            let musicbrainz = musicbrainz.clone();
                            runtime.spawn(async move {
                                let result = async {
                                    let unknown = || Discography {
                                        fetched_ms: chrono::Utc::now().timestamp_millis(),
                                        ..Default::default()
                                    };
                                    let mut artist = server.artist_mbid(&id).await?;
                                    if discography::is_various_artists(&name, &artist) {
                                        return Ok(unknown());
                                    }
                                    if artist.is_empty() {
                                        artist = musicbrainz
                                            .resolve_artist(&name, &titles)
                                            .await?
                                            .unwrap_or_default();
                                    }
                                    if artist.is_empty() {
                                        return Ok(unknown());
                                    }
                                    musicbrainz.discography(&artist).await
                                }
                                .await
                                .map_err(|error: anyhow::Error| format!("{error:#}"));
                                let _ = tx.send(Command::DiscographyLoaded(
                                    namespace, key, name, id, result,
                                ));
                            });
                        }
                    }
                }
                Command::DiscographyLoaded(namespace, key, name, id, result) => {
                    discography_requests.remove(&(namespace.clone(), key.clone()));
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        // A failed lookup keeps whatever was cached; the section just stays as it was.
                        if let Ok(found) = result {
                            catalog.set_state(&format!("discography:v1:{key}"), &Some(&found))?;
                        }
                        publish_discography(&catalog, &events, &name, &id, false)?;
                    }
                }
                Command::ReleaseDetails(group) => {
                    let cached: Option<ReleaseDetails> =
                        catalog.state(&format!("release-details:v1:{group}"))?;
                    let now = chrono::Utc::now().timestamp_millis();
                    if let Some(details) = cached.clone() {
                        events.send(Event::ReleaseDetails(group.clone(), Ok(Arc::new(details))))?;
                    }
                    if !cached.is_some_and(|known| known.fresh(now))
                        && release_requests.insert(group.clone())
                    {
                        let tx = tx.clone();
                        let musicbrainz = musicbrainz.clone();
                        runtime.spawn(async move {
                            let result = musicbrainz
                                .release_details(&group)
                                .await
                                .map_err(|error| format!("{error:#}"));
                            let _ = tx.send(Command::ReleaseDetailsLoaded(group, result));
                        });
                    }
                }
                Command::ReleaseDetailsLoaded(group, result) => {
                    release_requests.remove(&group);
                    let key = format!("release-details:v1:{group}");
                    match result {
                        Ok(details) => {
                            catalog.set_state(&key, &Some(&details))?;
                            events.send(Event::ReleaseDetails(group, Ok(Arc::new(details))))?;
                        }
                        // A failed lookup keeps what was cached and only reports when nothing was.
                        Err(error) => {
                            if catalog.state::<Option<ReleaseDetails>>(&key)?.is_none() {
                                events.send(Event::ReleaseDetails(group, Err(error)))?;
                            }
                        }
                    }
                }
                Command::HideRelease(name, id, release) => {
                    let state = format!("discography-hidden:{}", cache_key(&name, &id));
                    let mut hidden: Vec<String> = catalog.state(&state)?;
                    if !hidden.contains(&release) {
                        hidden.push(release);
                        catalog.set_state(&state, &hidden)?;
                    }
                    publish_discography(&catalog, &events, &name, &id, false)?;
                }
                Command::SlskdAuth(key, username, password) => {
                    slskd = None;
                    let saved = SlskdAuth::from_fields(&key, &username, &password)
                        .and_then(|auth| auth.map_or(Ok(()), |auth| paths.save_slskd_auth(&auth)));
                    if let Err(error) = saved {
                        events.send(Event::SlskdStatus(Err(format!("{error:#}"))))?;
                    }
                }
                Command::SlskdTest => match slskd_client(&mut slskd, &catalog, &paths) {
                    Ok(client) => {
                        let events = events.clone();
                        runtime.spawn(async move {
                            let result = client.test().await.map_err(|error| format!("{error:#}"));
                            let _ = events.send(Event::SlskdStatus(result));
                        });
                    }
                    Err(error) => events.send(Event::SlskdStatus(Err(format!("{error:#}"))))?,
                },
                Command::SoulseekSearch(release, query) => {
                    match slskd_client(&mut slskd, &catalog, &paths) {
                        Ok(client) => {
                            let events = events.clone();
                            let musicbrainz = musicbrainz.clone();
                            let known = catalog
                                .state::<Option<ReleaseDetails>>(&format!(
                                    "release-details:v1:{release}"
                                ))?
                                .map(|details| details.tracks.len() as u32)
                                .filter(|tracks| *tracks > 0);
                            events.send(Event::SoulseekSearch(
                                release.clone(),
                                SearchUpdate::Searching(0),
                            ))?;
                            runtime.spawn(async move {
                                let progress = events.clone();
                                let searching = release.clone();
                                let (responses, expected) = tokio::join!(
                                    client.search(&query, move |count| {
                                        let _ = progress.send(Event::SoulseekSearch(
                                            searching.clone(),
                                            SearchUpdate::Searching(count),
                                        ));
                                    }),
                                    async {
                                        match known {
                                            Some(tracks) => Ok(Some(tracks)),
                                            None => musicbrainz.track_count(&release).await,
                                        }
                                    }
                                );
                                let expected = expected.ok().flatten();
                                let _ = events.send(Event::SoulseekSearch(
                                    release,
                                    match responses {
                                        Ok(responses) => SearchUpdate::Results(
                                            Arc::new(crate::slskd::candidates(
                                                &responses, expected,
                                            )),
                                            expected,
                                        ),
                                        Err(error) => SearchUpdate::Failed(format!("{error:#}")),
                                    },
                                ));
                            });
                        }
                        Err(error) => events.send(Event::SoulseekSearch(
                            release,
                            SearchUpdate::Failed(format!("{error:#}")),
                        ))?,
                    }
                }
                Command::Download(request) => {
                    let client = slskd_client(&mut slskd, &catalog, &paths)?;
                    let namespace = account.as_ref().map(Account::namespace).unwrap_or_default();
                    let DownloadRequest {
                        artist,
                        artist_id,
                        release,
                        title,
                        expected,
                        candidate,
                    } = *request;
                    anyhow::ensure!(
                        watched.insert(release.clone()),
                        "This release is already downloading"
                    );
                    let tx = tx.clone();
                    let events = events.clone();
                    let runtime_handle = runtime.clone();
                    runtime.spawn(async move {
                        // Search hits only cover files that matched the text; take the whole folder.
                        let mut files = candidate.files.clone();
                        if let Ok(listed) = client
                            .directory(&candidate.username, &candidate.directory)
                            .await
                        {
                            for file in listed {
                                if !files.iter().any(|known| known.filename == file.filename) {
                                    files.push(file);
                                }
                            }
                        }
                        let mut job = DownloadJob {
                            artist: cache_key(&artist, &artist_id),
                            release,
                            title,
                            username: candidate.username.clone(),
                            expected,
                            files: files.iter().map(|file| file.filename.clone()).collect(),
                            ..Default::default()
                        };
                        job.progress.total = job.files.len() as u32;
                        match client.enqueue(&candidate.username, &files).await {
                            Ok(()) => {
                                let _ = tx
                                    .send(Command::DownloadUpdate(namespace.clone(), job.clone()));
                                watch_download(&runtime_handle, client, tx, namespace, job);
                            }
                            Err(error) => {
                                let _ = tx.send(Command::DismissDownload(job.release));
                                let _ = events
                                    .send(Event::Error(format!("Download not started: {error:#}")));
                            }
                        }
                    });
                }
                Command::DownloadUpdate(namespace, job) => {
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        let mut jobs: Vec<DownloadJob> = catalog.state(DOWNLOADS)?;
                        let finished = job.state != JobState::Active;
                        let downloaded = job.state == JobState::Downloaded;
                        if finished {
                            watched.remove(&job.release);
                        }
                        match jobs.iter_mut().find(|known| known.release == job.release) {
                            Some(known) => *known = job,
                            None => jobs.push(job),
                        }
                        catalog.set_state(DOWNLOADS, &jobs)?;
                        events.send(Event::Downloads(Arc::new(jobs)))?;
                        if downloaded && let Some(server) = server.clone() {
                            let tx = tx.clone();
                            let events = events.clone();
                            runtime.spawn(async move {
                                // slskd reports success just before it moves the file into place.
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                if server.start_scan().await.is_err() {
                                    return;
                                }
                                let _ = events.send(Event::Status(
                                    "Download finished · Navidrome is scanning".into(),
                                ));
                                for _ in 0..200 {
                                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                                    if matches!(server.scan_status().await, Ok(false)) {
                                        break;
                                    }
                                }
                                let _ = tx.send(Command::Refresh);
                            });
                        }
                    }
                }
                Command::DismissDownload(release) => {
                    watched.remove(&release);
                    let mut jobs: Vec<DownloadJob> = catalog.state(DOWNLOADS)?;
                    jobs.retain(|job| job.release != release);
                    catalog.set_state(DOWNLOADS, &jobs)?;
                    events.send(Event::Downloads(Arc::new(jobs)))?;
                }
                Command::ResumeDownloads => {
                    let jobs: Vec<DownloadJob> = catalog.state(DOWNLOADS)?;
                    events.send(Event::Downloads(Arc::new(jobs.clone())))?;
                    let active = jobs
                        .into_iter()
                        .filter(|job| job.state == JobState::Active)
                        .collect::<Vec<_>>();
                    if !active.is_empty()
                        && let Ok(client) = slskd_client(&mut slskd, &catalog, &paths)
                    {
                        let namespace =
                            account.as_ref().map(Account::namespace).unwrap_or_default();
                        for job in active {
                            if watched.insert(job.release.clone()) {
                                watch_download(
                                    &runtime,
                                    client.clone(),
                                    tx.clone(),
                                    namespace.clone(),
                                    job,
                                );
                            }
                        }
                    }
                }
                Command::ArtistProfile(name, id, full) => {
                    let key = cache_key(&name, &id);
                    let mut cached = catalog.artist_profile(&key)?;
                    if let Some(profile) = &mut cached {
                        catalog.hydrate_artist_profile(profile)?;
                    }
                    if let Some(profile) = &cached {
                        events
                            .send(Event::ArtistProfile(key.clone(), Arc::new(profile.clone())))?;
                    }
                    let now = chrono::Utc::now().timestamp_millis();
                    let fresh = cached
                        .as_ref()
                        .is_some_and(|profile| profile.fresh(full, now));
                    if !fresh && server.is_some() {
                        let namespace =
                            account.as_ref().map(Account::namespace).unwrap_or_default();
                        if !profile_requests.contains(&(namespace, key.clone(), full))
                            && !profile_pending.iter().any(
                                |(pending_name, pending_id, pending_full)| {
                                    cache_key(pending_name, pending_id) == key
                                        && (*pending_full || !full)
                                },
                            )
                        {
                            if full {
                                profile_pending.push_front((name, id, full));
                            } else {
                                profile_pending.push_back((name, id, full));
                            }
                        }
                    } else if cached.is_none() && server.is_none() {
                        events.send(Event::ArtistProfile(
                            key,
                            Arc::new(ArtistProfile {
                                name,
                                ..Default::default()
                            }),
                        ))?;
                    }
                }
                Command::ArtistProfileLoaded(namespace, key, full, result) => {
                    profile_requests.remove(&(namespace.clone(), key.clone(), full));
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        let previous = catalog.artist_profile(&key)?;
                        let mut profile = result.unwrap_or_else(|_| ArtistProfile {
                            fetched_ms: chrono::Utc::now().timestamp_millis(),
                            ..Default::default()
                        });
                        if !profile.complete
                            && let Some(old) = previous
                        {
                            if profile.name.is_empty() {
                                profile.name = old.name;
                            }
                            if profile.id.is_empty() {
                                profile.id = old.id;
                            }
                            if profile.portrait.is_empty() {
                                profile.portrait = old.portrait;
                            }
                            profile.biography = old.biography;
                            profile.similar = old.similar;
                            profile.popular = old.popular;
                            profile.url = old.url;
                            if old.complete {
                                profile.complete = true;
                                profile.fetched_ms = old.fetched_ms;
                            }
                        }
                        catalog.hydrate_artist_profile(&mut profile)?;
                        catalog.save_artist_profile(&key, &profile)?;
                        events.send(Event::ArtistProfile(key, Arc::new(profile)))?;
                    }
                }
                Command::TopSongs(name, id) => {
                    let key = cache_key(&name, &id);
                    let cached: Option<TopSongs> = catalog.state(&format!("top-songs:{key}"))?;
                    if let Some(top) = &cached {
                        events.send(Event::TopSongs(key.clone(), Arc::new(top.clone())))?;
                    }
                    let now = chrono::Utc::now().timestamp_millis();
                    if !cached.is_some_and(|top| top.fresh(now))
                        && let Some(server) = server.clone()
                    {
                        let namespace = server.account.namespace();
                        if top_song_requests.insert((namespace.clone(), key.clone())) {
                            let tx = tx.clone();
                            runtime.spawn(async move {
                                let result = server
                                    .top_songs(&name, &id)
                                    .await
                                    .map(|tracks| {
                                        TopSongs::from_tracks(
                                            &tracks,
                                            chrono::Utc::now().timestamp_millis(),
                                        )
                                    })
                                    .map_err(|error| format!("{error:#}"));
                                let _ = tx.send(Command::TopSongsLoaded(namespace, key, result));
                            });
                        }
                    }
                }
                Command::TopSongsLoaded(namespace, key, result) => {
                    top_song_requests.remove(&(namespace.clone(), key.clone()));
                    if account.as_ref().map(Account::namespace) == Some(namespace)
                        && let Ok(top) = result
                    {
                        catalog.set_state(&format!("top-songs:{key}"), &Some(&top))?;
                        events.send(Event::TopSongs(key, Arc::new(top)))?;
                    }
                }
                Command::Playlist(id) => {
                    if let Some(playlist) = catalog.playlist(&id)? {
                        events.send(Event::Playlist(playlist))?;
                    }
                    if let Some(server) = server.clone() {
                        let tx = tx.clone();
                        runtime.spawn(async move {
                            match server.playlist(&id).await {
                                Ok(playlist) => {
                                    let _ = tx.send(Command::PlaylistLoaded(playlist));
                                }
                                Err(error) => {
                                    let _ = tx.send(Command::Error(format!(
                                        "Could not load playlist: {error:#}"
                                    )));
                                }
                            }
                        });
                    }
                }
                Command::PlaylistsLoaded(playlists) => {
                    catalog.save_playlist_summaries(&playlists, true)?;
                    events.send(Event::Playlists(catalog.playlist_summaries()?))?;
                }
                Command::PlaylistLoaded(playlist) => {
                    catalog.save_playlist(&playlist)?;
                    events.send(Event::Playlist(playlist))?;
                    events.send(Event::Playlists(catalog.playlist_summaries()?))?;
                }
                Command::Lyrics(track) => {
                    // Older cache entries contain flattened text with timing discarded.
                    let key = format!("lyrics:v2:{}", track.id);
                    let cached: Option<Lyrics> = catalog.state(&key)?;
                    if let Some(lyrics) = cached {
                        events.send(Event::Lyrics(track.id.clone(), Ok(Arc::new(lyrics))))?;
                    } else if let Some(server) = server.clone() {
                        let tx = tx.clone();
                        runtime.spawn(async move {
                            let result = server
                                .lyrics(&track)
                                .await
                                .map_err(|error| format!("{error:#}"));
                            let _ = tx.send(Command::LyricsLoaded(
                                server.account.namespace(),
                                track.id.clone(),
                                result,
                            ));
                        });
                    } else {
                        events.send(Event::Lyrics(
                            track.id.clone(),
                            Err("Lyrics are unavailable while offline.".into()),
                        ))?;
                    }
                }
                Command::LyricsLoaded(namespace, id, result) => {
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        if let Ok(lyrics) = &result {
                            catalog.set_state(&format!("lyrics:v2:{id}"), &Some(lyrics))?;
                        }
                        events.send(Event::Lyrics(id, result.map(Arc::new)))?;
                    }
                }
                Command::Artwork(id, fetch) => {
                    if let Some(account) = &account {
                        let path = paths
                            .cache
                            .join(account.namespace())
                            .join(format!("{}.art", blake3::hash(id.as_bytes()).to_hex()));
                        if path.exists() {
                            publish_artwork(&runtime, &events, id, path);
                            return Ok(());
                        }
                    }
                    if let Some(priority) = fetch {
                        artwork_queue.request(id, Instant::now(), priority);
                    }
                }
                Command::Backdrop(id) => {
                    if let Some(account) = &account {
                        let namespace = account.namespace();
                        let art = paths
                            .cache
                            .join(&namespace)
                            .join(format!("{}.art", blake3::hash(id.as_bytes()).to_hex()));
                        let tx = tx.clone();
                        runtime.spawn_blocking(move || {
                            let result = backdrop::load_or_render(&art)
                                .map_err(|error| format!("{error:#}"));
                            let _ = tx.send(Command::BackdropFinished(namespace, id, result));
                        });
                    }
                }
                Command::BackdropFinished(namespace, id, result) => {
                    if let Ok(backdrop) = result
                        && account
                            .as_ref()
                            .is_some_and(|account| account.namespace() == namespace)
                    {
                        events.send(Event::Backdrop(id, Arc::new(backdrop)))?;
                    }
                }
                Command::ArtworkFinished(namespace, id, result) => {
                    if account
                        .as_ref()
                        .is_some_and(|account| account.namespace() == namespace)
                    {
                        match result {
                            Ok(path) => {
                                artwork_queue.finished(&id, true, Instant::now());
                                publish_artwork(&runtime, &events, id, path);
                            }
                            Err(_) => {
                                if artwork_queue.finished(&id, false, Instant::now()) {
                                    events.send(Event::ArtworkFailed(id))?;
                                }
                            }
                        }
                    }
                }
                Command::PlayIds(ids, at) => {
                    let tracks = catalog.by_ids(&ids)?;
                    player.send(PlayerCommand::Replace(tracks, at));
                }
                Command::PlayAlbum(id) => {
                    player.send(PlayerCommand::Replace(catalog.album_tracks(&id)?, 0));
                }
                Command::Enqueue(ids, placement) => {
                    player.send(PlayerCommand::Insert(catalog.by_ids(&ids)?, placement));
                }
                Command::EnqueueAlbum(id, placement) => {
                    player.send(PlayerCommand::Insert(catalog.album_tracks(&id)?, placement));
                }
                Command::Player(command) => player.send(command),
                Command::Preferences(prefs) => {
                    catalog.set_state("preferences", &prefs)?;
                    player.send(PlayerCommand::Preferences(prefs));
                }
                Command::Persist(queue) => catalog.set_state("queue", &queue)?,
                Command::Scrobble(report) => {
                    if account.as_ref().map(Account::namespace) == Some(report.account.clone()) {
                        if report.submission {
                            catalog.queue_scrobble(&report)?;
                        } else if let Some(server) = server.clone() {
                            runtime.spawn(async move {
                                let _ = server
                                    .scrobble(&report.track_id, false, report.started_ms)
                                    .await;
                            });
                        }
                    }
                }
                Command::ScrobbleFinished(namespace, id, result) => {
                    submission_requests.remove(&(namespace.clone(), id));
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        catalog.finish_scrobble(id, result.is_ok())?;
                        match result {
                            Ok(Some(fresh)) => {
                                if let Some(track) = catalog.refresh_play_count(&fresh)? {
                                    player.send(PlayerCommand::UpdateTrack(track.clone()));
                                    events.send(Event::TrackUpdated(track))?;
                                    if current_query.ordering.as_ref().is_some_and(|s| {
                                        s.scope == SortScope::Tracks
                                            && s.depends_on(SortField::Plays)
                                    }) || (current_query.ordering.is_none()
                                        && current_query.sort == Sort::Plays)
                                    {
                                        events.send(Event::View(
                                            current_request,
                                            catalog.view(&current_query)?,
                                        ))?;
                                    }
                                }
                            }
                            Ok(None) => {}
                            Err(message) => {
                                events.send(Event::Error(message))?;
                            }
                        }
                    }
                }
                Command::Mutate(mutation) => {
                    let Some(server) = server.clone() else {
                        tx.send(Command::MutationFailed(
                            account.as_ref().map(Account::namespace).unwrap_or_default(),
                            mutation,
                            "Connect to the server before saving changes.".into(),
                        ))?;
                        return Ok(());
                    };
                    let namespace = server.account.namespace();
                    let tx = tx.clone();
                    runtime.spawn(async move {
                        let result = match &mutation {
                            Mutation::Favorite(id, value) => server.star(id, *value).await,
                            Mutation::Rating(id, rating) => server.rating(id, *rating).await,
                            Mutation::AlbumFavorite(id, value) => {
                                server.album_star(id, *value).await
                            }
                            Mutation::AlbumRating(id, rating) => server.rating(id, *rating).await,
                            Mutation::CreatePlaylist(name, ids) => {
                                server.create_playlist(name, ids).await
                            }
                            Mutation::ReplacePlaylist(id, ids) => {
                                server.replace_playlist(id, ids).await
                            }
                            Mutation::RenamePlaylist(id, name) => {
                                server.rename_playlist(id, name).await
                            }
                            Mutation::DeletePlaylist(id) => server.delete_playlist(id).await,
                        };
                        match result {
                            Ok(()) => {
                                let _ = tx.send(Command::Updated(namespace, mutation));
                            }
                            Err(error) => {
                                let _ = tx.send(Command::MutationFailed(
                                    namespace,
                                    mutation,
                                    format!(
                                        "Change was not saved: {error:#}. Retry when connected."
                                    ),
                                ));
                            }
                        }
                    });
                }
                Command::Updated(namespace, mutation) => {
                    if account.as_ref().map(Account::namespace).unwrap_or_default() != namespace {
                        return Ok(());
                    }
                    let completed = mutation.clone();
                    // Track edits are patched in place by the UI; a fresh view would
                    // reload every album panel, so send one only when the edit can
                    // change which rows appear or their order.
                    let reshapes_view = match &mutation {
                        Mutation::Favorite(..) => current_query.favorites,
                        Mutation::Rating(..) => current_query
                            .ordering
                            .as_ref()
                            .map_or(current_query.sort == Sort::Rating, |s| {
                                s.scope == SortScope::Tracks && s.depends_on(SortField::Rating)
                            }),
                        Mutation::AlbumFavorite(..) => {
                            current_query.favorites
                                && current_query
                                    .ordering
                                    .as_ref()
                                    .is_some_and(|sort| sort.scope == SortScope::Albums)
                        }
                        Mutation::AlbumRating(..) => current_query.ordering.as_ref().map_or(
                            current_query.sort == Sort::Rating,
                            |sort| {
                                sort.scope == SortScope::Albums
                                    && sort.depends_on(SortField::Rating)
                            },
                        ),
                        _ => true,
                    };
                    match mutation {
                        Mutation::Favorite(id, value) => {
                            if let Some(track) = catalog.by_ids(&[id])?.first() {
                                let mut track = (**track).clone();
                                track.starred = value.then(|| chrono::Utc::now().to_rfc3339());
                                catalog.put_tracks(std::slice::from_ref(&track), generation)?;
                                let track = Arc::new(track);
                                player.send(PlayerCommand::UpdateTrack(track.clone()));
                                events.send(Event::TrackUpdated(track))?;
                            }
                        }
                        Mutation::Rating(id, rating) => {
                            if let Some(track) = catalog.by_ids(&[id])?.first() {
                                let mut track = (**track).clone();
                                track.user_rating = Some(rating);
                                catalog.put_tracks(std::slice::from_ref(&track), generation)?;
                                let track = Arc::new(track);
                                player.send(PlayerCommand::UpdateTrack(track.clone()));
                                events.send(Event::TrackUpdated(track))?;
                            }
                        }
                        Mutation::AlbumFavorite(id, favorite) => {
                            if let Some(album) =
                                catalog.update_album_feedback(&id, Some(favorite), None)?
                            {
                                events.send(Event::AlbumUpdated(album))?;
                            }
                        }
                        Mutation::AlbumRating(id, rating) => {
                            if let Some(album) =
                                catalog.update_album_feedback(&id, None, Some(rating))?
                            {
                                events.send(Event::AlbumUpdated(album))?;
                            }
                        }
                        _ => {
                            if let Some(server) = server.clone() {
                                let tx = tx.clone();
                                let changed_id = match mutation {
                                    Mutation::ReplacePlaylist(id, _)
                                    | Mutation::RenamePlaylist(id, _) => Some(id),
                                    _ => None,
                                };
                                runtime.spawn(async move {
                                    let result = async {
                                        let playlists = server.playlists().await?;
                                        let _ = tx.send(Command::PlaylistsLoaded(playlists));
                                        if let Some(id) = changed_id {
                                            let playlist = server.playlist(&id).await?;
                                            let _ = tx.send(Command::PlaylistLoaded(playlist));
                                        }
                                        Ok::<_, anyhow::Error>(())
                                    }
                                    .await;
                                    if let Err(error) = result {
                                        let _ = tx.send(Command::Error(format!(
                                            "Saved, but could not refresh playlists: {error:#}"
                                        )));
                                    }
                                });
                            }
                        }
                    }
                    events.send(Event::MutationDone(completed))?;
                    if reshapes_view {
                        events.send(Event::View(current_request, catalog.view(&current_query)?))?;
                    }
                }
                Command::MutationFailed(namespace, mutation, error) => {
                    if account.as_ref().map(Account::namespace).unwrap_or_default() != namespace {
                        return Ok(());
                    }
                    if let Mutation::Favorite(id, _) | Mutation::Rating(id, _) = &mutation
                        && let Some(track) = catalog.by_ids(std::slice::from_ref(id))?.first()
                    {
                        player.send(PlayerCommand::UpdateTrack(track.clone()));
                        events.send(Event::TrackUpdated(track.clone()))?;
                    }
                    if let Some(id) = mutation.album_feedback_id()
                        && let Some(album) = catalog.album(id)?
                    {
                        events.send(Event::AlbumUpdated(album))?;
                    }
                    events.send(Event::MutationFailed(mutation, error))?;
                }
                Command::SyncFailed(failed, error) => {
                    failed_generations.insert(failed);
                    if failed == generation {
                        syncing = false;
                        events.send(Event::Error(error))?;
                    }
                }
                Command::Error(error) => {
                    events.send(Event::Error(error))?;
                }
                Command::Shutdown(_) => unreachable!(),
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = events.send(Event::Error(format!("{error:#}")));
        }
    }
    Ok(())
}
