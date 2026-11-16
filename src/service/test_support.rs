//! Channel-backed application dependencies, enabled only by `test-support`.
//! Callers control scheduling and responses. Unknown commands should fail their
//! scenario rather than falling through to a real network or audio backend.
use super::{Command, Event, Service, persist_preferences};
use crate::{catalog::Catalog, model::Preferences, playback::PlayerCommand};
use flume::{Receiver, Sender};
use std::sync::{Arc, atomic::AtomicU64};

mod worker;
pub use worker::Worker;

pub struct ControlledService {
    service: Service,
    commands: Receiver<Command>,
    events: Sender<Event>,
    received_events: Receiver<Event>,
    player_commands: Receiver<PlayerCommand>,
}

impl Default for ControlledService {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlledService {
    pub fn new() -> Self {
        let (tx, commands) = flume::unbounded();
        let (events, received_events) = flume::unbounded();
        let (player, player_commands) = crate::playback::PlayerHandle::controlled();
        Self {
            service: Service {
                tx,
                player,
                query_epoch: Arc::new(AtomicU64::new(0)),
            },
            commands,
            events,
            received_events,
            player_commands,
        }
    }

    pub fn service(&self) -> Service {
        self.service.clone()
    }

    pub fn events(&self) -> Receiver<Event> {
        self.received_events.clone()
    }

    pub fn commands(&self) -> &Receiver<Command> {
        &self.commands
    }

    pub fn player_commands(&self) -> &Receiver<PlayerCommand> {
        &self.player_commands
    }

    /// Deliver through the production UI event-receive task.
    pub fn emit(&self, event: Event) {
        self.events
            .send(event)
            .expect("controlled UI event channel");
    }

    /// Exercise the same SQLite write and player update as the service worker.
    pub fn persist_preferences(&self, catalog: &Catalog, prefs: Preferences) -> anyhow::Result<()> {
        persist_preferences(catalog, &self.service.player, prefs)
    }
}
