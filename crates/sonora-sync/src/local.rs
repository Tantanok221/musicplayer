use crate::{
    session::{Peer, SessionHub},
    store::{Store, StoreError},
};
use sonora_sync_protocol::{ClientMessage, Device, Hello, ServerMessage};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
};

pub use crate::session::SessionError;

#[derive(Clone)]
pub struct LocalCoordinator {
    hub: SessionHub,
    generations: Arc<AtomicU64>,
}

impl LocalCoordinator {
    pub fn new() -> Result<Self, StoreError> {
        Ok(Self {
            hub: SessionHub::new(Store::open(Path::new(":memory:"))?),
            generations: Arc::new(AtomicU64::new(0)),
        })
    }

    pub async fn join(&self, device: Device) -> Result<(LocalPeer, ServerMessage), SessionError> {
        let (out, inbox) = mpsc::channel(32);
        let (cancel, cancelled) = watch::channel(false);
        let generation = self.generations.fetch_add(1, Ordering::Relaxed);
        let id = device.id;
        let welcome = self
            .hub
            .join(
                Hello {
                    username: String::new(),
                    subsonic_token: String::new(),
                    subsonic_salt: String::new(),
                    device_id: id.clone(),
                    device_name: device.name,
                    os: device.os,
                    client_version: String::new(),
                },
                Peer {
                    generation,
                    out,
                    cancel,
                },
            )
            .await?;
        Ok((
            LocalPeer {
                hub: self.hub.clone(),
                runtime: Handle::current(),
                id,
                generation,
                inbox,
                cancelled,
            },
            welcome,
        ))
    }
}

pub struct LocalPeer {
    hub: SessionHub,
    runtime: Handle,
    id: String,
    generation: u64,
    inbox: mpsc::Receiver<ServerMessage>,
    cancelled: watch::Receiver<bool>,
}

impl LocalPeer {
    pub async fn send(&self, message: ClientMessage) -> Result<(), SessionError> {
        self.hub
            .receive(String::new(), self.id.clone(), self.generation, message)
            .await
    }

    pub async fn receive(&mut self) -> Option<ServerMessage> {
        tokio::select! {
            _ = self.cancelled.changed() => None,
            message = self.inbox.recv() => message,
        }
    }
}

impl Drop for LocalPeer {
    fn drop(&mut self) {
        let hub = self.hub.clone();
        let id = self.id.clone();
        let generation = self.generation;
        self.runtime.spawn(async move {
            let _ = hub.disconnect(String::new(), id, generation).await;
        });
    }
}
