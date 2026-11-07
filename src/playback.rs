use crate::{model::*, server::Server};
use anyhow::Result;
use flume::{Receiver, Sender};
use rand::seq::SliceRandom;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, Source};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use stream_download::{Settings, StreamDownload, storage::temp::TempStorageProvider};

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const CHUNK: usize = 2048;

type HttpDecoder = Decoder<StreamDownload<TempStorageProvider>>;

#[derive(Clone)]
pub enum PlayerCommand {
    Server(Server),
    Replace(Vec<Arc<Track>>, usize),
    Restore(Vec<Arc<Track>>, usize, f64),
    Append(Vec<Arc<Track>>),
    PlayNext(Vec<Arc<Track>>),
    Remove(usize),
    Move(usize, usize),
    Toggle,
    Play,
    Pause,
    Next,
    JumpTo(usize),
    Previous,
    Seek(f64),
    Preferences(Preferences),
    UpdateTrack(Arc<Track>),
    SaveQueue(Sender<SavedQueue>),
    Retry,
    ClearError,
    Shutdown,
}
#[derive(Clone, Debug)]
pub enum PlayerEvent {
    Snapshot(PlaybackSnapshot),
    Persist(SavedQueue),
    Scrobble(String, bool),
    Error(String),
}

#[derive(Clone)]
pub struct PlayerHandle {
    tx: Sender<PlayerCommand>,
}
impl PlayerHandle {
    pub fn send(&self, command: PlayerCommand) {
        let _ = self.tx.send(command);
    }
    pub fn start(runtime: tokio::runtime::Handle) -> (Self, Receiver<PlayerEvent>) {
        let (tx, rx) = flume::unbounded();
        let (events, event_rx) = flume::unbounded();
        std::thread::Builder::new()
            .name("sonora-player".into())
            .spawn(move || {
                if let Err(error) = run(rx, events.clone(), runtime) {
                    let _ =
                        events.send(PlayerEvent::Error(format!("Audio unavailable: {error:#}")));
                }
            })
            .expect("Could not start player thread");
        (Self { tx }, event_rx)
    }
}

struct Clock {
    epoch: AtomicU64,
    active: AtomicUsize,
    boundary: AtomicU64,
    samples: AtomicU64,
    position_base: AtomicU64,
    playing: AtomicBool,
    buffering: AtomicBool,
    finished: AtomicBool,
    volume: AtomicU64,
}
impl Default for Clock {
    fn default() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            active: AtomicUsize::new(usize::MAX),
            boundary: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            position_base: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            buffering: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            volume: AtomicU64::new(0.7f64.to_bits()),
        }
    }
}
struct Packet {
    epoch: u64,
    index: usize,
    position: Option<f64>,
    samples: Vec<f32>,
    end: bool,
}
struct BufferedAudio {
    clock: Arc<Clock>,
    rx: Receiver<Packet>,
    packet: Option<Packet>,
    offset: usize,
}
impl Iterator for BufferedAudio {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if !self.clock.playing.load(Ordering::Relaxed) {
            return Some(0.);
        }
        let epoch = self.clock.epoch.load(Ordering::Acquire);
        if self.packet.as_ref().is_some_and(|p| p.epoch != epoch) {
            self.packet = None;
            self.offset = 0;
        }
        if self
            .packet
            .as_ref()
            .is_none_or(|p| self.offset >= p.samples.len())
        {
            self.packet = None;
            while let Ok(packet) = self.rx.try_recv() {
                if packet.epoch != epoch {
                    continue;
                }
                if packet.end {
                    self.clock.finished.store(true, Ordering::Release);
                    return Some(0.);
                }
                if let Some(position) = packet.position {
                    self.clock
                        .position_base
                        .store(position.to_bits(), Ordering::Relaxed);
                    self.clock.samples.store(0, Ordering::Relaxed);
                    self.clock.active.store(packet.index, Ordering::Release);
                    self.clock.boundary.fetch_add(1, Ordering::Release);
                }
                self.clock.active.store(packet.index, Ordering::Release);
                self.packet = Some(packet);
                self.offset = 0;
                break;
            }
        }
        if let Some(packet) = &self.packet
            && let Some(sample) = packet.samples.get(self.offset)
        {
            self.offset += 1;
            self.clock.samples.fetch_add(1, Ordering::Relaxed);
            self.clock.buffering.store(false, Ordering::Relaxed);
            let volume = f64::from_bits(self.clock.volume.load(Ordering::Relaxed)) as f32;
            return Some(sample * volume);
        }
        self.clock.buffering.store(true, Ordering::Relaxed);
        Some(0.)
    }
}
impl Source for BufferedAudio {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        CHANNELS.try_into().unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        RATE.try_into().unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub fn replay_gain(gain: Option<&ReplayGain>, mode: &GainMode) -> f32 {
    let Some(gain) = gain else {
        return 1.;
    };
    let (db, peak) = match mode {
        GainMode::Off => return 1.,
        GainMode::Track => (gain.track_gain, gain.track_peak),
        GainMode::Album => {
            if gain.album_gain.is_some() {
                (gain.album_gain, gain.album_peak)
            } else {
                (gain.track_gain, gain.track_peak)
            }
        }
    };
    let mut factor = 10f32.powf(db.filter(|v| v.is_finite()).unwrap_or(0.) / 20.);
    if let Some(peak) = peak.filter(|p| p.is_finite() && *p > 0.) {
        factor = factor.min(1. / peak);
    }
    factor
}

async fn stream(
    server: &Server,
    track: &Track,
    transcode: bool,
) -> Result<StreamDownload<TempStorageProvider>> {
    let mut params = vec![("id", track.id.clone())];
    if transcode {
        params.extend([("format", "mp3".into()), ("maxBitRate", "320".into())]);
    } else {
        params.push(("format", "raw".into()));
    }
    let url = server.url("stream", &params)?;
    StreamDownload::new_http(
        url,
        TempStorageProvider::with_prefix("sonora-audio-"),
        Settings::default()
            .prefetch_bytes(128 * 1024)
            .retry_timeout(Duration::from_secs(15)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Cannot buffer this track. Check the connection and retry."))
}
fn decoder(source: StreamDownload<TempStorageProvider>, suffix: &str) -> Result<HttpDecoder> {
    let mut builder = Decoder::builder()
        .with_seekable(true)
        .with_gapless(true)
        .with_hint(suffix);
    if let Some(len) = source.content_length() {
        builder = builder.with_byte_len(len);
    }
    Ok(builder.with_data(source).build()?)
}

fn send_packet(tx: &Sender<Packet>, mut packet: Packet, clock: &Clock) -> bool {
    loop {
        if clock.epoch.load(Ordering::Acquire) != packet.epoch {
            return false;
        }
        match tx.send_timeout(packet, Duration::from_millis(50)) {
            Ok(()) => return true,
            Err(flume::SendTimeoutError::Timeout(value)) => packet = value,
            Err(_) => return false,
        }
    }
}

struct DecodeRequest {
    queue: Vec<Arc<Track>>,
    index: usize,
    position: f64,
    prefs: Preferences,
    server: Server,
    epoch: u64,
}
fn decode_queue(
    request: DecodeRequest,
    tx: Sender<Packet>,
    clock: Arc<Clock>,
    errors: Sender<(u64, String)>,
    runtime: tokio::runtime::Handle,
) {
    let DecodeRequest {
        queue,
        mut index,
        mut position,
        prefs,
        server,
        epoch,
    } = request;
    // Start the next HTTP buffer while the current decoder still has queued PCM.
    let mut prefetched: Option<
        tokio::task::JoinHandle<Result<StreamDownload<TempStorageProvider>>>,
    > = None;
    loop {
        if clock.epoch.load(Ordering::Acquire) != epoch {
            return;
        }
        let Some(track) = queue.get(index) else {
            break;
        };
        let source_result = if let Some(task) = prefetched.take() {
            runtime
                .block_on(task)
                .map_err(anyhow::Error::from)
                .and_then(|v| v)
        } else {
            runtime.block_on(stream(&server, track, false))
        };
        let decoded = source_result
            .and_then(|source| decoder(source, &track.suffix))
            .or_else(|_| {
                let source = runtime.block_on(stream(&server, track, true))?;
                decoder(source, "mp3")
            });
        let mut decoded = match decoded {
            Ok(d) => d,
            Err(_) => {
                let _=errors.send((epoch,format!("Could not play “{}”. Check the connection or server transcoding and retry.",track.title)));
                return;
            }
        };
        if position > 0.
            && let Err(error) = decoded.try_seek(Duration::from_secs_f64(position))
        {
            let _ = errors.send((epoch, format!("Could not seek this track: {error}")));
            return;
        }
        let next = match prefs.repeat {
            RepeatMode::One => Some(index),
            _ if index + 1 < queue.len() => Some(index + 1),
            RepeatMode::All => Some(0),
            _ => None,
        };
        if let Some(next) = next {
            let server = server.clone();
            let track = queue[next].clone();
            prefetched = Some(runtime.spawn(async move { stream(&server, &track, false).await }));
        }
        let gain = replay_gain(track.replay_gain.as_ref(), &prefs.gain);
        let mut pcm = rodio::source::UniformSourceIterator::new(
            decoded,
            CHANNELS.try_into().unwrap(),
            RATE.try_into().unwrap(),
        );
        let mut first = true;
        loop {
            if clock.epoch.load(Ordering::Acquire) != epoch {
                if let Some(task) = prefetched {
                    task.abort();
                }
                return;
            }
            let samples = pcm
                .by_ref()
                .take(CHUNK)
                .map(|v| v * gain)
                .collect::<Vec<_>>();
            if samples.is_empty() {
                break;
            }
            if !send_packet(
                &tx,
                Packet {
                    epoch,
                    index,
                    position: first.then_some(position),
                    samples,
                    end: false,
                },
                &clock,
            ) {
                return;
            }
            first = false;
        }
        position = 0.;
        if let Some(next) = next {
            index = next;
        } else {
            break;
        }
    }
    send_packet(
        &tx,
        Packet {
            epoch,
            index,
            position: None,
            samples: vec![],
            end: true,
        },
        &clock,
    );
}

fn default_output_id() -> Option<String> {
    rodio::cpal::default_host()
        .default_output_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string())
}
fn open_output(
    clock: Arc<Clock>,
    rx: Receiver<Packet>,
    errors: Sender<()>,
) -> Result<rodio::MixerDeviceSink> {
    let mut output = rodio::DeviceSinkBuilder::from_default_device()?
        .with_error_callback(move |_| {
            let _ = errors.send(());
        })
        .open_sink_or_fallback()?;
    output.log_on_drop(false);
    output.mixer().add(BufferedAudio {
        clock,
        rx,
        packet: None,
        offset: 0,
    });
    Ok(output)
}

fn run(
    commands: Receiver<PlayerCommand>,
    events: Sender<PlayerEvent>,
    runtime: tokio::runtime::Handle,
) -> Result<()> {
    let clock = Arc::new(Clock::default());
    let (pcm_tx, pcm_rx) = flume::bounded(96);
    let (error_tx, error_rx) = flume::unbounded();
    let (device_errors, device_error_rx) = flume::unbounded();
    let mut output = open_output(clock.clone(), pcm_rx.clone(), device_errors.clone()).ok();
    let mut output_id = default_output_id();
    let mut last_device_check = Instant::now();
    let mut queue: Vec<Arc<Track>> = Vec::new();
    let mut index = None;
    let mut base_position = 0.;
    let mut server: Option<Server> = None;
    let mut prefs = Preferences::default();
    let mut error = if output.is_none() {
        Some("No audio output available. Connect a device and retry.".into())
    } else {
        None
    };
    let mut last_snapshot = Instant::now();
    let mut last_persist = Instant::now();
    let mut last_boundary = 0;
    let mut last_boundary_epoch = 0;
    let mut last_track_id = String::new();
    let mut listened = 0.;
    let mut submitted = false;
    let mut last_tick = Instant::now();
    let mut started = false;
    let mut media = souvlaki::MediaControls::new(souvlaki::PlatformConfig {
        dbus_name: "sonora",
        display_name: "Sonora",
        hwnd: None,
    })
    .ok();
    // OS callbacks enter the same command queue as UI actions.
    let (media_tx, media_rx) = flume::unbounded();
    if let Some(controls) = media.as_mut() {
        let _ = controls.attach(move |event| {
            let _ = media_tx.send(event);
        });
    }
    let position = |clock: &Clock| {
        f64::from_bits(clock.position_base.load(Ordering::Relaxed))
            + clock.samples.load(Ordering::Relaxed) as f64 / (RATE as f64 * CHANNELS as f64)
    };
    loop {
        let mut batch = VecDeque::new();
        match commands.recv_timeout(Duration::from_millis(40)) {
            Ok(command) => batch.push_back(command),
            Err(flume::RecvTimeoutError::Disconnected) => break,
            Err(_) => {}
        }
        batch.extend(commands.try_iter());
        for event in media_rx.try_iter() {
            use souvlaki::MediaControlEvent as M;
            match event {
                M::Play => batch.push_back(PlayerCommand::Play),
                M::Pause => batch.push_back(PlayerCommand::Pause),
                M::Toggle => batch.push_back(PlayerCommand::Toggle),
                M::Next => batch.push_back(PlayerCommand::Next),
                M::Previous => batch.push_back(PlayerCommand::Previous),
                M::SetPosition(p) => batch.push_back(PlayerCommand::Seek(p.0.as_secs_f64())),
                _ => {}
            }
        }
        let elapsed = last_tick.elapsed().as_secs_f64();
        last_tick = Instant::now();
        if last_device_check.elapsed() >= Duration::from_secs(1) {
            let device = default_output_id();
            let failed = device_error_rx.try_iter().next().is_some();
            if device != output_id || failed || output.is_none() || elapsed > 3. {
                base_position = if started {
                    position(&clock)
                } else {
                    base_position
                };
                let was_playing = clock.playing.swap(false, Ordering::AcqRel);
                clock.epoch.fetch_add(1, Ordering::AcqRel);
                started = false;
                drop(output.take());
                output = open_output(clock.clone(), pcm_rx.clone(), device_errors.clone()).ok();
                output_id = device;
                if output.is_none() {
                    error = Some("Audio output disconnected. Connect a device and retry.".into());
                } else if was_playing {
                    error = Some(
                        "Audio output changed or Mac woke from sleep. Press Play to resume.".into(),
                    );
                }
            }
            last_device_check = Instant::now();
        }
        if clock.playing.load(Ordering::Relaxed) && !clock.buffering.load(Ordering::Relaxed) {
            listened += elapsed.min(0.25);
        }
        let active = clock.active.load(Ordering::Acquire);
        let boundary = clock.boundary.load(Ordering::Acquire);
        if active != usize::MAX && boundary != last_boundary {
            index = Some(active);
            last_boundary = boundary;
            if let Some(track) = queue.get(active) {
                let current_epoch = clock.epoch.load(Ordering::Acquire);
                let new_play = track.id != last_track_id || current_epoch == last_boundary_epoch;
                last_boundary_epoch = current_epoch;
                last_track_id = track.id.clone();
                if new_play {
                    listened = 0.;
                    submitted = false;
                    let _ = events.send(PlayerEvent::Scrobble(track.id.clone(), false));
                }
                if let Some(media) = media.as_mut() {
                    let _ = media.set_metadata(souvlaki::MediaMetadata {
                        title: Some(&track.title),
                        artist: Some(track.artist_label()),
                        album: Some(&track.album),
                        duration: Some(Duration::from_secs_f64(track.duration.max(0.))),
                        ..Default::default()
                    });
                }
            }
        }
        if let Some(track) = index.and_then(|i| queue.get(i))
            && !submitted
            && track.duration >= 30.
            && listened >= (track.duration / 2.).min(240.)
        {
            submitted = true;
            let _ = events.send(PlayerEvent::Scrobble(track.id.clone(), true));
        }
        let mut restart = false;
        for command in batch {
            match command {
                PlayerCommand::Shutdown => {
                    clock.epoch.fetch_add(1, Ordering::AcqRel);
                    let _ = events.send(PlayerEvent::Persist(SavedQueue {
                        ids: queue.iter().map(|t| t.id.clone()).collect(),
                        index,
                        position: if started {
                            position(&clock)
                        } else {
                            base_position
                        },
                    }));
                    return Ok(());
                }
                PlayerCommand::Server(value) => server = Some(value),
                PlayerCommand::UpdateTrack(track) => {
                    for queued in &mut queue {
                        if queued.id == track.id {
                            *queued = track.clone();
                        }
                    }
                }
                PlayerCommand::SaveQueue(reply) => {
                    let _ = reply.send(SavedQueue {
                        ids: queue.iter().map(|t| t.id.clone()).collect(),
                        index,
                        position: if started {
                            position(&clock)
                        } else {
                            base_position
                        },
                    });
                }
                PlayerCommand::Replace(mut tracks, at) => {
                    let at = at.min(tracks.len().saturating_sub(1));
                    if prefs.shuffle && !tracks.is_empty() {
                        let current = tracks.remove(at);
                        tracks.shuffle(&mut rand::rng());
                        tracks.insert(0, current);
                        index = Some(0);
                    } else {
                        index = (!tracks.is_empty()).then_some(at);
                    }
                    queue = tracks;
                    base_position = 0.;
                    last_track_id.clear();
                    clock.playing.store(true, Ordering::Relaxed);
                    restart = true;
                }
                PlayerCommand::Restore(tracks, at, pos) => {
                    queue = tracks;
                    index = (!queue.is_empty()).then_some(at.min(queue.len().saturating_sub(1)));
                    base_position = pos;
                    clock.playing.store(false, Ordering::Relaxed);
                    restart = false;
                    started = false;
                }
                PlayerCommand::Append(tracks) => {
                    base_position = if started {
                        position(&clock)
                    } else {
                        base_position
                    };
                    queue.extend(tracks);
                    restart = started;
                }
                PlayerCommand::PlayNext(tracks) => {
                    base_position = if started {
                        position(&clock)
                    } else {
                        base_position
                    };
                    let at = index.map_or(0, |i| i + 1);
                    queue.splice(at..at, tracks);
                    restart = started;
                }
                PlayerCommand::Remove(at) => {
                    if at < queue.len() {
                        base_position = if started {
                            position(&clock)
                        } else {
                            base_position
                        };
                        queue.remove(at);
                        if let Some(i) = index {
                            if at < i {
                                index = Some(i - 1)
                            } else if at == i {
                                index = (!queue.is_empty())
                                    .then_some(i.min(queue.len().saturating_sub(1)));
                                base_position = 0.;
                            }
                        }
                        restart = true;
                    }
                }
                PlayerCommand::Move(from, to) => {
                    if from < queue.len() && to < queue.len() {
                        let current = index.and_then(|i| queue.get(i)).map(|t| t.id.clone());
                        base_position = if started {
                            position(&clock)
                        } else {
                            base_position
                        };
                        let track = queue.remove(from);
                        queue.insert(to, track);
                        index = current.and_then(|id| queue.iter().position(|t| t.id == id));
                        restart = started;
                    }
                }
                PlayerCommand::Toggle => {
                    let play = !clock.playing.load(Ordering::Relaxed);
                    clock.playing.store(play, Ordering::Relaxed);
                    restart = play && !started;
                }
                PlayerCommand::Play => {
                    clock.playing.store(true, Ordering::Relaxed);
                    restart = !started;
                }
                PlayerCommand::Pause => {
                    clock.playing.store(false, Ordering::Relaxed);
                }
                PlayerCommand::JumpTo(at) => {
                    if at < queue.len() {
                        index = Some(at);
                        base_position = 0.;
                        clock.playing.store(true, Ordering::Relaxed);
                        restart = true;
                    }
                }
                PlayerCommand::Next => {
                    if let Some(i) = index {
                        index = if i + 1 < queue.len() {
                            Some(i + 1)
                        } else if prefs.repeat == RepeatMode::All {
                            Some(0)
                        } else {
                            None
                        };
                        base_position = 0.;
                        restart = true;
                    }
                }
                PlayerCommand::Previous => {
                    if let Some(i) = index {
                        index = Some(if position(&clock) > 3. {
                            i
                        } else {
                            i.saturating_sub(1)
                        });
                        base_position = 0.;
                        restart = true;
                    }
                }
                PlayerCommand::Seek(pos) => {
                    if pos.is_finite() {
                        base_position = pos.clamp(
                            0.,
                            index
                                .and_then(|i| queue.get(i))
                                .map_or(0., |t| t.duration.max(0.)),
                        );
                        restart = true;
                    }
                }
                PlayerCommand::Preferences(value) => {
                    let rebuffer = prefs.gain != value.gain || prefs.repeat != value.repeat;
                    if !prefs.shuffle && value.shuffle {
                        base_position = if started {
                            position(&clock)
                        } else {
                            base_position
                        };
                        if let Some(i) = index {
                            queue[i + 1..].shuffle(&mut rand::rng());
                        }
                        restart = started;
                    }
                    prefs = value;
                    clock.volume.store(
                        (prefs.volume.clamp(0., 1.) as f64).to_bits(),
                        Ordering::Relaxed,
                    );
                    if rebuffer && started {
                        base_position = position(&clock);
                        restart = true;
                    }
                }
                PlayerCommand::ClearError => {
                    error = None;
                }
                PlayerCommand::Retry => {
                    base_position = if started {
                        position(&clock)
                    } else {
                        base_position
                    };
                    clock.playing.store(true, Ordering::Relaxed);
                    restart = true;
                }
            }
        }
        if restart {
            let epoch = clock.epoch.fetch_add(1, Ordering::AcqRel) + 1;
            clock.finished.store(false, Ordering::Relaxed);
            clock.buffering.store(true, Ordering::Relaxed);
            clock.active.store(usize::MAX, Ordering::Release);
            clock.samples.store(0, Ordering::Relaxed);
            clock
                .position_base
                .store(base_position.to_bits(), Ordering::Relaxed);
            if let (Some(at), Some(server), true) = (index, server.clone(), output.is_some()) {
                let request = DecodeRequest {
                    queue: queue.clone(),
                    index: at,
                    position: base_position,
                    prefs: prefs.clone(),
                    server,
                    epoch,
                };
                let tx = pcm_tx.clone();
                let clock = clock.clone();
                let errors = error_tx.clone();
                let runtime = runtime.clone();
                std::thread::Builder::new()
                    .name("sonora-decoder".into())
                    .spawn(move || decode_queue(request, tx, clock, errors, runtime))?;
                started = true;
                error = None;
            } else {
                started = false;
                clock.playing.store(false, Ordering::Relaxed);
                if index.is_some() {
                    error = Some(
                        if output.is_none() {
                            "No audio output available. Connect a device and retry."
                        } else {
                            "Connect to your server to play music."
                        }
                        .into(),
                    );
                }
            }
        }
        for (epoch, message) in error_rx.try_iter() {
            if epoch == clock.epoch.load(Ordering::Acquire) {
                error = Some(message);
                clock.playing.store(false, Ordering::Relaxed);
                clock.buffering.store(false, Ordering::Relaxed);
            }
        }
        if clock.finished.swap(false, Ordering::AcqRel) {
            clock.playing.store(false, Ordering::Relaxed);
            clock.buffering.store(false, Ordering::Relaxed);
            started = false;
            base_position = 0.;
        }
        if last_snapshot.elapsed() >= Duration::from_millis(200) {
            let pos = if started {
                position(&clock)
            } else {
                base_position
            };
            let playing = clock.playing.load(Ordering::Relaxed);
            let track = index.and_then(|i| queue.get(i)).cloned();
            if let Some(media) = media.as_mut() {
                let state = if playing {
                    souvlaki::MediaPlayback::Playing {
                        progress: Some(souvlaki::MediaPosition(Duration::from_secs_f64(
                            pos.max(0.),
                        ))),
                    }
                } else {
                    souvlaki::MediaPlayback::Paused {
                        progress: Some(souvlaki::MediaPosition(Duration::from_secs_f64(
                            pos.max(0.),
                        ))),
                    }
                };
                let _ = media.set_playback(state);
            }
            let _ = events.send(PlayerEvent::Snapshot(PlaybackSnapshot {
                duration: track.as_ref().map_or(0., |t| t.duration),
                track,
                playing,
                buffering: clock.buffering.load(Ordering::Relaxed) && playing,
                position: pos,
                index,
                queue: queue.clone(),
                error: error.clone(),
            }));
            last_snapshot = Instant::now();
        }
        if last_persist.elapsed() >= Duration::from_secs(3) {
            let _ = events.send(PlayerEvent::Persist(SavedQueue {
                ids: queue.iter().map(|t| t.id.clone()).collect(),
                index,
                position: if started {
                    position(&clock)
                } else {
                    base_position
                },
            }));
            last_persist = Instant::now();
        }
        // Reopen the output after a device failure through Retry; source state stays in the actor.
    }
    clock.epoch.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_gain_modes_fallback_and_peak_protection() {
        let gain = ReplayGain {
            track_gain: Some(-6.),
            album_gain: Some(-3.),
            track_peak: Some(0.9),
            album_peak: Some(0.9),
        };
        assert!((replay_gain(Some(&gain), &GainMode::Track) - 0.5011872).abs() < 0.0001);
        assert!((replay_gain(Some(&gain), &GainMode::Album) - 0.7079458).abs() < 0.0001);
        assert_eq!(replay_gain(Some(&gain), &GainMode::Off), 1.);
        let fallback = ReplayGain {
            album_gain: None,
            ..gain.clone()
        };
        assert_eq!(
            replay_gain(Some(&fallback), &GainMode::Album),
            replay_gain(Some(&gain), &GainMode::Track)
        );
        let hot = ReplayGain {
            track_gain: Some(12.),
            track_peak: Some(2.),
            ..Default::default()
        };
        assert_eq!(replay_gain(Some(&hot), &GainMode::Track), 0.5);
        assert_eq!(replay_gain(None, &GainMode::Album), 1.);
    }
    #[test]
    fn buffered_output_has_no_inserted_samples_at_track_boundary() {
        let clock = Arc::new(Clock::default());
        clock.playing.store(true, Ordering::Relaxed);
        clock.volume.store(1f64.to_bits(), Ordering::Relaxed);
        clock.epoch.store(1, Ordering::Release);
        let (tx, rx) = flume::bounded(8);
        tx.send(Packet {
            epoch: 1,
            index: 0,
            position: Some(0.),
            samples: vec![0.1, 0.2, 0.3, 0.4],
            end: false,
        })
        .unwrap();
        tx.send(Packet {
            epoch: 1,
            index: 1,
            position: Some(0.),
            samples: vec![0.5, 0.6, 0.7, 0.8],
            end: false,
        })
        .unwrap();
        let mut source = BufferedAudio {
            clock: clock.clone(),
            rx,
            packet: None,
            offset: 0,
        };
        assert_eq!(
            source.by_ref().take(8).collect::<Vec<_>>(),
            vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]
        );
        assert_eq!(clock.active.load(Ordering::Acquire), 1);
        assert_eq!(clock.boundary.load(Ordering::Acquire), 2);
        assert_eq!(source.next(), Some(0.));
        assert!(clock.buffering.load(Ordering::Relaxed));
        clock.playing.store(false, Ordering::Relaxed);
        let samples = clock.samples.load(Ordering::Relaxed);
        source.next();
        assert_eq!(clock.samples.load(Ordering::Relaxed), samples);
    }
    #[test]
    fn seeking_discards_stale_pcm() {
        let clock = Arc::new(Clock::default());
        clock.playing.store(true, Ordering::Relaxed);
        clock.volume.store(1f64.to_bits(), Ordering::Relaxed);
        clock.epoch.store(2, Ordering::Release);
        let (tx, rx) = flume::bounded(8);
        tx.send(Packet {
            epoch: 1,
            index: 0,
            position: Some(0.),
            samples: vec![0.1; 8],
            end: false,
        })
        .unwrap();
        tx.send(Packet {
            epoch: 2,
            index: 1,
            position: Some(20.),
            samples: vec![0.5; 4],
            end: false,
        })
        .unwrap();
        let mut source = BufferedAudio {
            clock: clock.clone(),
            rx,
            packet: None,
            offset: 0,
        };
        assert_eq!(source.next(), Some(0.5));
        assert_eq!(
            f64::from_bits(clock.position_base.load(Ordering::Relaxed)),
            20.
        );
    }
}
