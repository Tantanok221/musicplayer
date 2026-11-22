use crate::{
    artists::{ArtistPageData, ArtistProfile, TopSongs, cache_key},
    artwork::{ArtworkQueue, Priority},
    backdrop::{self, Backdrop},
    catalog::{Catalog, LibraryView},
    discography::{self, Discography, DiscographyView, ReleaseDetails},
    discover::History,
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
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const SIGN_IN_NOTICE: &str =
    "Sign in once to save your login locally. Sonora no longer uses Keychain.";

pub mod native;

#[derive(Clone, Debug, PartialEq)]
pub enum Mutation {
    Favorite(String, bool),
    Rating(String, u8),
    AlbumFavorite(String, bool),
    AlbumRating(String, u8),
    CreatePlaylist(String, Vec<String>),
    ReorderPlaylist {
        id: String,
        original: Vec<String>,
        reordered: Vec<String>,
    },
    RenamePlaylist(String, String),
    DeletePlaylist(String),
    /// Playlist ID and the songs appended to it.
    AddToPlaylist(String, Vec<String>),
    RemoveFromPlaylist(String, usize),
}
/// Where an album's songs go: a new playlist by name, or an existing one by ID.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaylistTarget {
    New(String),
    Existing(String),
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
    CancelSmartPlaylist,
    LoadSmartRules(String),
    SaveSmartPlaylist {
        id: Option<String>,
        name: String,
        rules: crate::smart_rules::SmartRules,
    },
    NativeSignIn(String),
    NativeSessionReady(u64),
    NativeRenewed(
        u64,
        crate::platform::NativeToken,
        Result<crate::platform::NativeToken, crate::server::NativeError>,
    ),
    NativeCompleted(u64, Result<native::Reply, crate::server::NativeError>),
    NativeSignedIn(u64, Result<(), String>),
    NativeRefreshed(u64, Result<(Vec<Playlist>, Playlist), String>),
    Connect(Account, String),
    Refresh,
    Initialize,
    Query(u64, LibraryQuery),
    Page(u64, u64, LibraryQuery, usize),
    Album(String),
    Artist(u64, String, String),
    Discover(u64),
    HideDiscover(String, crate::discover::Dismissal),
    /// Account namespace, Last.fm user and when their profile was created.
    LastFmRegistered(String, String, i64),
    /// Account namespace, Last.fm user, season key, its chart, and progress.
    LastFmSeason(
        String,
        String,
        String,
        crate::discover::SeasonChart,
        usize,
        usize,
    ),
    LastFmDone(String, String, Result<(), String>),
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
    /// A Last.fm API key as typed; empty keeps what is saved.
    LastFmKey(String),
    SlskdTest,
    /// Release group and search text.
    SoulseekSearch(String, String),
    Download(Box<DownloadRequest>),
    DownloadUpdate(String, DownloadJob),
    DownloadRejected(String, String, String),
    DismissDownload(String),
    ResumeDownloads,
    ScrobbleFinished(String, u64, Result<Option<Arc<Track>>, String>),
    GroupTracks(u64, LibraryQuery, String),
    PlayTrack(String),
    Playlist(String),
    /// Finds a cover in the disk cache; with a priority, downloads it when missing.
    Artwork(String, Option<Priority>),
    ArtworkFinished(String, String, Result<PathBuf, String>),
    ArtworkPrepared(String, String, PathBuf, bool),
    #[cfg(feature = "test-support")]
    ArtworkTime(Duration, Sender<Option<Duration>>),
    Backdrop(String),
    BackdropFinished(String, String, Result<Backdrop, String>),
    Lyrics(Arc<Track>),
    LyricsLoaded(String, String, Result<Lyrics, String>),
    PlayIds(Vec<String>, usize),
    PlayAlbum(String),
    /// Plays the album's songs that match the query, as its expanded card lists them.
    PlayAlbumMatching(LibraryQuery, String),
    Enqueue(Vec<String>, Placement),
    EnqueueAlbum(String, Placement),
    /// Loads a playlist by ID and queues its entries in playlist order.
    EnqueuePlaylist(String, Placement),
    /// Album ID: save its songs, in disc/track order, to a playlist.
    AlbumToPlaylist(String, PlaylistTarget),
    Mutate(Mutation),
    /// Writes the edit into the files under the mapped music folder.
    WriteTags(crate::tags::TagEdit),
    /// Puts back the tags the last edit changed.
    UndoTags,
    Preferences(Preferences),
    Player(PlayerCommand),
    Persist(String, SavedQueue),
    Scrobble(ListenReport),
    Connected(Server, String, Option<flume::Receiver<()>>),
    Tracks(i64, Vec<Track>),
    Albums(i64, Vec<Album>),
    Synced(i64, Vec<Playlist>, bool),
    Updated(String, Mutation),
    MutationFailed(String, Mutation, String),
    PlaylistLoaded(String, Playlist),
    PlaylistForQueue(u64, Placement, Option<Playlist>),
    PlaylistsLoaded(String, Vec<Playlist>),
    Error(String),
    SyncFailed(i64, String),
    /// Pings the server when a check is due; `true` checks right away.
    CheckConnection(bool),
    SaveWindow(crate::platform::WindowFrame),
    /// Account namespace and whether its server answered.
    ConnectionChecked(String, Result<(), String>),
    Shutdown(Sender<()>),
}

/// Whether the saved Navidrome server is reachable, shown in the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    /// No server account is saved or signed in.
    #[default]
    None,
    Connecting,
    Online,
    /// The server did not answer; the reason is for the tooltip.
    Offline(String),
}
impl Connection {
    /// How long to wait between automatic checks in this state.
    fn check_interval(&self) -> Duration {
        match self {
            Self::Online => Duration::from_secs(30),
            _ => Duration::from_secs(10),
        }
    }
}
const CONNECTION_TICK: Duration = Duration::from_secs(5);
/// Library pages requested at once while syncing.
const SYNC_REQUESTS_AHEAD: usize = 4;
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
    NativeAvailable(bool),
    SmartRules(String, Option<crate::smart_rules::SmartRules>),
    SmartPlaylistSaved(String),
    NativePasswordRequired,
    Ready(Option<Account>, Preferences),
    SignInRequired(String),
    Connected(Account, String),
    View(u64, LibraryView),
    Page(u64, u64, usize, Vec<Arc<Track>>),
    Album(String, Vec<Arc<Track>>),
    Artist(u64, ArtistPageData),
    Discover(u64, Arc<crate::discover::Rediscover>),
    ArtistProfile(String, Arc<ArtistProfile>),
    TopSongs(String, Arc<TopSongs>),
    Discography(String, Arc<DiscographyView>),
    SoulseekSearch(String, SearchUpdate),
    ReleaseDetails(String, Result<Arc<ReleaseDetails>, String>),
    Downloads(Arc<Vec<DownloadJob>>),
    /// Release ID and why slskd did not accept the download, shown beside the search results.
    DownloadNotStarted(String, String),
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
    Connection(Connection),
    Error(String),
    MutationDone(Mutation),
    MutationFailed(Mutation, String),
    AlbumUpdated(Arc<Album>),
    TrackUpdated(Arc<Track>),
    TagsWritten(crate::tags::TagReport),
}
#[derive(Clone)]
pub struct Service {
    pub tx: Sender<Command>,
    pub player: PlayerHandle,
    query_epoch: Arc<AtomicU64>,
}

#[cfg(feature = "test-support")]
pub mod test_support;

/// Writes an edit into every selected file it can find, journaling each
/// file before it is touched.
fn write_tags(catalog: &Catalog, edit: &crate::tags::TagEdit) -> Result<crate::tags::TagReport> {
    use crate::tags::{self, EditState};
    let prefs: Preferences = catalog.state("preferences")?;
    let mut report = tags::TagReport {
        job: chrono::Utc::now().timestamp_micros(),
        ..Default::default()
    };
    let tracks = catalog.by_ids(&edit.tracks)?;
    if !prefs.metadata.write_tags {
        report.failures = tracks
            .iter()
            .map(|track| {
                (
                    track.title.clone(),
                    "Turn on Write tags to files in Settings → Metadata first.".into(),
                )
            })
            .collect();
        return Ok(report);
    }
    let root = std::path::PathBuf::from(&prefs.metadata.library_folder);
    // A file written since the last rescan no longer matches the size Navidrome has.
    let unconfirmed = catalog.unconfirmed_tag_edits()?;
    for track in tracks {
        let file = match unconfirmed
            .iter()
            .rev()
            .find(|entry| entry.track_id == track.id)
        {
            Some(entry) => Ok(std::path::PathBuf::from(&entry.file)),
            None => tags::resolve(&root, &track),
        };
        let file = match file {
            Ok(file) => file,
            Err(reason) => {
                report.failures.push((track.title.clone(), reason));
                continue;
            }
        };
        let before = match tags::read(&file) {
            Ok(before) => before,
            Err(error) => {
                report
                    .failures
                    .push((track.title.clone(), format!("{error:#}")));
                continue;
            }
        };
        let id = catalog.journal_tag_edit(report.job, &track, &file, &before)?;
        match tags::write(&file, &edit.fields) {
            Ok((_, after)) => {
                catalog.finish_tag_edit(id, Some(&after), EditState::Written)?;
                report.written += 1;
            }
            Err(error) => {
                catalog.finish_tag_edit(id, None, EditState::Failed)?;
                report
                    .failures
                    .push((track.title.clone(), format!("{error:#}")));
            }
        }
    }
    Ok(report)
}

/// Puts back what the newest job changed, as a new job so it can be undone too.
fn undo_tags(catalog: &Catalog) -> Result<crate::tags::TagReport> {
    use crate::tags::{self, EditState};
    let mut report = tags::TagReport {
        job: chrono::Utc::now().timestamp_micros(),
        undo: true,
        ..Default::default()
    };
    let Some(job) = catalog.last_tag_job()? else {
        report
            .failures
            .push((String::new(), "There is no tag edit to undo.".into()));
        return Ok(report);
    };
    for entry in catalog.tag_job(job)? {
        if !matches!(entry.state, EditState::Written | EditState::Confirmed) {
            continue;
        }
        let changed: tags::TagValues = entry
            .after
            .iter()
            .filter(|(field, values)| entry.before.get(field) != Some(values))
            .map(|(field, values)| (*field, values.clone()))
            .collect();
        let track = catalog
            .track_by_path(&entry.path)?
            .map(|track| (*track).clone())
            .unwrap_or_else(|| Track {
                id: entry.track_id.clone(),
                path: entry.path.clone(),
                title: entry.title.clone(),
                starred: entry.starred.then(String::new),
                user_rating: entry.rating,
                ..Default::default()
            });
        let file = std::path::PathBuf::from(&entry.file);
        let current = match tags::read(&file) {
            Ok(current) => current,
            Err(error) => {
                report
                    .failures
                    .push((entry.title.clone(), format!("{error:#}")));
                continue;
            }
        };
        let id = catalog.journal_tag_edit(report.job, &track, &file, &current)?;
        match tags::write(&file, &tags::restore(&entry.before, &changed)) {
            Ok((_, after)) => {
                catalog.finish_tag_edit(id, Some(&after), EditState::Written)?;
                catalog.finish_tag_edit(entry.id, None, EditState::Undone)?;
                report.written += 1;
            }
            Err(error) => {
                catalog.finish_tag_edit(id, None, EditState::Failed)?;
                report
                    .failures
                    .push((entry.title.clone(), format!("{error:#}")));
            }
        }
    }
    Ok(report)
}

/// Reports a tag job and asks Navidrome to rescan. Only administrators can
/// start a scan; for others Navidrome's file watcher picks the change up.
fn finish_tag_job(
    report: &crate::tags::TagReport,
    server: Option<Server>,
    runtime: &tokio::runtime::Handle,
    tx: &Sender<Command>,
    events: &Sender<Event>,
) -> Result<()> {
    events.send(Event::TagsWritten(report.clone()))?;
    if report.written == 0 {
        return Ok(());
    }
    let Some(server) = server else {
        events.send(Event::Status(
            "Tags saved · connect to Navidrome to rescan".into(),
        ))?;
        return Ok(());
    };
    let (tx, events) = (tx.clone(), events.clone());
    runtime.spawn(async move {
        if server.start_scan().await.is_err() {
            let _ = events.send(Event::Status(
                "Tags saved · Navidrome shows them after its next scan".into(),
            ));
            return;
        }
        let _ = events.send(Event::Status("Tags saved · Navidrome is rescanning".into()));
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        for _ in 0..600 {
            if matches!(server.scan_status().await, Ok(false)) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        let _ = tx.send(Command::Refresh);
    });
    Ok(())
}

/// After a complete sync, finds each written file by path and, when the
/// rescan gave it a new ID, carries its favorite and rating over.
/// Confirms written edits Navidrome has rescanned. Returns annotations to
/// carry over to new IDs, and each rescanned song with the ID it had before.
type Reconciled = (Vec<Mutation>, Vec<(String, Arc<Track>)>);

fn reconcile_tags(catalog: &Catalog) -> Result<Reconciled> {
    use crate::tags::EditState;
    let mut mutations = Vec::new();
    let mut rescanned_tracks = Vec::new();
    for entry in catalog.unconfirmed_tag_edits()? {
        let Some(track) = catalog.track_by_path(&entry.path)? else {
            continue;
        };
        let rescanned = entry
            .after
            .iter()
            .filter(|(field, values)| entry.before.get(field) != Some(values))
            .all(|(field, values)| field.shown(&track) == values.join("; "));
        if !rescanned {
            continue;
        }
        if track.id != entry.track_id {
            if entry.starred && track.starred.is_none() {
                mutations.push(Mutation::Favorite(track.id.clone(), true));
            }
            if let Some(rating) = entry.rating.filter(|rating| *rating > 0)
                && track.user_rating != Some(rating)
            {
                mutations.push(Mutation::Rating(track.id.clone(), rating));
            }
        }
        catalog.finish_tag_edit(entry.id, None, EditState::Confirmed)?;
        rescanned_tracks.push((entry.track_id, track));
    }
    Ok((mutations, rescanned_tracks))
}

fn persist_preferences(catalog: &Catalog, player: &PlayerHandle, prefs: Preferences) -> Result<()> {
    catalog.set_state("preferences", &prefs)?;
    player.send(PlayerCommand::Preferences(prefs));
    Ok(())
}
impl Service {
    pub fn start(paths: Paths, runtime: tokio::runtime::Handle) -> (Self, Receiver<Event>) {
        let (tx, rx) = flume::unbounded();
        let (events, event_rx) = flume::unbounded();
        let (player, player_events) = PlayerHandle::start(runtime.clone());
        let query_epoch = Arc::new(AtomicU64::new(0));
        let service = Self {
            tx: tx.clone(),
            player,
            query_epoch,
        };
        let event_tx = events.clone();
        std::thread::Builder::new()
            .name("sonora-player-events".into())
            .spawn(move || forward_player_events(player_events, tx, event_tx))
            .expect("Could not start playback bridge");
        let worker = service.clone();
        std::thread::Builder::new()
            .name("sonora-catalog".into())
            .spawn(move || {
                let result = MusicBrainz::new().and_then(|metadata| {
                    let lastfm = crate::lastfm::LastFm::new()?;
                    run(paths, runtime, worker, rx, events.clone(), metadata, lastfm)
                });
                if let Err(error) = result {
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
fn forward_player_events(
    player_events: Receiver<PlayerEvent>,
    command_tx: Sender<Command>,
    event_tx: Sender<Event>,
) {
    let mut playback_failed = false;
    while let Ok(event) = player_events.recv() {
        match event {
            PlayerEvent::Snapshot(snapshot) => {
                // A new playback error is often the first sign the server went away.
                let failed = snapshot.error.is_some();
                if failed && !playback_failed {
                    let _ = command_tx.send(Command::CheckConnection(true));
                }
                playback_failed = failed;
                let _ = event_tx.send(Event::Playback(snapshot));
            }
            PlayerEvent::Persist(namespace, queue) => {
                let _ = command_tx.send(Command::Persist(namespace, queue));
            }
            PlayerEvent::Scrobble(report) => {
                let _ = command_tx.send(Command::Scrobble(report));
            }
            PlayerEvent::Error(error) => {
                let _ = event_tx.send(Event::Error(error));
            }
        }
    }
}

fn enqueue_playlist(
    playlist: Option<Playlist>,
    placement: Placement,
    player: &PlayerHandle,
    events: &Sender<Event>,
) -> Result<()> {
    match playlist {
        Some(playlist) if !playlist.entry.is_empty() => {
            player.send(PlayerCommand::Insert(
                playlist.entry.as_ref().clone(),
                placement,
            ));
        }
        Some(_) => events.send(Event::Status("This playlist is empty.".into()))?,
        None => events.send(Event::Error(
            "Could not load playlist tracks to queue.".into(),
        ))?,
    }
    Ok(())
}

/// Resolve saved occurrences individually so missing predecessors do not move
/// the current entry. A missing current entry starts its replacement at zero.
fn restore_queue(catalog: &Catalog, player: &PlayerHandle, namespace: String) -> Result<()> {
    let queue: SavedQueue = catalog.state("queue")?;
    let mut shuffle_positions = queue
        .shuffle_positions
        .filter(|positions| positions.len() == queue.ids.len());
    let current = queue.index.unwrap_or(0);
    let mut tracks = Vec::new();
    let mut index = None;
    let mut same = false;
    for (at, id) in queue.ids.iter().enumerate() {
        if let Some(track) = catalog.by_ids(std::slice::from_ref(id))?.pop() {
            if at == current {
                same = true;
            }
            if at >= current && index.is_none() {
                index = Some(tracks.len());
            }
            if let Some(positions) = &mut shuffle_positions {
                positions[tracks.len()] = positions[at];
            }
            tracks.push(track);
        }
    }
    if let Some(positions) = &mut shuffle_positions {
        positions.truncate(tracks.len());
    }
    let index = index.unwrap_or(tracks.len().saturating_sub(1));
    player.send(PlayerCommand::RestoreAccount {
        namespace,
        tracks,
        index,
        position: if same { queue.position } else { 0. },
        listen: if same { queue.listen } else { None },
        shuffle_positions,
    });
    Ok(())
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
            // Only a running download keeps its release listed; a failed one that the
            // library holds anyway (e.g. most files arrived) counts as owned.
            .filter(|job| job.artist == key && job.state == JobState::Active)
            .map(|job| job.release)
            .collect(),
    };
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    Ok(DiscographyView {
        loading: false,
        missing: discography::missing(&cached, &library, &today),
    })
}

/// Shows the artist's missing releases and forgets finished or failed downloads
/// the library now holds.
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
            || job.state == JobState::Active
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
                // A few pages in flight at once: syncing is bound by round trips, not the server.
                server.each_page(kind,SYNC_REQUESTS_AHEAD,|page| {
                    let result=(|| {
                        for item in &page{
                            let id=item["id"].as_str().unwrap_or_default();
                            if id.is_empty()||!seen.insert(id.to_string()){anyhow::bail!("Library changed during sync. Refresh again after the server scan finishes.");}
                        }
                        offset+=page.len();
                        let page=serde_json::Value::Array(page);
                        Ok(if kind=="song"{Command::Tracks(generation,serde_json::from_value(page)?)}else{Command::Albums(generation,serde_json::from_value(page)?)})
                    })();
                    let (tx,events)=(tx.clone(),events.clone());
                    let status=format!("Syncing {} · {offset:} indexed",if kind=="song"{"tracks"}else{"albums"});
                    async move {
                        tx.send_async(result?).await?;
                        let _=events.send(Event::Status(status));
                        Ok(())
                    }
                }).await?;
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
    tx: &Sender<Command>,
    events: &Sender<Event>,
    namespace: String,
    id: String,
    path: PathBuf,
) {
    if thumbnail::ready(&path) {
        let _ = events.send(Event::Artwork(id, path, true));
        return;
    }
    let tx = tx.clone();
    runtime.spawn_blocking(move || {
        let thumbs = thumbnail::ensure(&path).is_ok();
        let _ = tx.send(Command::ArtworkPrepared(namespace, id, path, thumbs));
    });
}

fn set_connection(current: &mut Connection, next: Connection, events: &Sender<Event>) {
    if *current != next {
        *current = next.clone();
        let _ = events.send(Event::Connection(next));
    }
}

/// Connects with the saved login. Returns false when there is no login to use.
fn reconnect(
    account: Account,
    paths: &Paths,
    runtime: &tokio::runtime::Handle,
    tx: &Sender<Command>,
    events: &Sender<Event>,
) -> bool {
    let credentials = match paths.credentials(&account) {
        Ok(Some(credentials)) => credentials,
        result => {
            let message = match result {
                Err(error) => format!("{error:#}"),
                _ => SIGN_IN_NOTICE.into(),
            };
            let _ = events.send(Event::SignInRequired(message));
            return false;
        }
    };
    let namespace = account.namespace();
    let tx = tx.clone();
    runtime.spawn(async move {
        let result = async {
            let mut server = Server::with_credentials(account, credentials)?;
            let label = server.connect().await?;
            Ok::<_, anyhow::Error>((server, label))
        }
        .await;
        let _ = tx.send(match result {
            Ok((server, label)) => Command::Connected(server, label, None),
            Err(error) => Command::ConnectionChecked(namespace, Err(format!("{error:#}"))),
        });
    });
    true
}

#[derive(Default)]
struct ArtworkClock {
    offset: Duration,
}
impl ArtworkClock {
    fn now(&self) -> Instant {
        Instant::now() + self.offset
    }
}

struct LastFmFetch {
    done: usize,
    total: usize,
}

fn lastfm_state(user: &str) -> String {
    format!("lastfm-history:v1:{}", user.to_lowercase())
}

fn load_history(catalog: &Catalog, user: &str) -> Result<History> {
    Ok(catalog
        .state::<Option<History>>(&lastfm_state(user))?
        .unwrap_or_else(|| History {
            user: user.into(),
            ..Default::default()
        }))
}

/// Fetches the profile's creation date if unknown, then every season whose
/// chart is missing or unfinished, handing each back as it arrives.
async fn fetch_lastfm(
    lastfm: crate::lastfm::LastFm,
    tx: Sender<Command>,
    namespace: String,
    user: String,
    mut history: History,
) {
    let result = async {
        if history.registered.is_none() {
            let registered = lastfm.registered(&user).await?;
            let _ = tx.send(Command::LastFmRegistered(
                namespace.clone(),
                user.clone(),
                registered,
            ));
            history.registered = Some(registered);
        }
        let seasons = crate::discover::stale_seasons(&history, chrono::Utc::now());
        let total = seasons.len();
        for (done, season) in seasons.into_iter().enumerate() {
            let albums = lastfm.album_chart(&user, season.from, season.to).await?;
            let _ = tx.send(Command::LastFmSeason(
                namespace.clone(),
                user.clone(),
                season.key,
                crate::discover::SeasonChart {
                    from: season.from,
                    to: season.to,
                    fetched_ms: chrono::Utc::now().timestamp_millis(),
                    albums,
                },
                done + 1,
                total,
            ));
        }
        Ok(())
    }
    .await
    .map_err(|error: anyhow::Error| format!("{error:#}"));
    let _ = tx.send(Command::LastFmDone(namespace, user, result));
}

fn publish_discover(
    catalog: &Catalog,
    events: &Sender<Event>,
    lastfm: &crate::lastfm::LastFm,
    request: u64,
    fetches: &HashMap<(String, String), LastFmFetch>,
    account: Option<&Account>,
    failure: Option<&(String, String)>,
) -> Result<()> {
    use crate::discover::HistoryStatus;
    let user = catalog
        .state::<Preferences>("preferences")?
        .lastfm_user
        .trim()
        .to_string();
    let history: Option<History> = if user.is_empty() || !lastfm.has_key() {
        None
    } else {
        catalog.state(&lastfm_state(&user))?
    };
    let fetch = account.and_then(|account| fetches.get(&(account.namespace(), user.clone())));
    let hidden: Option<crate::discover::Hidden> = catalog.state("discover-hidden")?;
    let mut shelves = crate::discover::rediscover(
        &catalog.rediscover_candidates()?,
        history.as_ref(),
        &hidden.unwrap_or_default(),
        chrono::Utc::now(),
    );
    shelves.history = if user.is_empty() {
        HistoryStatus::Off
    } else if !lastfm.has_key() {
        HistoryStatus::Unavailable
    } else if let Some(fetch) = fetch {
        HistoryStatus::Loading {
            done: fetch.done,
            total: fetch.total,
        }
    } else if let Some((_, message)) = failure.filter(|(failed, _)| *failed == user) {
        HistoryStatus::Failed(message.clone())
    } else {
        HistoryStatus::Ready
    };
    events.send(Event::Discover(request, Arc::new(shelves)))?;
    Ok(())
}

fn run(
    paths: Paths,
    runtime: tokio::runtime::Handle,
    service: Service,
    rx: Receiver<Command>,
    events: Sender<Event>,
    musicbrainz: MusicBrainz,
    lastfm: crate::lastfm::LastFm,
) -> Result<()> {
    let Service {
        player,
        tx,
        query_epoch,
        ..
    } = service;
    let mut account = paths.account()?;
    let playlist_track_edits = Arc::new(tokio::sync::Mutex::new(()));
    let mut catalog = Catalog::open(&catalog_path(&paths, account.as_ref()))?;
    let prefs: Preferences = catalog.state("preferences")?;
    player.send(PlayerCommand::Preferences(prefs.clone()));
    restore_queue(
        &catalog,
        &player,
        account.as_ref().map(Account::namespace).unwrap_or_default(),
    )?;
    events.send(Event::Ready(account.clone(), prefs))?;
    events.send(Event::Playlists(catalog.playlist_summaries()?))?;
    let interrupted = catalog.interrupted_tag_edits()?;
    if !interrupted.is_empty() {
        for entry in &interrupted {
            catalog.finish_tag_edit(entry.id, None, crate::tags::EditState::Failed)?;
        }
        events.send(Event::Error(format!(
            "Sonora stopped while writing tags to {} file(s), starting with {}. Check them before editing again.",
            interrupted.len(),
            interrupted[0].file
        )))?;
    }
    events.send(Event::Status(
        if account.is_some() {
            "Cached library · connecting…"
        } else {
            "Connect your Navidrome library in Settings"
        }
        .into(),
    ))?;
    let mut connection = Connection::None;
    let mut checking = false;
    let mut last_check = Instant::now();
    if let Some(account) = account.clone() {
        checking = reconnect(account, &paths, &runtime, &tx, &events);
        if checking {
            set_connection(&mut connection, Connection::Connecting, &events);
        }
    }
    if account.is_none() {
        events.send(Event::SignInRequired(SIGN_IN_NOTICE.into()))?;
    }
    {
        let ticker = tx.clone();
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(CONNECTION_TICK).await;
                if ticker
                    .send_async(Command::CheckConnection(false))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }
    let mut server: Option<Server> = None;
    let mut account_epoch = 0;
    let mut native_editor = native::Editor::default();
    let mut native_renewal = native::Renewal::default();
    let mut syncing = false;
    let mut generation = 0i64;
    let mut current_query = LibraryQuery::default();
    let mut current_request = 0;
    let mut failed_generations = HashSet::new();
    let mut artwork_queue = ArtworkQueue::default();
    #[cfg(feature = "test-support")]
    let mut artwork_clock = ArtworkClock::default();
    #[cfg(not(feature = "test-support"))]
    let artwork_clock = ArtworkClock::default();
    let mut profile_requests = HashSet::new();
    let mut profile_pending: VecDeque<(String, String, bool)> = VecDeque::new();
    let mut top_song_requests = HashSet::new();
    let mut submission_requests = HashSet::new();
    let mut discography_requests = HashSet::new();
    let mut discover_request = 0;
    let mut lastfm = lastfm;
    // A key entered in Settings wins over one this build was made with.
    if let Ok(Some(key)) = paths.lastfm_key() {
        lastfm.set_key(key);
    }
    let mut lastfm_fetches = HashMap::new();
    let mut lastfm_failure: Option<(String, String)> = None;
    let mut release_requests: HashSet<String> = HashSet::new();
    let mut open_artist: Option<(u64, String, String)> = None;
    let mut slskd: Option<(String, Slskd)> = None;
    let mut watched: HashSet<String> = HashSet::new();
    let _ = tx.send(Command::ResumeDownloads);
    let mut published = 0;
    loop {
        native_editor.start(server.as_ref(), &paths, &runtime, &tx);
        if let Some(server) = server.clone() {
            for id in artwork_queue.take_ready(artwork_clock.now()) {
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
        if connection == Connection::Online
            && let Some(server) = server.clone()
        {
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
            .and_then(|_| artwork_queue.next_retry(artwork_clock.now()));
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
                Command::CancelSmartPlaylist => native_editor.reset(),
                Command::LoadSmartRules(id) => {
                    native::Editor::require_server(server.as_ref())?;
                    native_editor.enqueue(native::Operation::Load(id), &events)?;
                }
                Command::SaveSmartPlaylist { id, name, rules } => {
                    native::Editor::require_server(server.as_ref())?;
                    anyhow::ensure!(!name.trim().is_empty(), "Enter a playlist name");
                    native_editor.enqueue(
                        native::Operation::Save {
                            id,
                            name,
                            rules: Box::new(rules),
                        },
                        &events,
                    )?;
                }
                Command::NativeSignIn(password) => {
                    let server = native::Editor::require_server(server.as_ref())?;
                    native_editor.sign_in(server, &paths, password, &runtime, &tx)?;
                }
                Command::NativeSignedIn(epoch, result) => native_editor.signed_in(epoch, result)?,
                Command::NativeRenewed(epoch, original, result) => {
                    native_renewal.complete(epoch, original, result, account.as_ref(), &paths)
                }
                Command::NativeSessionReady(epoch) => {
                    if epoch == native_editor.epoch {
                        native_editor.busy = false;
                    }
                }
                Command::NativeCompleted(epoch, result) => {
                    if let Some(reply) = native_editor.complete(epoch, result, &events)? {
                        match reply {
                            native::Reply::Rules(id, rules) => {
                                events.send(Event::SmartRules(id, rules))?
                            }
                            native::Reply::Saved(id) => {
                                events.send(Event::SmartPlaylistSaved(id.clone()))?;
                                let server =
                                    native::Editor::require_server(server.as_ref())?.clone();
                                let tx = tx.clone();
                                native_editor.busy = true;
                                runtime.spawn(async move {
                                    let result = async {
                                        let lists = server.playlists().await.map_err(|_| {
                                            "Saved, but could not refresh playlists".to_string()
                                        })?;
                                        let playlist =
                                            server.playlist(&id).await.map_err(|_| {
                                                "Saved, but could not refresh playlist tracks"
                                                    .to_string()
                                            })?;
                                        Ok((lists, playlist))
                                    }
                                    .await;
                                    let _ = tx.send(Command::NativeRefreshed(epoch, result));
                                });
                            }
                        }
                    }
                }
                Command::NativeRefreshed(epoch, result) => {
                    if epoch == native_editor.epoch {
                        native_editor.busy = false;
                        let (playlists, playlist) = result.map_err(anyhow::Error::msg)?;
                        catalog.save_playlist_summaries(&playlists, true)?;
                        catalog.save_playlist(&playlist)?;
                        events.send(Event::Playlists(catalog.playlist_summaries()?))?;
                        events.send(Event::Playlist(playlist))?;
                    }
                }
                Command::Initialize => {
                    events.send(Event::Ready(account.clone(), catalog.state("preferences")?))?;
                    if let Some(server) = &server {
                        events.send(Event::NativeAvailable(server.is_navidrome))?;
                    }
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
                                let session = native::sign_in_session(&server, &paths, password);
                                let _ = command_tx.send(Command::Connected(server, label, session));
                            }
                            Err(error) => {
                                let _ = command_tx
                                    .send(Command::Error(format!("Could not connect: {error:#}")));
                            }
                        }
                    });
                }
                Command::Connected(value, label, session) => {
                    if account.as_ref().map(Account::namespace) != Some(value.account.namespace()) {
                        if syncing {
                            anyhow::bail!(
                                "Wait for the current refresh to finish before changing accounts"
                            );
                        }
                        native_editor.reset();
                        native_renewal.reset();
                        if account.is_some() {
                            let (reply, saved) = flume::bounded(1);
                            player.send(PlayerCommand::SaveQueue(reply));
                            let saved =
                                saved.recv_timeout(Duration::from_secs(2)).map_err(|_| {
                                    anyhow::anyhow!(
                                        "Could not save the current queue before changing accounts"
                                    )
                                })?;
                            catalog.set_state("queue", &saved)?;
                        }
                        artwork_queue = ArtworkQueue::default();
                        profile_pending.clear();
                        catalog = Catalog::open(&catalog_path(&paths, Some(&value.account)))?;
                        account_epoch += 1;
                        let prefs: Preferences = catalog.state("preferences")?;
                        player.send(PlayerCommand::Preferences(prefs.clone()));
                        restore_queue(&catalog, &player, value.account.namespace())?;
                        events.send(Event::Ready(Some(value.account.clone()), prefs))?;
                        slskd = None;
                        watched.clear();
                        open_artist = None;
                        let _ = tx.send(Command::ResumeDownloads);
                    }
                    paths.save_account(&value.account)?;
                    native_editor.session(session);
                    account = Some(value.account.clone());
                    checking = false;
                    last_check = Instant::now();
                    set_connection(&mut connection, Connection::Online, &events);
                    player.send(PlayerCommand::Server(value.clone()));
                    server = Some(value.clone());
                    artwork_queue.retry_failed(artwork_clock.now());
                    events.send(Event::Connected(value.account.clone(), label))?;
                    events.send(Event::NativeAvailable(value.is_navidrome))?;
                    events.send(Event::View(current_request, catalog.view(&current_query)?))?;
                    if !syncing {
                        syncing = true;
                        generation = chrono::Utc::now().timestamp_micros();
                        native_renewal.start(&value, &paths, &runtime, &tx);
                        begin_sync(value, tx.clone(), events.clone(), &runtime, generation);
                    }
                }
                Command::Refresh => {
                    catalog.invalidate_artist_profiles()?;
                    artwork_queue.retry_failed(artwork_clock.now());
                    events.send(Event::ArtworkRetrying)?;
                    if let Some(server) = server.clone() {
                        if !syncing {
                            syncing = true;
                            generation = chrono::Utc::now().timestamp_micros();
                            native_renewal.start(&server, &paths, &runtime, &tx);
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
                        let refresh = catalog
                            .cached_playlist_summaries()?
                            .into_iter()
                            .filter(|cached| {
                                playlists.iter().any(|playlist| playlist.id == cached.id)
                                    && crate::playlists::refresh_due(
                                        cached,
                                        account.as_ref().map(|account| account.username.as_str()),
                                        chrono::Utc::now(),
                                    )
                            })
                            .map(|playlist| playlist.id)
                            .collect::<Vec<_>>();
                        if can_delete {
                            catalog.finish_sync(generation)?;
                        }
                        catalog.save_playlist_summaries(&playlists, can_delete)?;
                        if can_delete {
                            let (mutations, rescanned) = reconcile_tags(&catalog)?;
                            for mutation in mutations {
                                tx.send(Command::Mutate(mutation))?;
                            }
                            for (old, track) in rescanned {
                                player.send(if old == track.id {
                                    PlayerCommand::UpdateTrack(track.clone())
                                } else {
                                    PlayerCommand::RetagTrack(old, track.clone())
                                });
                                events.send(Event::TrackUpdated(track))?;
                            }
                        }
                        syncing = false;
                        events.send(Event::Playlists(catalog.playlist_summaries()?))?;
                        for id in refresh {
                            tx.send(Command::Playlist(id))?;
                        }
                        events.send(Event::View(current_request, catalog.view(&current_query)?))?;
                        events.send(Event::Status(if can_delete {
                            format!(
                                "Up to date · {} tracks · {} albums",
                                catalog.count()?,
                                catalog.album_count()?
                            )
                        } else {
                            "Server scan in progress · refresh again when complete".into()
                        }))?;
                        // A refresh can turn a missing release into an owned one.
                        let mut finished = catalog
                            .state::<Vec<DownloadJob>>(DOWNLOADS)?
                            .into_iter()
                            .filter(|job| job.state != JobState::Active)
                            .map(|job| job.artist)
                            .collect::<Vec<_>>();
                        finished.sort();
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
                Command::Discover(request) => {
                    discover_request = request;
                    let user = catalog
                        .state::<Preferences>("preferences")?
                        .lastfm_user
                        .trim()
                        .to_string();
                    let namespace = account.as_ref().map(Account::namespace);
                    if let Some(namespace) = namespace
                        && !user.is_empty()
                        && lastfm.has_key()
                        && !lastfm_fetches.contains_key(&(namespace.clone(), user.clone()))
                    {
                        let history = load_history(&catalog, &user)?;
                        if history.registered.is_none()
                            || !crate::discover::stale_seasons(&history, chrono::Utc::now())
                                .is_empty()
                        {
                            lastfm_failure = None;
                            lastfm_fetches.insert(
                                (namespace.clone(), user.clone()),
                                LastFmFetch { done: 0, total: 0 },
                            );
                            let tx = tx.clone();
                            let lastfm = lastfm.clone();
                            runtime.spawn(fetch_lastfm(lastfm, tx, namespace, user, history));
                        }
                    }
                    publish_discover(
                        &catalog,
                        &events,
                        &lastfm,
                        request,
                        &lastfm_fetches,
                        account.as_ref(),
                        lastfm_failure.as_ref(),
                    )?;
                }
                Command::LastFmRegistered(namespace, user, registered) => {
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        let mut history = load_history(&catalog, &user)?;
                        history.registered = Some(registered);
                        catalog.set_state(&lastfm_state(&user), &Some(&history))?;
                    }
                }
                Command::LastFmSeason(namespace, user, season, chart, done, total) => {
                    if account.as_ref().map(Account::namespace).as_ref() == Some(&namespace) {
                        let mut history = load_history(&catalog, &user)?;
                        history.seasons.insert(season, chart);
                        catalog.set_state(&lastfm_state(&user), &Some(&history))?;
                        if let Some(fetch) = lastfm_fetches.get_mut(&(namespace, user)) {
                            fetch.done = done;
                            fetch.total = total;
                        }
                        publish_discover(
                            &catalog,
                            &events,
                            &lastfm,
                            discover_request,
                            &lastfm_fetches,
                            account.as_ref(),
                            lastfm_failure.as_ref(),
                        )?;
                    }
                }
                Command::LastFmDone(namespace, user, result) => {
                    lastfm_fetches.remove(&(namespace.clone(), user.clone()));
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        if let Err(message) = result {
                            lastfm_failure = Some((user, message));
                        }
                        publish_discover(
                            &catalog,
                            &events,
                            &lastfm,
                            discover_request,
                            &lastfm_fetches,
                            account.as_ref(),
                            lastfm_failure.as_ref(),
                        )?;
                    }
                }
                Command::HideDiscover(id, dismissal) => {
                    let mut hidden: crate::discover::Hidden = catalog
                        .state::<Option<_>>("discover-hidden")?
                        .unwrap_or_default();
                    hidden.hide(&id, dismissal, chrono::Utc::now());
                    catalog.set_state("discover-hidden", &hidden)?;
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
                    let (known_artist, overview) = cached
                        .as_ref()
                        .map(|known| (known.artist.clone(), known.overview))
                        .unwrap_or_default();
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
                                        artist = known_artist.clone();
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
                                    let overview = overview && artist == known_artist;
                                    musicbrainz.discography(&artist, overview).await
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
                Command::LastFmKey(key) => {
                    let key = key.trim().to_string();
                    if !key.is_empty() {
                        match paths.save_lastfm_key(&key) {
                            Ok(()) => {
                                lastfm.set_key(key);
                                lastfm_failure = None;
                            }
                            Err(error) => {
                                let user = catalog
                                    .state::<Preferences>("preferences")?
                                    .lastfm_user
                                    .trim()
                                    .to_string();
                                lastfm_failure = Some((user, format!("{error:#}")));
                            }
                        }
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
                    let namespace = account.as_ref().map(Account::namespace).unwrap_or_default();
                    let DownloadRequest {
                        artist,
                        artist_id,
                        release,
                        title,
                        expected,
                        candidate,
                    } = *request;
                    let client = match slskd_client(&mut slskd, &catalog, &paths) {
                        Ok(client) => client,
                        Err(error) => {
                            events
                                .send(Event::DownloadNotStarted(release, format!("{error:#}")))?;
                            return Ok(());
                        }
                    };
                    if !watched.insert(release.clone()) {
                        events.send(Event::DownloadNotStarted(
                            release,
                            "This release is already downloading.".into(),
                        ))?;
                        return Ok(());
                    }
                    let tx = tx.clone();
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
                            artist_name: artist.clone(),
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
                                let _ = tx.send(Command::DownloadRejected(
                                    namespace,
                                    job.release,
                                    format!(
                                        "{} did not accept the request: {error:#}",
                                        candidate.username
                                    ),
                                ));
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
                Command::DownloadRejected(namespace, release, message) => {
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        watched.remove(&release);
                        let mut jobs: Vec<DownloadJob> = catalog.state(DOWNLOADS)?;
                        jobs.retain(|job| job.release != release);
                        catalog.set_state(DOWNLOADS, &jobs)?;
                        events.send(Event::Downloads(Arc::new(jobs)))?;
                        events.send(Event::DownloadNotStarted(release, message))?;
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
                                    let _ = tx.send(Command::PlaylistLoaded(
                                        server.account.namespace(),
                                        playlist,
                                    ));
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
                Command::PlaylistsLoaded(namespace, playlists) => {
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        catalog.save_playlist_summaries(&playlists, true)?;
                        events.send(Event::Playlists(catalog.playlist_summaries()?))?;
                    }
                }
                Command::PlaylistLoaded(namespace, playlist) => {
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        catalog.save_playlist(&playlist)?;
                        events.send(Event::Playlist(playlist))?;
                        events.send(Event::Playlists(catalog.playlist_summaries()?))?;
                    }
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
                #[cfg(feature = "test-support")]
                Command::ArtworkTime(elapsed, reply) => {
                    artwork_clock.offset += elapsed;
                    let _ = reply.send(artwork_queue.next_retry(artwork_clock.now()));
                }
                Command::Artwork(id, fetch) => {
                    if let Some(account) = &account {
                        let namespace = account.namespace();
                        let path = paths
                            .cache
                            .join(&namespace)
                            .join(format!("{}.art", blake3::hash(id.as_bytes()).to_hex()));
                        if path.exists() {
                            publish_artwork(&runtime, &tx, &events, namespace, id, path);
                            return Ok(());
                        }
                    }
                    if let Some(priority) = fetch {
                        artwork_queue.request(id, artwork_clock.now(), priority);
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
                                artwork_queue.finished(&id, true, artwork_clock.now());
                                publish_artwork(&runtime, &tx, &events, namespace, id, path);
                            }
                            Err(_) => {
                                if artwork_queue.finished(&id, false, artwork_clock.now()) {
                                    events.send(Event::ArtworkFailed(id))?;
                                }
                            }
                        }
                    }
                }
                Command::ArtworkPrepared(namespace, id, path, thumbs) => {
                    if account
                        .as_ref()
                        .is_some_and(|account| account.namespace() == namespace)
                    {
                        events.send(Event::Artwork(id, path, thumbs))?;
                    }
                }
                Command::PlayIds(ids, at) => {
                    let tracks = catalog.by_ids(&ids)?;
                    player.send(PlayerCommand::Replace(tracks, at));
                }
                Command::PlayAlbum(id) => {
                    player.send(PlayerCommand::Replace(catalog.album_tracks(&id)?, 0));
                }
                Command::PlayAlbumMatching(query, id) => {
                    player.send(PlayerCommand::Replace(
                        catalog.album_tracks_matching(&query, &id)?,
                        0,
                    ));
                }
                Command::Enqueue(ids, placement) => {
                    player.send(PlayerCommand::Insert(catalog.by_ids(&ids)?, placement));
                }
                Command::EnqueueAlbum(id, placement) => {
                    player.send(PlayerCommand::Insert(catalog.album_tracks(&id)?, placement));
                }
                Command::EnqueuePlaylist(id, placement) => {
                    if let Some(server) =
                        server.clone().filter(|_| connection == Connection::Online)
                    {
                        let tx = tx.clone();
                        let epoch = account_epoch;
                        runtime.spawn(async move {
                            let playlist = server.playlist(&id).await.ok();
                            let _ = tx.send(Command::PlaylistForQueue(epoch, placement, playlist));
                        });
                    } else {
                        let cached = catalog
                            .playlist(&id)?
                            .filter(|p| !p.entry.is_empty() || p.song_count == 0);
                        enqueue_playlist(cached, placement, &player, &events)?;
                    }
                }
                Command::PlaylistForQueue(epoch, placement, playlist) => {
                    if epoch == account_epoch {
                        enqueue_playlist(playlist, placement, &player, &events)?;
                    }
                }
                Command::AlbumToPlaylist(id, target) => {
                    let ids = catalog
                        .album_tracks(&id)?
                        .iter()
                        .map(|track| track.id.clone())
                        .collect::<Vec<_>>();
                    if ids.is_empty() {
                        events.send(Event::Error(
                            "This album has no songs in the library yet. Refresh and try again."
                                .into(),
                        ))?;
                    } else {
                        tx.send(Command::Mutate(match target {
                            PlaylistTarget::New(name) => Mutation::CreatePlaylist(name, ids),
                            PlaylistTarget::Existing(id) => Mutation::AddToPlaylist(id, ids),
                        }))?;
                    }
                }
                Command::Player(command) => player.send(command),
                Command::WriteTags(edit) => {
                    let report = write_tags(&catalog, &edit)?;
                    finish_tag_job(&report, server.clone(), &runtime, &tx, &events)?;
                }
                Command::UndoTags => {
                    let report = undo_tags(&catalog)?;
                    finish_tag_job(&report, server.clone(), &runtime, &tx, &events)?;
                }
                Command::Preferences(prefs) => {
                    persist_preferences(&catalog, &player, prefs)?;
                }
                Command::Persist(namespace, queue) => {
                    if namespace == account.as_ref().map(Account::namespace).unwrap_or_default() {
                        catalog.set_state("queue", &queue)?;
                    }
                }
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
                    let playlist_edit = matches!(
                        mutation,
                        Mutation::ReorderPlaylist { .. }
                            | Mutation::AddToPlaylist(..)
                            | Mutation::RemoveFromPlaylist(..)
                            | Mutation::DeletePlaylist(..)
                    )
                    .then(|| playlist_track_edits.clone());
                    runtime.spawn(async move {
                        // Don't let another local edit interleave a reorder's read and write.
                        let _playlist_guard = match playlist_edit {
                            Some(lock) => Some(lock.lock_owned().await),
                            None => None,
                        };
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
                            Mutation::ReorderPlaylist {
                                id,
                                original,
                                reordered,
                            } => server.reorder_playlist(id, original, reordered).await,
                            Mutation::RenamePlaylist(id, name) => {
                                server.rename_playlist(id, name).await
                            }
                            Mutation::DeletePlaylist(id) => server.delete_playlist(id).await,
                            Mutation::AddToPlaylist(id, ids) => {
                                server.add_to_playlist(id, ids).await
                            }
                            Mutation::RemoveFromPlaylist(id, index) => {
                                server.remove_from_playlist(id, *index).await
                            }
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
                    // Favorites match album feedback only in album views.
                    let album_scope = current_query
                        .ordering
                        .as_ref()
                        .is_some_and(|sort| sort.scope == SortScope::Albums);
                    let rating_filtered =
                        current_query.favorites && current_query.favorite_filter.min_rating > 0;
                    // Track edits are patched in place by the UI; a fresh view would
                    // reload every album panel, so send one only when the edit can
                    // change which rows appear or their order.
                    let reshapes_view = match &mutation {
                        Mutation::Favorite(..) => current_query.favorites,
                        Mutation::Rating(..) => {
                            rating_filtered
                                || current_query.ordering.as_ref().map_or(
                                    current_query.sort == Sort::Rating,
                                    |s| {
                                        s.scope == SortScope::Tracks
                                            && s.depends_on(SortField::Rating)
                                    },
                                )
                        }
                        Mutation::AlbumFavorite(..) => current_query.favorites && album_scope,
                        Mutation::AlbumRating(..) => {
                            (rating_filtered && album_scope)
                                || current_query.ordering.as_ref().map_or(
                                    current_query.sort == Sort::Rating,
                                    |sort| {
                                        sort.scope == SortScope::Albums
                                            && sort.depends_on(SortField::Rating)
                                    },
                                )
                        }
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
                                    Mutation::ReorderPlaylist { id, .. }
                                    | Mutation::RenamePlaylist(id, _)
                                    | Mutation::AddToPlaylist(id, _) => Some(id),
                                    Mutation::RemoveFromPlaylist(id, _) => Some(id),
                                    _ => None,
                                };
                                runtime.spawn(async move {
                                    let result = async {
                                        let playlists = server.playlists().await?;
                                        let _ = tx.send(Command::PlaylistsLoaded(
                                            server.account.namespace(),
                                            playlists,
                                        ));
                                        if let Some(id) = changed_id {
                                            let playlist = server.playlist(&id).await?;
                                            let _ = tx.send(Command::PlaylistLoaded(
                                                server.account.namespace(),
                                                playlist,
                                            ));
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
                        let _ = tx.send(Command::CheckConnection(true));
                    }
                }
                Command::SaveWindow(frame) => paths.save_window_frame(&frame)?,
                Command::CheckConnection(now) => {
                    if !checking && (now || last_check.elapsed() >= connection.check_interval()) {
                        if let Some(server) = server.clone() {
                            checking = true;
                            last_check = Instant::now();
                            let tx = tx.clone();
                            runtime.spawn(async move {
                                let result =
                                    server.ping().await.map_err(|error| format!("{error:#}"));
                                let _ = tx.send(Command::ConnectionChecked(
                                    server.account.namespace(),
                                    result,
                                ));
                            });
                        } else if let Some(account) = account.clone()
                            && matches!(connection, Connection::Offline(_))
                        {
                            // The first connection failed; connecting again also starts a sync.
                            last_check = Instant::now();
                            checking = reconnect(account, &paths, &runtime, &tx, &events);
                            if !checking {
                                set_connection(&mut connection, Connection::None, &events);
                            }
                        }
                    }
                }
                Command::ConnectionChecked(namespace, result) => {
                    if account.as_ref().map(Account::namespace) == Some(namespace) {
                        checking = false;
                        last_check = Instant::now();
                        let reconnected =
                            matches!(connection, Connection::Offline(_)) && result.is_ok();
                        if reconnected {
                            artwork_queue.retry_failed(artwork_clock.now());
                            events.send(Event::ArtworkRetrying)?;
                            if !syncing && let Some(server) = server.clone() {
                                syncing = true;
                                generation = chrono::Utc::now().timestamp_micros();
                                native_renewal.start(&server, &paths, &runtime, &tx);
                                begin_sync(
                                    server,
                                    tx.clone(),
                                    events.clone(),
                                    &runtime,
                                    generation,
                                );
                            }
                        }
                        set_connection(
                            &mut connection,
                            match result {
                                Ok(()) => Connection::Online,
                                Err(reason) => Connection::Offline(reason),
                            },
                            &events,
                        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discography::ReleaseGroup;

    fn job(release: &str, state: JobState) -> DownloadJob {
        DownloadJob {
            artist: cache_key("ONE OK ROCK", "artist-id"),
            release: release.into(),
            state,
            ..Default::default()
        }
    }

    #[test]
    fn failed_downloads_the_library_now_holds_are_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let mut catalog = Catalog::open(&dir.path().join("library.sqlite")).unwrap();
        let key = cache_key("ONE OK ROCK", "artist-id");
        let group = |id: &str, title: &str| ReleaseGroup {
            id: id.into(),
            title: title.into(),
            primary: "Album".into(),
            date: "2010-01-01".into(),
            official: true,
            ..Default::default()
        };
        catalog
            .set_state(
                &format!("discography:v1:{key}"),
                &Discography {
                    artist: "artist-id".into(),
                    groups: vec![
                        group("owned-failed", "Skyfall"),
                        group("missing-failed", "Luxury Disease"),
                        group("owned-active", "Eye of the Storm"),
                    ],
                    fetched_ms: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        // Most files of each owned release reached the library.
        let album = |name: &str| Album {
            id: name.into(),
            name: name.into(),
            artist: "ONE OK ROCK".into(),
            artist_id: "artist-id".into(),
            ..Default::default()
        };
        catalog
            .put_albums(&[album("Skyfall"), album("Eye of the Storm")], 1)
            .unwrap();
        catalog
            .set_state(
                DOWNLOADS,
                &vec![
                    job("owned-failed", JobState::Failed),
                    job("missing-failed", JobState::Failed),
                    job("owned-active", JobState::Active),
                ],
            )
            .unwrap();
        let (events, received) = flume::unbounded();
        publish_discography(&catalog, &events, "one ok rock", "artist-id", false).unwrap();
        let kept: Vec<DownloadJob> = catalog.state(DOWNLOADS).unwrap();
        let kept: Vec<_> = kept.iter().map(|job| job.release.as_str()).collect();
        // A failed release still absent stays to pick another folder; a running one stays.
        assert_eq!(kept, ["missing-failed", "owned-active"]);
        assert!(
            received
                .try_iter()
                .any(|event| matches!(event, Event::Downloads(jobs) if jobs.len() == 2))
        );
    }
}
