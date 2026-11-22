//! Real catalog actor with a private network runtime and silent player endpoint.
use super::{Command, Event, Service};
use crate::{
    lastfm::LastFm,
    model::{Preferences, SavedQueue},
    musicbrainz::MusicBrainz,
    platform::{Account, Paths},
    playback::PlayerCommand,
};
use anyhow::{Context, Result, ensure};
use flume::Receiver;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, atomic::AtomicU64},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct Worker {
    service: Service,
    events: WorkerEvents,
    player_commands: Receiver<PlayerCommand>,
    finished: Receiver<Result<(), String>>,
    thread: Option<JoinHandle<()>>,
    player_threads: Vec<JoinHandle<()>>,
}

/// Single-consumer event stream retaining events observed while awaiting readiness.
pub struct WorkerEvents {
    received: Receiver<Event>,
    pending: Mutex<VecDeque<Event>>,
}

impl WorkerEvents {
    pub fn recv_deadline(&self, deadline: Instant) -> Result<Event, flume::RecvTimeoutError> {
        if let Some(event) = self.pending.lock().unwrap().pop_front() {
            return Ok(event);
        }
        self.received.recv_deadline(deadline)
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, flume::RecvTimeoutError> {
        self.recv_deadline(Instant::now() + timeout)
    }

    pub fn try_iter(&self) -> impl Iterator<Item = Event> + '_ {
        std::iter::from_fn(|| {
            self.pending
                .lock()
                .unwrap()
                .pop_front()
                .or_else(|| self.received.try_recv().ok())
        })
    }

    pub fn is_disconnected(&self) -> bool {
        self.received.is_disconnected()
    }

    fn wait_ready(&self, timeout: Duration) -> Result<(Option<Account>, Preferences)> {
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .received
                .recv_deadline(deadline)
                .context("worker readiness")?
            {
                Event::Ready(account, preferences) => return Ok((account, preferences)),
                event => self.pending.lock().unwrap().push_back(event),
            }
        }
    }
}

impl Worker {
    /// Uses the production command loop, SQLite, HTTP clients and event ordering.
    /// The supplied metadata and Last.fm clients must use fixture endpoints in isolated tests.
    pub fn start(paths: Paths, metadata: MusicBrainz, lastfm: LastFm) -> Result<Self> {
        Self::start_inner(paths, metadata, lastfm, false)
    }

    pub fn start_paused(paths: Paths, metadata: MusicBrainz, lastfm: LastFm) -> Result<Self> {
        Self::start_inner(paths, metadata, lastfm, true)
    }

    fn start_inner(
        paths: Paths,
        metadata: MusicBrainz,
        lastfm: LastFm,
        paused: bool,
    ) -> Result<Self> {
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
            // Exercise the legal ordering where playback wins catalog startup.
            match player_events.recv_timeout(Duration::from_secs(5))? {
                crate::playback::PlayerEvent::Snapshot(snapshot) => {
                    events.send(Event::Playback(snapshot))?;
                }
                _ => anyhow::bail!("paused player did not publish a snapshot first"),
            }
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
                        lastfm,
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
            events: WorkerEvents {
                received: received_events,
                pending: Mutex::default(),
            },
            player_commands,
            finished: received_finish,
            thread: Some(thread),
            player_threads,
        })
    }

    pub fn service(&self) -> &Service {
        &self.service
    }

    pub fn events(&self) -> &WorkerEvents {
        &self.events
    }

    pub fn player_commands(&self) -> &Receiver<PlayerCommand> {
        &self.player_commands
    }

    pub fn wait_ready(&self, timeout: Duration) -> Result<(Option<Account>, Preferences)> {
        self.events.wait_ready(timeout)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stream() -> (flume::Sender<Event>, WorkerEvents) {
        let (send, received) = flume::unbounded();
        (
            send,
            WorkerEvents {
                received,
                pending: Mutex::default(),
            },
        )
    }

    #[test]
    fn readiness_preserves_all_events_in_order() {
        let (send, events) = stream();
        send.send(Event::Status("before".into())).unwrap();
        send.send(Event::Error("startup error".into())).unwrap();
        send.send(Event::Ready(None, Preferences::default()))
            .unwrap();
        send.send(Event::Status("after".into())).unwrap();
        events.wait_ready(Duration::ZERO).unwrap();
        assert!(
            matches!(events.recv_timeout(Duration::ZERO).unwrap(), Event::Status(s) if s == "before")
        );
        assert!(
            matches!(events.recv_timeout(Duration::ZERO).unwrap(), Event::Error(s) if s == "startup error")
        );
        assert!(
            matches!(events.recv_timeout(Duration::ZERO).unwrap(), Event::Status(s) if s == "after")
        );
    }

    #[test]
    fn readiness_timeout_and_disconnect_keep_pending_events() {
        let (send, events) = stream();
        assert!(events.wait_ready(Duration::ZERO).is_err());
        send.send(Event::Status("pending".into())).unwrap();
        assert!(events.wait_ready(Duration::ZERO).is_err());
        drop(send);
        assert!(events.wait_ready(Duration::ZERO).is_err());
        assert!(events.is_disconnected());
        assert_eq!(events.try_iter().count(), 1);
        assert!(matches!(
            events.recv_timeout(Duration::ZERO),
            Err(flume::RecvTimeoutError::Disconnected)
        ));
    }
}
