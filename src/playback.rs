use crate::{
    listening::{AudibleListen, AudioProgress, ListenReport, ListenSession, ListenTracker},
    model::*,
    server::Server,
    visualizer::{WAVEFORM_BARS, Waveform},
};
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

const CHUNK: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AudioFormat {
    rate: rodio::SampleRate,
    channels: rodio::ChannelCount,
}
impl AudioFormat {
    fn of(source: &impl Source) -> Self {
        Self {
            rate: source.sample_rate(),
            channels: source.channels(),
        }
    }
    fn samples_per_second(self) -> u64 {
        u64::from(self.rate.get()) * u64::from(self.channels.get())
    }
    fn chunk_size(self) -> usize {
        let channels = usize::from(self.channels.get());
        CHUNK.max(channels) / channels * channels
    }
}

type HttpDecoder = Decoder<StreamDownload<TempStorageProvider>>;

#[derive(Clone)]
pub enum PlayerCommand {
    Server(Server),
    Replace(Vec<Arc<Track>>, usize),
    Restore(Vec<Arc<Track>>, usize, f64, Option<ListenSession>),
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
    Scrobble(ListenReport),
    Error(String),
}

#[derive(Clone)]
pub struct PlayerHandle {
    tx: Sender<PlayerCommand>,
    clock: Arc<Clock>,
}
impl PlayerHandle {
    pub fn send(&self, command: PlayerCommand) {
        let _ = self.tx.send(command);
    }
    pub fn waveform(&self) -> [f32; WAVEFORM_BARS] {
        if self.clock.playing.load(Ordering::Relaxed)
            && !self.clock.buffering.load(Ordering::Relaxed)
        {
            self.clock.waveform.snapshot()
        } else {
            [0.; WAVEFORM_BARS]
        }
    }
    pub fn start(runtime: tokio::runtime::Handle) -> (Self, Receiver<PlayerEvent>) {
        let (tx, rx) = flume::unbounded();
        let (events, event_rx) = flume::unbounded();
        let clock = Arc::new(Clock::default());
        let playback_clock = clock.clone();
        std::thread::Builder::new()
            .name("sonora-player".into())
            .spawn(move || {
                if let Err(error) = run(rx, events.clone(), runtime, playback_clock) {
                    let _ =
                        events.send(PlayerEvent::Error(format!("Audio unavailable: {error:#}")));
                }
            })
            .expect("Could not start player thread");
        (Self { tx, clock }, event_rx)
    }
}

struct Clock {
    listen_tx: Sender<AudioProgress>,
    listen_rx: Receiver<AudioProgress>,
    epoch: AtomicU64,
    active: AtomicUsize,
    active_listen: AtomicU64,
    boundary: AtomicU64,
    samples: AtomicU64,
    samples_per_second: AtomicU64,
    position_base: AtomicU64,
    playing: AtomicBool,
    buffering: AtomicBool,
    finished: AtomicBool,
    volume: AtomicU64,
    waveform: Waveform,
}
impl Default for Clock {
    fn default() -> Self {
        let (listen_tx, listen_rx) = flume::unbounded();
        Self {
            listen_tx,
            listen_rx,
            epoch: AtomicU64::new(0),
            active: AtomicUsize::new(usize::MAX),
            active_listen: AtomicU64::new(0),
            boundary: AtomicU64::new(0),
            samples: AtomicU64::new(0),
            samples_per_second: AtomicU64::new(1),
            position_base: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            buffering: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            volume: AtomicU64::new(0.7f64.to_bits()),
            waveform: Waveform::default(),
        }
    }
}
impl Clock {
    fn position(&self) -> f64 {
        f64::from_bits(self.position_base.load(Ordering::Relaxed))
            + self.samples.load(Ordering::Relaxed) as f64
                / self.samples_per_second.load(Ordering::Relaxed).max(1) as f64
    }
}
struct Packet {
    listen: Option<Arc<AudibleListen>>,
    epoch: u64,
    index: usize,
    position: Option<f64>,
    format: AudioFormat,
    samples: Vec<f32>,
    end: bool,
}
struct BufferedAudio {
    clock: Arc<Clock>,
    rx: Receiver<Packet>,
    format: AudioFormat,
    format_changes: Sender<Packet>,
    pending: Option<Packet>,
    waiting_format: Option<u64>,
    reported: usize,
    listen_started_ms: i64,
    packet: Option<Packet>,
    offset: usize,
    visual_samples: usize,
    visual_peak: f32,
    visual_epoch: u64,
}
impl BufferedAudio {
    fn new(
        clock: Arc<Clock>,
        rx: Receiver<Packet>,
        format_changes: Sender<Packet>,
        format: AudioFormat,
        first: Option<Packet>,
    ) -> Self {
        Self {
            clock,
            rx,
            format,
            format_changes,
            pending: first,
            waiting_format: None,
            reported: 0,
            listen_started_ms: 0,
            packet: None,
            offset: 0,
            visual_samples: 0,
            visual_peak: 0.,
            visual_epoch: 0,
        }
    }
}
impl BufferedAudio {
    fn report_progress(&mut self) {
        if self.offset > self.reported
            && let Some(packet) = &self.packet
        {
            if let Some(listen) = &packet.listen {
                let _ = self.clock.listen_tx.send(AudioProgress {
                    listen: listen.clone(),
                    seconds: (self.offset - self.reported) as f64
                        / self.format.samples_per_second() as f64,
                    started_ms: self.listen_started_ms,
                    epoch: packet.epoch,
                });
            }
            self.reported = self.offset;
        }
    }
}
impl Drop for BufferedAudio {
    fn drop(&mut self) {
        self.report_progress();
    }
}
fn drain_listens(
    clock: &Clock,
    tracker: &mut ListenTracker,
    current: &mut Option<u64>,
    events: &Sender<PlayerEvent>,
) {
    for progress in clock.listen_rx.try_iter() {
        if progress.epoch == clock.epoch.load(Ordering::Acquire) {
            *current = Some(progress.listen.id);
        }
        for report in tracker.observe(progress) {
            let _ = events.send(PlayerEvent::Scrobble(report));
        }
    }
}
impl Iterator for BufferedAudio {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if !self.clock.playing.load(Ordering::Relaxed) {
            self.report_progress();
            return Some(0.);
        }
        let epoch = self.clock.epoch.load(Ordering::Acquire);
        if self.visual_epoch != epoch {
            self.visual_epoch = epoch;
            self.visual_samples = 0;
            self.visual_peak = 0.;
            self.waiting_format = None;
            self.clock.waveform.reset();
        }
        if self.waiting_format == Some(epoch) {
            return Some(0.);
        }
        if self.packet.as_ref().is_some_and(|p| p.epoch != epoch) {
            self.report_progress();
            self.packet = None;
            self.offset = 0;
        }
        if self
            .packet
            .as_ref()
            .is_none_or(|p| self.offset >= p.samples.len())
        {
            self.report_progress();
            self.packet = None;
            while let Some(packet) = self.pending.take().or_else(|| self.rx.try_recv().ok()) {
                if packet.epoch != epoch {
                    continue;
                }
                if packet.end {
                    self.clock.finished.store(true, Ordering::Release);
                    return Some(0.);
                }
                if packet.format != self.format {
                    // The player actor reopens the output at the next source's format.
                    // Hand off the complete first packet without consuming any samples.
                    self.waiting_format = Some(epoch);
                    self.clock.buffering.store(true, Ordering::Relaxed);
                    let _ = self.format_changes.send(packet);
                    return Some(0.);
                }
                if packet.position.is_some() {
                    self.listen_started_ms = chrono::Utc::now().timestamp_millis();
                    if let Some(listen) = &packet.listen {
                        self.clock.active_listen.store(listen.id, Ordering::Relaxed);
                        let _ = self.clock.listen_tx.send(AudioProgress {
                            listen: listen.clone(),
                            seconds: 0.,
                            started_ms: self.listen_started_ms,
                            epoch: packet.epoch,
                        });
                    }
                }
                if let Some(position) = packet.position {
                    self.clock
                        .position_base
                        .store(position.to_bits(), Ordering::Relaxed);
                    self.clock.samples.store(0, Ordering::Relaxed);
                    self.clock
                        .samples_per_second
                        .store(self.format.samples_per_second(), Ordering::Relaxed);
                    self.clock.active.store(packet.index, Ordering::Release);
                    self.clock.boundary.fetch_add(1, Ordering::Release);
                }
                self.clock.active.store(packet.index, Ordering::Release);
                self.packet = Some(packet);
                self.offset = 0;
                self.reported = 0;
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
            let output = sample * volume;
            self.visual_peak = self.visual_peak.max(output.abs());
            self.visual_samples += 1;
            // Sixteen milliseconds at the source's native rate and channel count.
            if self.visual_samples >= (self.format.samples_per_second() * 16 / 1000) as usize {
                self.clock.waveform.push(self.visual_peak);
                self.visual_samples = 0;
                self.visual_peak = 0.;
            }
            if self.offset == packet.samples.len() {
                self.report_progress();
            }
            return Some(output);
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
        self.format.channels
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.format.rate
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

async fn stream(server: &Server, track: &Track) -> Result<StreamDownload<TempStorageProvider>> {
    let params = [("id", track.id.clone()), ("format", "raw".into())];
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
    listen_id: u64,
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
        mut listen_id,
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
    let mut final_format = None;
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
            runtime.block_on(stream(&server, track))
        };
        let decoded = source_result.and_then(|source| decoder(source, &track.suffix));
        let mut decoded = match decoded {
            Ok(d) => d,
            Err(_) => {
                let _=errors.send((epoch,format!("Could not decode the original audio for “{}”. Check the file format and connection, then retry. No transcoding was applied.",track.title)));
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
            prefetched = Some(runtime.spawn(async move { stream(&server, &track).await }));
        }
        let listen = Arc::new(AudibleListen {
            id: listen_id,
            account: server.account.namespace(),
            track: track.clone(),
        });
        let gain = replay_gain(track.replay_gain.as_ref(), &prefs.gain);
        let format = AudioFormat::of(&decoded);
        final_format = Some(format);
        let mut pcm = decoded;
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
                .take(format.chunk_size())
                .map(|v| v * gain)
                .collect::<Vec<_>>();
            if samples.is_empty() {
                break;
            }
            if !send_packet(
                &tx,
                Packet {
                    listen: Some(listen.clone()),
                    epoch,
                    index,
                    position: first.then_some(position),
                    format,
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
            listen_id = rand::random();
            index = next;
        } else {
            break;
        }
    }
    if let Some(format) = final_format {
        send_packet(
            &tx,
            Packet {
                listen: None,
                epoch,
                index,
                position: None,
                format,
                samples: vec![],
                end: true,
            },
            &clock,
        );
    }
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
    format_changes: Sender<Packet>,
    first: Packet,
) -> Result<rodio::MixerDeviceSink> {
    let format = first.format;
    let mut output = rodio::DeviceSinkBuilder::from_default_device()?
        .with_sample_rate(format.rate)
        .with_channels(format.channels)
        .with_error_callback(move |_| {
            let _ = errors.send(());
        })
        .open_stream()?;
    output.log_on_drop(false);
    output.mixer().add(BufferedAudio::new(
        clock,
        rx,
        format_changes,
        format,
        Some(first),
    ));
    Ok(output)
}

#[derive(PartialEq)]
struct SnapshotStamp {
    playing: bool,
    buffering: bool,
    position: u64,
    index: Option<usize>,
    revision: u64,
    error: Option<String>,
}
fn media_command(event: souvlaki::MediaControlEvent) -> Option<PlayerCommand> {
    use souvlaki::MediaControlEvent as M;
    Some(match event {
        M::Play => PlayerCommand::Play,
        M::Pause => PlayerCommand::Pause,
        M::Toggle => PlayerCommand::Toggle,
        M::Next => PlayerCommand::Next,
        M::Previous => PlayerCommand::Previous,
        M::SetPosition(p) => PlayerCommand::Seek(p.0.as_secs_f64()),
        _ => return None,
    })
}

fn run(
    commands: Receiver<PlayerCommand>,
    events: Sender<PlayerEvent>,
    runtime: tokio::runtime::Handle,
    clock: Arc<Clock>,
) -> Result<()> {
    let (pcm_tx, pcm_rx) = flume::bounded(96);
    let (error_tx, error_rx) = flume::unbounded();
    let (device_errors, device_error_rx) = flume::unbounded();
    let (format_changes, format_rx) = flume::unbounded::<Packet>();
    // Wait for decoded audio before opening a device: metadata can be incomplete.
    let mut output: Option<rodio::MixerDeviceSink> = None;
    let mut output_id = default_output_id();
    let mut last_device_check = Instant::now();
    let mut queue: Vec<Arc<Track>> = Vec::new();
    let mut index = None;
    let mut base_position = 0.;
    let mut server: Option<Server> = None;
    let mut prefs = Preferences::default();
    let mut error = if output_id.is_none() {
        Some("No audio output available. Connect a device and retry.".into())
    } else {
        None
    };
    let mut last_snapshot = Instant::now();
    let mut last_persist = Instant::now();
    let mut last_boundary = 0;
    let mut revision = 0;
    let mut snapshot_stamp = None;
    let mut published_queue = Arc::new(Vec::new());
    let mut queue_dirty = true;
    let mut last_media = Instant::now();
    let mut media_playing = None;
    let mut tracker = ListenTracker::default();
    let mut listen_seed = None;
    let mut last_tick = Instant::now();
    let mut started = false;
    let mut pending_output: Option<(Packet, Instant)> = None;
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
    let position = |clock: &Clock| clock.position();
    loop {
        let mut batch = VecDeque::new();
        // Media keys and UI commands wake immediately. A paused player only checks
        // device changes once per second, instead of spinning 25 times per second.
        let wait = if clock.playing.load(Ordering::Relaxed) || pending_output.is_some() {
            Duration::from_millis(40)
        } else {
            Duration::from_secs(1)
        };
        match flume::Selector::new()
            .recv(&commands, |command| command.map(Some))
            .recv(&media_rx, |event| event.map(media_command))
            .wait_timeout(wait)
        {
            Ok(Ok(Some(command))) => batch.push_back(command),
            Ok(Err(_)) => break,
            _ => {}
        }
        batch.extend(commands.try_iter());
        let persist_dirty = !batch.is_empty();
        let elapsed = last_tick.elapsed().as_secs_f64();
        last_tick = Instant::now();
        if last_device_check.elapsed() >= Duration::from_secs(1) {
            let device = default_output_id();
            let failed = device_error_rx.try_iter().next().is_some();
            if device != output_id || failed || elapsed > 3. {
                base_position = if started {
                    position(&clock)
                } else {
                    base_position
                };
                let was_playing = clock.playing.swap(false, Ordering::AcqRel);
                clock.epoch.fetch_add(1, Ordering::AcqRel);
                started = false;
                drop(output.take());
                output_id = device;
                if output_id.is_none() {
                    error = Some("Audio output disconnected. Connect a device and retry.".into());
                } else if was_playing {
                    error = Some(
                        "Audio output changed or Mac woke from sleep. Press Play to resume.".into(),
                    );
                }
            }
            last_device_check = Instant::now();
        }
        drain_listens(&clock, &mut tracker, &mut listen_seed, &events);
        let active = clock.active.load(Ordering::Acquire);
        let boundary = clock.boundary.load(Ordering::Acquire);
        if active != usize::MAX && boundary != last_boundary {
            index = Some(active);
            let audible_id = clock.active_listen.load(Ordering::Relaxed);
            if audible_id != 0 {
                listen_seed = Some(audible_id);
            }
            last_boundary = boundary;
            if let Some(track) = queue.get(active)
                && let Some(media) = media.as_mut()
            {
                let _ = media.set_metadata(souvlaki::MediaMetadata {
                    title: Some(&track.title),
                    artist: Some(track.artist_label()),
                    album: Some(&track.album),
                    duration: Some(Duration::from_secs_f64(track.duration.max(0.))),
                    ..Default::default()
                });
            }
        }
        let mut restart = false;
        for command in batch {
            revision += 1;
            queue_dirty |= matches!(
                &command,
                PlayerCommand::Replace(..)
                    | PlayerCommand::Restore(..)
                    | PlayerCommand::Append(..)
                    | PlayerCommand::PlayNext(..)
                    | PlayerCommand::Remove(..)
                    | PlayerCommand::Move(..)
                    | PlayerCommand::UpdateTrack(..)
            );
            match command {
                PlayerCommand::Shutdown => {
                    clock.epoch.fetch_add(1, Ordering::AcqRel);
                    drop(output.take());
                    drain_listens(&clock, &mut tracker, &mut listen_seed, &events);
                    let _ = events.send(PlayerEvent::Persist(SavedQueue {
                        listen: tracker.session(listen_seed),
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
                        listen: tracker.session(listen_seed),
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
                    listen_seed = None;
                    clock.playing.store(true, Ordering::Relaxed);
                    restart = true;
                }
                PlayerCommand::Restore(tracks, at, pos, listen) => {
                    clock.epoch.fetch_add(1, Ordering::AcqRel);
                    drop(output.take());
                    pending_output = None;
                    listen_seed = None;
                    if let Some(listen) = listen.filter(|listen| {
                        tracks
                            .get(at)
                            .is_some_and(|track| track.id == listen.track_id)
                    }) {
                        listen_seed = Some(listen.id);
                        tracker.restore(listen);
                    }
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
                                listen_seed = None;
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
                        listen_seed = None;
                        index = Some(at);
                        base_position = 0.;
                        clock.playing.store(true, Ordering::Relaxed);
                        restart = true;
                    }
                }
                PlayerCommand::Next => {
                    if let Some(i) = index {
                        listen_seed = None;
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
                        listen_seed = None;
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
            pending_output = None;
            drop(output.take());
            output_id = default_output_id();
            if let (Some(at), Some(server), true) = (index, server.clone(), output_id.is_some()) {
                let request = DecodeRequest {
                    listen_id: *listen_seed.get_or_insert_with(rand::random),
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
                        if output_id.is_none() {
                            "No audio output available. Connect a device and retry."
                        } else {
                            "Connect to your server to play music."
                        }
                        .into(),
                    );
                }
            }
        }
        // A format change is requested only after all earlier PCM has been consumed.
        let epoch = clock.epoch.load(Ordering::Acquire);
        if pending_output
            .as_ref()
            .is_some_and(|(packet, _)| packet.epoch != epoch)
        {
            pending_output = None;
        }
        if let Some(first) = format_rx.try_iter().find(|packet| packet.epoch == epoch) {
            // The callback has filled an OS buffer, which may not have played yet.
            // Leave two buffer periods to drain before closing the old device stream.
            let drain =
                output
                    .as_ref()
                    .map_or(Duration::ZERO, |sink| match *sink.config().buffer_size() {
                        rodio::cpal::BufferSize::Fixed(frames) => Duration::from_secs_f64(
                            2. * frames as f64 / f64::from(sink.config().sample_rate().get()),
                        ),
                        rodio::cpal::BufferSize::Default => Duration::from_millis(200),
                    });
            pending_output = Some((first, Instant::now() + drain));
        }
        if pending_output.is_none() && output.is_none() && started {
            pending_output = pcm_rx
                .try_iter()
                .find(|packet| packet.epoch == epoch)
                .map(|packet| (packet, Instant::now()));
        }
        let next_output = if pending_output
            .as_ref()
            .is_some_and(|(_, ready)| Instant::now() >= *ready)
        {
            pending_output.take().map(|(packet, _)| packet)
        } else {
            None
        };
        if let Some(first) = next_output {
            if first.end {
                clock.finished.store(true, Ordering::Release);
            } else {
                let format = first.format;
                let next_index = first.index;
                let next_position = first.position.unwrap_or(0.);
                drop(output.take());
                match open_output(
                    clock.clone(),
                    pcm_rx.clone(),
                    device_errors.clone(),
                    format_changes.clone(),
                    first,
                ) {
                    Ok(sink) => {
                        output = Some(sink);
                        last_tick = Instant::now();
                        last_device_check = Instant::now();
                    }
                    Err(_) => {
                        index = Some(next_index);
                        base_position = next_position;
                        started = false;
                        clock.playing.store(false, Ordering::Relaxed);
                        clock.buffering.store(false, Ordering::Relaxed);
                        clock.epoch.fetch_add(1, Ordering::AcqRel);
                        error = Some(format!(
                            "Could not open source audio output ({} Hz, {} channels). Check the output device and retry. No conversion was applied.",
                            format.rate, format.channels,
                        ));
                    }
                }
            }
        }
        for (epoch, message) in error_rx.try_iter() {
            if epoch == clock.epoch.load(Ordering::Acquire) {
                base_position = position(&clock);
                started = false;
                pending_output = None;
                error = Some(message);
                clock.playing.store(false, Ordering::Relaxed);
                clock.buffering.store(false, Ordering::Relaxed);
                clock.epoch.fetch_add(1, Ordering::AcqRel);
                drop(output.take());
            }
        }
        if clock.finished.swap(false, Ordering::AcqRel) {
            clock.playing.store(false, Ordering::Relaxed);
            clock.buffering.store(false, Ordering::Relaxed);
            started = false;
            base_position = 0.;
        }
        if persist_dirty || last_snapshot.elapsed() >= Duration::from_millis(200) {
            let pos = if started {
                position(&clock)
            } else {
                base_position
            };
            let playing = clock.playing.load(Ordering::Relaxed);
            let track = index.and_then(|i| queue.get(i)).cloned();
            let stamp = SnapshotStamp {
                playing,
                buffering: clock.buffering.load(Ordering::Relaxed) && playing,
                position: pos.to_bits(),
                index,
                revision,
                error: error.clone(),
            };
            let changed = snapshot_stamp.as_ref() != Some(&stamp);
            if let Some(media) = media.as_mut()
                && (media_playing != Some(playing)
                    || (changed && (!playing || last_media.elapsed() >= Duration::from_secs(1))))
            {
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
                media_playing = Some(playing);
                last_media = Instant::now();
            }
            if queue_dirty {
                published_queue = Arc::new(queue.clone());
                queue_dirty = false;
            }
            if changed {
                snapshot_stamp = Some(stamp);
                let _ = events.send(PlayerEvent::Snapshot(PlaybackSnapshot {
                    duration: track.as_ref().map_or(0., |t| t.duration),
                    track,
                    playing,
                    buffering: clock.buffering.load(Ordering::Relaxed) && playing,
                    position: pos,
                    index,
                    queue: published_queue.clone(),
                    error: error.clone(),
                }));
            }
            last_snapshot = Instant::now();
        }
        if persist_dirty
            || (clock.playing.load(Ordering::Relaxed)
                && last_persist.elapsed() >= Duration::from_secs(3))
        {
            let _ = events.send(PlayerEvent::Persist(SavedQueue {
                listen: tracker.session(listen_seed),
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
    fn format(rate: u32, channels: u16) -> AudioFormat {
        AudioFormat {
            rate: rate.try_into().unwrap(),
            channels: channels.try_into().unwrap(),
        }
    }
    fn test_source(clock: Arc<Clock>, rx: Receiver<Packet>) -> BufferedAudio {
        let (format_changes, _) = flume::unbounded();
        BufferedAudio::new(clock, rx, format_changes, format(48_000, 2), None)
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn original_streams_keep_native_rates_channels_and_pcm_without_transcoding() {
        use axum::{Router, extract::Query, routing::get};
        use std::{collections::HashMap, io::Cursor};

        let mut files = HashMap::new();
        let mut expected = Vec::new();
        let formats = [format(44_100, 1), format(96_000, 2), format(48_000, 2)];
        for (index, format) in formats.into_iter().enumerate() {
            let mut bytes = Cursor::new(Vec::new());
            let samples = (0..format.samples_per_second() as usize / 10)
                .map(|sample| (sample % 2000) as i16 - 1000)
                .collect::<Vec<_>>();
            {
                let mut writer = hound::WavWriter::new(
                    &mut bytes,
                    hound::WavSpec {
                        channels: format.channels.get(),
                        sample_rate: format.rate.get(),
                        bits_per_sample: 16,
                        sample_format: hound::SampleFormat::Int,
                    },
                )
                .unwrap();
                for sample in &samples {
                    writer.write_sample(*sample).unwrap();
                }
                writer.finalize().unwrap();
            }
            files.insert(index.to_string(), bytes.into_inner());
            expected.push(
                samples
                    .into_iter()
                    .map(|sample| sample as f32 / 32768.)
                    .collect::<Vec<_>>(),
            );
        }
        let app = Router::new().route(
            "/rest/stream.view",
            get(move |Query(params): Query<HashMap<String, String>>| {
                let files = files.clone();
                async move {
                    assert_eq!(params.get("format").map(String::as_str), Some("raw"));
                    assert!(!params.contains_key("maxBitRate"));
                    files[&params["id"]].clone()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let http = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let server = Server::new(
            crate::platform::Account {
                url: format!("http://{address}"),
                username: "fixture-user".into(),
            },
            "fixture-password".into(),
        )
        .unwrap();
        let queue = (0..formats.len())
            .map(|index| {
                Arc::new(Track {
                    id: index.to_string(),
                    suffix: "wav".into(),
                    // Deliberately wrong metadata: output must follow the actual decoder.
                    sampling_rate: Some(192_000),
                    channel_count: Some(6),
                    ..Default::default()
                })
            })
            .collect();
        let clock = Arc::new(Clock::default());
        clock.epoch.store(1, Ordering::Release);
        let request = DecodeRequest {
            listen_id: 1,
            queue,
            index: 0,
            position: 0.,
            prefs: Preferences {
                gain: GainMode::Off,
                ..Default::default()
            },
            server,
            epoch: 1,
        };
        let (tx, rx) = flume::bounded(96);
        let (errors, error_rx) = flume::unbounded();
        let runtime = tokio::runtime::Handle::current();
        let worker = std::thread::spawn(move || decode_queue(request, tx, clock, errors, runtime));
        let mut actual = vec![vec![]; formats.len()];
        let mut first = [true; 3];
        loop {
            let packet = tokio::time::timeout(Duration::from_secs(10), rx.recv_async())
                .await
                .unwrap()
                .unwrap();
            if packet.end {
                break;
            }
            assert_eq!(packet.format, formats[packet.index]);
            assert_eq!(
                packet.samples.len() % usize::from(packet.format.channels.get()),
                0
            );
            assert_eq!(packet.position, first[packet.index].then_some(0.));
            first[packet.index] = false;
            actual[packet.index].extend(packet.samples);
        }
        worker.join().unwrap();
        assert!(error_rx.try_recv().is_err());
        assert_eq!(actual, expected);
        http.abort();
    }

    #[test]
    fn format_handoff_keeps_the_first_packet_and_native_clock() {
        let clock = Arc::new(Clock::default());
        clock.playing.store(true, Ordering::Relaxed);
        clock.volume.store(1f64.to_bits(), Ordering::Relaxed);
        clock.epoch.store(1, Ordering::Release);
        let (tx, rx) = flume::bounded(8);
        let (changes, change_rx) = flume::unbounded();
        let mono = format(44_100, 1);
        let stereo = format(96_000, 2);
        for (index, format, samples) in [(0, mono, vec![0.1; 4410]), (1, stereo, vec![0.2; 19200])]
        {
            tx.send(Packet {
                listen: None,
                epoch: 1,
                index,
                position: Some(0.),
                format,
                samples,
                end: false,
            })
            .unwrap();
        }
        let mut output = BufferedAudio::new(clock.clone(), rx.clone(), changes.clone(), mono, None);
        assert_eq!(
            output.by_ref().take(4410).collect::<Vec<_>>(),
            vec![0.1; 4410]
        );
        assert!((clock.position() - 0.1).abs() < 0.000001);
        assert_eq!(output.next(), Some(0.));
        assert_eq!(clock.active.load(Ordering::Acquire), 0);
        let first = change_rx.try_recv().unwrap();
        assert_eq!(first.samples.len(), 19200);
        assert_eq!(output.next(), Some(0.));
        assert!(
            change_rx.try_recv().is_err(),
            "Request exactly one output change"
        );
        let mut output = BufferedAudio::new(clock.clone(), rx, changes, stereo, Some(first));
        assert_eq!(output.sample_rate(), stereo.rate);
        assert_eq!(output.channels(), stereo.channels);
        assert_eq!(
            output.by_ref().take(19200).collect::<Vec<_>>(),
            vec![0.2; 19200]
        );
        assert!((clock.position() - 0.1).abs() < 0.000001);
        assert_eq!(clock.active.load(Ordering::Acquire), 1);
        assert_eq!(clock.boundary.load(Ordering::Acquire), 2);
        for channels in [1, 2, 6] {
            let format = format(44_100, channels);
            assert_eq!(format.chunk_size() % usize::from(channels), 0);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsupported_original_audio_does_not_retry_with_transcoding() {
        use axum::{Router, routing::get};
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let app = Router::new().route(
            "/rest/stream.view",
            get(move || {
                seen.fetch_add(1, Ordering::Relaxed);
                async { "This is not an audio file" }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let http = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let clock = Arc::new(Clock::default());
        clock.epoch.store(1, Ordering::Release);
        let request = DecodeRequest {
            listen_id: 1,
            queue: vec![Arc::new(Track {
                id: "unsupported".into(),
                suffix: "flac".into(),
                ..Default::default()
            })],
            index: 0,
            position: 0.,
            prefs: Preferences::default(),
            epoch: 1,
            server: Server::new(
                crate::platform::Account {
                    url: format!("http://{address}"),
                    username: "fixture-user".into(),
                },
                "fixture-password".into(),
            )
            .unwrap(),
        };
        let (tx, pcm) = flume::bounded(96);
        let (errors, error_rx) = flume::unbounded();
        let runtime = tokio::runtime::Handle::current();
        let worker = std::thread::spawn(move || decode_queue(request, tx, clock, errors, runtime));
        let (_, error) = tokio::time::timeout(Duration::from_secs(10), error_rx.recv_async())
            .await
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert_eq!(requests.load(Ordering::Relaxed), 1);
        assert!(error.contains("No transcoding was applied"));
        assert!(pcm.try_recv().is_err());
        http.abort();
    }
    #[test]
    fn listened_time_counts_only_consumed_samples_including_partial_packets() {
        let clock = Arc::new(Clock::default());
        clock.playing.store(true, Ordering::Relaxed);
        clock.epoch.store(1, Ordering::Release);
        let (tx, rx) = flume::bounded(4);
        let listen = Arc::new(AudibleListen {
            id: 1,
            account: "fixture".into(),
            track: Arc::new(Track {
                id: "track".into(),
                duration: 100.,
                ..Default::default()
            }),
        });
        tx.send(Packet {
            listen: Some(listen),
            epoch: 1,
            index: 0,
            position: Some(0.),
            format: format(44_100, 1),
            samples: vec![0.1; 44_100],
            end: false,
        })
        .unwrap();
        let (changes, _) = flume::unbounded();
        let mut source = BufferedAudio::new(clock.clone(), rx, changes, format(44_100, 1), None);
        source.by_ref().take(22_050).for_each(drop);
        clock.playing.store(false, Ordering::Relaxed);
        for _ in 0..100 {
            assert_eq!(source.next(), Some(0.));
        }
        let heard = clock
            .listen_rx
            .try_iter()
            .map(|progress| progress.seconds)
            .sum::<f64>();
        assert!((heard - 0.5).abs() < 1e-9);
        clock.epoch.store(2, Ordering::Release);
        drop(source);
        assert!(
            clock.listen_rx.try_recv().is_err(),
            "Already reported samples must not count twice on recovery"
        );
    }
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
            listen: None,
            epoch: 1,
            index: 0,
            position: Some(0.),
            format: format(48_000, 2),
            samples: vec![0.1, 0.2, 0.3, 0.4],
            end: false,
        })
        .unwrap();
        tx.send(Packet {
            listen: None,
            epoch: 1,
            index: 1,
            position: Some(0.),
            format: format(48_000, 2),
            samples: vec![0.5, 0.6, 0.7, 0.8],
            end: false,
        })
        .unwrap();
        let mut source = test_source(clock.clone(), rx);
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
            listen: None,
            epoch: 1,
            index: 0,
            position: Some(0.),
            format: format(48_000, 2),
            samples: vec![0.1; 8],
            end: false,
        })
        .unwrap();
        tx.send(Packet {
            listen: None,
            epoch: 2,
            index: 1,
            position: Some(20.),
            format: format(48_000, 2),
            samples: vec![0.5; 4],
            end: false,
        })
        .unwrap();
        let mut source = test_source(clock.clone(), rx);
        source.pending = Some(Packet {
            listen: None,
            epoch: 1,
            index: 0,
            position: Some(0.),
            format: format(48_000, 2),
            samples: vec![0.1; 8],
            end: false,
        });
        assert_eq!(source.next(), Some(0.5));
        assert_eq!(
            f64::from_bits(clock.position_base.load(Ordering::Relaxed)),
            20.
        );
    }

    #[test]
    fn visualizer_measures_consumed_output_and_goes_quiet_when_paused() {
        let clock = Arc::new(Clock::default());
        clock.playing.store(true, Ordering::Relaxed);
        clock.volume.store(0.5f64.to_bits(), Ordering::Relaxed);
        clock.epoch.store(1, Ordering::Release);
        let (tx, rx) = flume::bounded(8);
        tx.send(Packet {
            listen: None,
            epoch: 1,
            index: 0,
            position: Some(0.),
            format: format(48_000, 2),
            samples: vec![0.8; 3072],
            end: false,
        })
        .unwrap();
        let mut source = test_source(clock.clone(), rx);
        assert_eq!(clock.waveform.snapshot(), [0.; WAVEFORM_BARS]);
        source.by_ref().take(1536).for_each(drop);
        assert_eq!(clock.waveform.snapshot()[WAVEFORM_BARS - 1], 0.4);
        let (commands, _) = flume::unbounded();
        let handle = PlayerHandle {
            tx: commands,
            clock: clock.clone(),
        };
        assert_eq!(handle.waveform()[WAVEFORM_BARS - 1], 0.4);
        clock.playing.store(false, Ordering::Relaxed);
        assert_eq!(source.next(), Some(0.));
        assert_eq!(handle.waveform(), [0.; WAVEFORM_BARS]);
        clock.playing.store(true, Ordering::Relaxed);
        clock.epoch.store(2, Ordering::Release);
        tx.send(Packet {
            listen: None,
            epoch: 2,
            index: 0,
            position: Some(10.),
            format: format(48_000, 2),
            samples: vec![0.; 1536],
            end: false,
        })
        .unwrap();
        source.by_ref().take(1536).for_each(drop);
        assert_eq!(handle.waveform(), [0.; WAVEFORM_BARS]);
    }
}
