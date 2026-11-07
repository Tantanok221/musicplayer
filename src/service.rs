use crate::{
    artwork::ArtworkQueue,
    catalog::{Catalog, LibraryView},
    lyrics::Lyrics,
    model::*,
    platform::{Account, Paths},
    playback::{PlayerCommand, PlayerEvent, PlayerHandle},
    server::Server,
};
use anyhow::Result;
use flume::{Receiver, Sender};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const SIGN_IN_NOTICE: &str =
    "Sign in once to save your login locally. Sonora no longer uses Keychain.";

#[derive(Clone)]
pub enum Mutation {
    Favorite(String, bool),
    Rating(String, u8),
    CreatePlaylist(String, Vec<String>),
    ReplacePlaylist(String, Vec<String>),
    RenamePlaylist(String, String),
    DeletePlaylist(String),
}
#[derive(Clone)]
pub enum Command {
    Connect(Account, String),
    Refresh,
    Initialize,
    Query(u64, LibraryQuery),
    Page(u64, LibraryQuery, usize),
    Album(String),
    Artist(String, String),
    GroupTracks(u64, LibraryQuery, String),
    PlayTrack(String),
    Playlist(String),
    Artwork(String),
    ArtworkFinished(String, String, Result<PathBuf, String>),
    Lyrics(Arc<Track>),
    LyricsLoaded(String, String, Result<Lyrics, String>),
    PlayIds(Vec<String>, usize),
    PlayAlbum(String),
    Enqueue(Vec<String>, bool),
    Mutate(Mutation),
    Preferences(Preferences),
    Player(PlayerCommand),
    Persist(SavedQueue),
    Scrobble(String, bool),
    Connected(Server, String),
    Tracks(i64, Vec<Track>),
    Albums(i64, Vec<Album>),
    Synced(i64, Vec<Playlist>, bool),
    Updated(Mutation),
    PlaylistLoaded(Playlist),
    PlaylistsLoaded(Vec<Playlist>),
    Error(String),
    SyncFailed(i64, String),
    Shutdown(Sender<()>),
}
#[derive(Clone, Debug)]
pub enum Event {
    Ready(Option<Account>, Preferences),
    SignInRequired(String),
    Connected(Account, String),
    View(u64, LibraryView),
    Page(u64, usize, Vec<Arc<Track>>),
    Album(String, Vec<Arc<Track>>),
    Artist(String, Vec<Arc<Album>>),
    GroupTracks(u64, String, Vec<Arc<Track>>),
    Playlists(Vec<Playlist>),
    Playlist(Playlist),
    Artwork(String, PathBuf),
    ArtworkFailed(String),
    ArtworkRetrying,
    Lyrics(String, Result<Arc<Lyrics>, String>),
    Playback(PlaybackSnapshot),
    Status(String),
    Error(String),
    MutationDone,
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
                        PlayerEvent::Scrobble(id, submission) => {
                            let _ = command_tx.send(Command::Scrobble(id, submission));
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
            let mut playlists=server.playlists().await?;
            for playlist in &mut playlists{*playlist=server.playlist(&playlist.id).await?;}
            let scanning_after=server.scan_status().await?;
            Ok((playlists,!scanning_before&&!scanning_after))
        }.await;
        match result{Ok((playlists,can_delete))=>{let _=tx.send_async(Command::Synced(generation,playlists,can_delete)).await;}
            Err(error)=>{let _=tx.send_async(Command::SyncFailed(generation,format!("Refresh incomplete: {error:#}. Cached music is still available."))).await;}}
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
    ));
    events.send(Event::Ready(account.clone(), prefs))?;
    events.send(Event::Playlists(catalog.playlists()?))?;
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
        let command = match rx.recv_timeout(Duration::from_millis(200)) {
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
                    events.send(Event::Playlists(catalog.playlists()?))?;
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
                            let mut server = Server::new(account, password)?;
                            let label = server.connect().await?;
                            paths.save_credentials(&server.account, server.credentials())?;
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
                        catalog = Catalog::open(&catalog_path(&paths, Some(&value.account)))?;
                        let prefs: Preferences = catalog.state("preferences")?;
                        player.send(PlayerCommand::Preferences(prefs.clone()));
                        let queue: SavedQueue = catalog.state("queue")?;
                        player.send(PlayerCommand::Restore(
                            catalog.by_ids(&queue.ids)?,
                            queue.index.unwrap_or(0),
                            queue.position,
                        ));
                        events.send(Event::Ready(Some(value.account.clone()), prefs))?;
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
                        catalog.save_playlists(&playlists)?;
                        syncing = false;
                        events.send(Event::Playlists(playlists))?;
                        events.send(Event::View(current_request, catalog.view(&current_query)?))?;
                        events.send(Event::Status(if can_delete {
                            format!("Up to date · {} tracks", catalog.count()?)
                        } else {
                            "Server scan in progress · refresh again when complete".into()
                        }))?;
                    }
                }
                Command::Query(request, query) => {
                    if request == query_epoch.load(Ordering::Acquire) {
                        current_query = query;
                        current_request = request;
                        events.send(Event::View(request, catalog.view(&current_query)?))?;
                    }
                }
                Command::Page(request, query, offset) => {
                    if request == query_epoch.load(Ordering::Acquire) {
                        events.send(Event::Page(
                            request,
                            offset,
                            catalog.tracks(&query, offset, 500)?,
                        ))?;
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
                Command::Artist(name, id) => {
                    events.send(Event::Artist(
                        name.clone(),
                        catalog.artist_releases(&name, &id)?,
                    ))?;
                }
                Command::Playlist(id) => {
                    if let Some(playlist) = catalog.playlists()?.into_iter().find(|p| p.id == id) {
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
                    catalog.save_playlists(&playlists)?;
                    events.send(Event::Playlists(playlists))?;
                }
                Command::PlaylistLoaded(playlist) => {
                    let mut playlists = catalog.playlists()?;
                    if let Some(existing) = playlists.iter_mut().find(|p| p.id == playlist.id) {
                        *existing = playlist.clone();
                    } else {
                        playlists.push(playlist.clone());
                    }
                    catalog.save_playlists(&playlists)?;
                    events.send(Event::Playlist(playlist))?;
                    events.send(Event::Playlists(playlists))?;
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
                Command::Artwork(id) => {
                    if let Some(account) = &account {
                        let path = paths
                            .cache
                            .join(account.namespace())
                            .join(format!("{}.art", blake3::hash(id.as_bytes()).to_hex()));
                        if path.exists() {
                            events.send(Event::Artwork(id.clone(), path))?;
                            return Ok(());
                        }
                    }
                    artwork_queue.request(id, Instant::now());
                }
                Command::ArtworkFinished(namespace, id, result) => {
                    if account
                        .as_ref()
                        .is_some_and(|account| account.namespace() == namespace)
                    {
                        match result {
                            Ok(path) => {
                                artwork_queue.finished(&id, true, Instant::now());
                                events.send(Event::Artwork(id, path))?;
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
                Command::Enqueue(ids, next) => {
                    let tracks = catalog.by_ids(&ids)?;
                    player.send(if next {
                        PlayerCommand::PlayNext(tracks)
                    } else {
                        PlayerCommand::Append(tracks)
                    });
                }
                Command::Player(command) => player.send(command),
                Command::Preferences(prefs) => {
                    catalog.set_state("preferences", &prefs)?;
                    player.send(PlayerCommand::Preferences(prefs));
                }
                Command::Persist(queue) => catalog.set_state("queue", &queue)?,
                Command::Scrobble(id, submission) => {
                    if let Some(server) = server.clone() {
                        let events = events.clone();
                        runtime.spawn(async move {
                            if let Err(error) = server.scrobble(&id, submission).await {
                                let _ = events.send(Event::Error(format!(
                                    "Playback history was not saved: {error:#}"
                                )));
                            }
                        });
                    }
                }
                Command::Mutate(mutation) => {
                    let server = server.clone().ok_or_else(|| {
                        anyhow::anyhow!("Connect to the server before saving changes")
                    })?;
                    let tx = tx.clone();
                    runtime.spawn(async move {
                        let result = match &mutation {
                            Mutation::Favorite(id, value) => server.star(id, *value).await,
                            Mutation::Rating(id, rating) => server.rating(id, *rating).await,
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
                                let _ = tx.send(Command::Updated(mutation));
                            }
                            Err(error) => {
                                let _ = tx.send(Command::Error(format!(
                                    "Change was not saved: {error:#}. Retry when connected."
                                )));
                            }
                        }
                    });
                }
                Command::Updated(mutation) => {
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
                        _ => {
                            if let Some(server) = server.clone() {
                                let tx = tx.clone();
                                runtime.spawn(async move{match server.playlists().await{Ok(mut playlists)=>{let mut error=None;for playlist in &mut playlists{match server.playlist(&playlist.id).await{Ok(detail)=>*playlist=detail,Err(e)=>{error=Some(e);break;}}}if let Some(error)=error{let _=tx.send(Command::Error(format!("Saved, but could not refresh playlists: {error:#}")));}else{let _=tx.send(Command::PlaylistsLoaded(playlists));}}Err(error)=>{let _=tx.send(Command::Error(format!("Saved, but could not refresh playlists: {error:#}")));}}});
                            }
                        }
                    }
                    events.send(Event::MutationDone)?;
                    events.send(Event::View(current_request, catalog.view(&current_query)?))?;
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
