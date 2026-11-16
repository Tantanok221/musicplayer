//! Real catalog actor with a private network runtime and silent player endpoint.
use super::{Command, Event, Service};
use crate::{
    model::{Preferences, SavedQueue},
    musicbrainz::MusicBrainz,
    platform::{Account, Paths},
    playback::PlayerCommand,
};
use anyhow::{Context, Result, ensure};
use flume::Receiver;
use std::{
    sync::{Arc, atomic::AtomicU64},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct Worker {
    service: Service,
    events: Receiver<Event>,
    player_commands: Receiver<PlayerCommand>,
    finished: Receiver<Result<(), String>>,
    thread: Option<JoinHandle<()>>,
    player_threads: Vec<JoinHandle<()>>,
}

impl Worker {
    /// Uses the production command loop, SQLite, HTTP clients and event ordering.
    /// The supplied metadata client must use fixture endpoints in isolated tests.
    pub fn start(paths: Paths, metadata: MusicBrainz) -> Result<Self> {
        Self::start_inner(paths, metadata, false)
    }

    pub fn start_paused(paths: Paths, metadata: MusicBrainz) -> Result<Self> {
        Self::start_inner(paths, metadata, true)
    }

    fn start_inner(paths: Paths, metadata: MusicBrainz, paused: bool) -> Result<Self> {
        let (tx, commands) = flume::unbounded();
        let (events, received_events) = flume::unbounded();
        let (player, player_commands) = crate::playback::PlayerHandle::controlled();
        let service = Service {
            tx,
            player,
            query_epoch: Arc::new(AtomicU64::new(0)),
        };
        let mut service = service;
        let mut player_threads = Vec::new();
        if paused {
            let (player, player_events, thread) =
                crate::playback::test_support::start_worker_player()?;
            service.player = player;
            player_threads.push(thread);
            let command_tx = service.tx.clone();
            let event_tx = events.clone();
            player_threads.push(thread::spawn(move || {
                super::super::forward_player_events(player_events, command_tx, event_tx)
            }));
        }
        let worker = service.clone();
        let (finished, received_finish) = flume::bounded(1);
        let thread = thread::Builder::new()
            .name("sonora-test-catalog".into())
            .spawn(move || {
                let result = (|| {
                    let runtime = tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(2)
                        .enable_all()
                        .build()?;
                    let result = super::super::run(
                        paths,
                        runtime.handle().clone(),
                        worker,
                        commands,
                        events,
                        metadata,
                    );
                    // Dropping this private runtime cancels async requests/timers
                    // and waits for blocking work before reporting completion.
                    drop(runtime);
                    result
                })();
                let _ = finished.send(result.map_err(|error: anyhow::Error| format!("{error:#}")));
            })?;
        Ok(Self {
            service,
            events: received_events,
            player_commands,
            finished: received_finish,
            thread: Some(thread),
            player_threads,
        })
    }

    pub fn service(&self) -> &Service {
        &self.service
    }

    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }

    pub fn player_commands(&self) -> &Receiver<PlayerCommand> {
        &self.player_commands
    }

    pub fn wait_ready(&self, timeout: Duration) -> Result<(Option<Account>, Preferences)> {
        match self
            .events
            .recv_timeout(timeout)
            .context("worker readiness")?
        {
            Event::Ready(account, preferences) => Ok((account, preferences)),
            _ => anyhow::bail!("worker did not publish Ready first"),
        }
    }

    /// Acknowledge the real queue-save handshake with an empty controlled queue,
    /// then confirm the actor and its runtime stopped. Waits share one deadline.
    pub fn shutdown(&mut self, timeout: Duration) -> Result<()> {
        if self.thread.is_none() {
            return Ok(());
        }
        let deadline = Instant::now() + timeout;
        let remaining = || deadline.saturating_duration_since(Instant::now());
        let (ack, received_ack) = flume::bounded(1);
        self.service.tx.send(Command::Shutdown(ack))?;
        if self.player_threads.is_empty() {
            let mut saved = false;
            loop {
                match self
                    .player_commands
                    .recv_timeout(remaining())
                    .context("controlled player shutdown")?
                {
                    PlayerCommand::SaveQueue(reply) => {
                        reply.send(SavedQueue::default())?;
                        saved = true;
                    }
                    PlayerCommand::Shutdown => break,
                    _ => {}
                }
            }
            ensure!(saved, "worker skipped the queue-save handshake");
        }
        received_ack
            .recv_timeout(remaining())
            .context("worker shutdown acknowledgement")?;
        let result = self
            .finished
            .recv_timeout(remaining())
            .context("worker/runtime completion")?;
        self.thread
            .take()
            .expect("worker thread")
            .join()
            .map_err(|_| anyhow::anyhow!("worker thread panicked"))?;
        for thread in self.player_threads.drain(..) {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("player or event bridge panicked"))?;
        }
        result.map_err(anyhow::Error::msg)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.shutdown(Duration::from_secs(5));
    }
}
