//! Real paused player actor, without audio discovery, decoders or media controls.
use super::{Clock, HeadlessOutput, PlaybackPlatform, PlayerCommand, PlayerEvent, run};
use crate::model::PlaybackSnapshot;
use anyhow::{Context, Result, bail, ensure};
use flume::{Receiver, Sender};
use std::{
    sync::{Arc, atomic::Ordering},
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub struct PausedPlayer {
    commands: Sender<PlayerCommand>,
    events: Receiver<PlayerEvent>,
    finished: Receiver<Result<(), String>>,
    thread: Option<JoinHandle<()>>,
}

impl PausedPlayer {
    pub fn start(timeout: Duration) -> Result<Self> {
        Self::spawn(HeadlessOutput::default(), Arc::default(), timeout)
    }

    fn spawn(output: HeadlessOutput, clock: Arc<Clock>, timeout: Duration) -> Result<Self> {
        let (commands, received) = flume::unbounded();
        let (events, received_events) = flume::unbounded();
        let (finished, received_finish) = flume::bounded(1);
        let thread = std::thread::Builder::new()
            .name("sonora-test-paused-player".into())
            .spawn(move || {
                let result = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    let result = run(
                        received,
                        events,
                        runtime.handle().clone(),
                        clock,
                        PlaybackPlatform::Headless(output),
                    );
                    drop(runtime);
                    result
                })();
                let _ = finished.send(result.map_err(|error: anyhow::Error| format!("{error:#}")));
            })?;
        let player = Self {
            commands,
            events: received_events,
            finished: received_finish,
            thread: Some(thread),
        };
        // Readiness is a completed real command, not a sleep or a synthetic event.
        player.command(PlayerCommand::ClearError, timeout)?;
        Ok(player)
    }

    /// Only paused queue commands are accepted. No server, transport or decoder
    /// can be started through this test endpoint. Every result is a real snapshot.
    pub fn command(&self, command: PlayerCommand, timeout: Duration) -> Result<PlaybackSnapshot> {
        ensure!(
            matches!(
                command,
                PlayerCommand::Restore(..)
                    | PlayerCommand::Insert(..)
                    | PlayerCommand::Move(..)
                    | PlayerCommand::EditQueue(..)
                    | PlayerCommand::UndoQueue
                    | PlayerCommand::UpdateTrack(..)
                    | PlayerCommand::Preferences(..)
                    | PlayerCommand::ClearError
            ),
            "paused player only accepts queue commands"
        );
        self.commands.send(command)?;
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .events
                .recv_deadline(deadline)
                .context("paused player snapshot")?
            {
                PlayerEvent::Snapshot(snapshot) => return Ok(snapshot),
                PlayerEvent::Persist(..) => {}
                _ => bail!("unexpected event from paused player"),
            }
        }
    }

    pub fn shutdown(&mut self, timeout: Duration) -> Result<()> {
        if self.thread.is_none() {
            return Ok(());
        }
        self.commands.send(PlayerCommand::Shutdown)?;
        let result = self
            .finished
            .recv_timeout(timeout)
            .context("paused player completion")?;
        self.thread
            .take()
            .expect("player thread")
            .join()
            .map_err(|_| anyhow::anyhow!("paused player thread panicked"))?;
        result.map_err(anyhow::Error::msg)
    }
}

/// Real actor on a scripted output device, for device-loss scenarios. No server,
/// decoder or audio device is opened: `start_audio` marks the clock audible the
/// way a running stream does, and `switch_output` changes the default device.
pub struct OutputPlayer {
    player: PausedPlayer,
    clock: Arc<Clock>,
    output: HeadlessOutput,
}

impl OutputPlayer {
    pub fn start(device: &str, timeout: Duration) -> Result<Self> {
        let output = HeadlessOutput::new(Some(device.into()).into());
        let clock = Arc::new(Clock::default());
        let player = PausedPlayer::spawn(output.clone(), clock.clone(), timeout)?;
        Ok(Self {
            player,
            clock,
            output,
        })
    }

    pub fn clear_error(&self, timeout: Duration) -> Result<PlaybackSnapshot> {
        self.player.command(PlayerCommand::ClearError, timeout)
    }

    pub fn start_audio(&self) {
        self.clock.playing.store(true, Ordering::Relaxed);
    }

    pub fn switch_output(&self, device: Option<&str>) {
        *self.output.lock().unwrap() = device.map(Into::into);
    }

    /// The next snapshot that reports a playback error.
    pub fn next_error(&self, timeout: Duration) -> Result<PlaybackSnapshot> {
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .player
                .events
                .recv_deadline(deadline)
                .context("player error snapshot")?
            {
                PlayerEvent::Snapshot(snapshot) if snapshot.error.is_some() => {
                    return Ok(snapshot);
                }
                PlayerEvent::Snapshot(_) | PlayerEvent::Persist(..) => {}
                _ => bail!("unexpected event from output player"),
            }
        }
    }

    /// No error snapshot arrives within `period`.
    pub fn stays_clear(&self, period: Duration) -> Result<()> {
        match self.next_error(period) {
            Ok(snapshot) => bail!("unexpected playback error {:?}", snapshot.error),
            Err(_) => Ok(()),
        }
    }
}

impl Drop for PausedPlayer {
    fn drop(&mut self) {
        let _ = self.shutdown(Duration::from_secs(5));
    }
}

/// Production actor with the headless platform, for worker persistence scenarios.
/// The caller must keep playback paused and join after the shutdown handshake.
pub(crate) fn start_worker_player()
-> Result<(super::PlayerHandle, Receiver<PlayerEvent>, JoinHandle<()>)> {
    let (handle, commands) = super::PlayerHandle::controlled();
    let clock = handle.clock.clone();
    let (events, received) = flume::unbounded();
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        run(
            commands,
            events,
            runtime.handle().clone(),
            clock,
            PlaybackPlatform::Headless(HeadlessOutput::default()),
        )
        .unwrap();
    });
    Ok((handle, received, thread))
}
