use crate::{
    catalog::Catalog,
    model::SavedQueue,
    server::Server,
    sync::{milliseconds, wall_ms},
};
use anyhow::Result;
use flume::Sender;
use std::time::{Duration, Instant};
use tokio::{runtime::Handle, sync::mpsc};

const MODIFIED_KEY: &str = "queue:modified_ms";
const SAVE_INTERVAL: Duration = Duration::from_secs(15);

pub(super) struct QueueSync {
    namespace: Option<String>,
    last_write: Option<Instant>,
    was_playing: bool,
    changed: bool,
    writes: mpsc::Sender<QueueWrite>,
}

struct QueueWrite {
    server: Server,
    queue: SavedQueue,
    done: Option<Sender<()>>,
}

impl QueueSync {
    pub fn new(runtime: &Handle) -> Self {
        let (writes, mut requests) = mpsc::channel::<QueueWrite>(8);
        runtime.spawn(async move {
            while let Some(write) = requests.recv().await {
                let _ = tokio::time::timeout(
                    Duration::from_secs(5),
                    write.server.save_play_queue(&write.queue),
                )
                .await;
                if let Some(done) = write.done {
                    let _ = done.send(());
                }
            }
        });
        Self {
            namespace: None,
            last_write: None,
            was_playing: false,
            changed: false,
            writes,
        }
    }

    pub fn connected(&mut self, server: &Server) {
        let namespace = server.account.namespace();
        if self.namespace.as_ref() == Some(&namespace) {
            return;
        }
        self.namespace = Some(namespace);
        self.last_write = None;
        self.was_playing = false;
        self.changed = false;
    }

    fn local_change(catalog: &Catalog, queue: &SavedQueue) -> Result<bool> {
        let previous: SavedQueue = catalog.state("queue")?;
        Ok(queue.ids != previous.ids
            || queue.index != previous.index
            || milliseconds(queue.position) != milliseconds(previous.position))
    }

    pub fn persist(
        &mut self,
        catalog: &Catalog,
        queue: SavedQueue,
        server: Option<&Server>,
        playing: bool,
    ) -> Result<()> {
        let changed = Self::local_change(catalog, &queue)?;
        catalog.set_state("queue", &queue)?;
        if changed {
            catalog.set_state(MODIFIED_KEY, &wall_ms())?;
            self.changed = true;
        }
        let paused = self.was_playing && !playing;
        self.was_playing = playing;
        if let Some(server) = server
            && (paused
                || ((self.changed || playing)
                    && self
                        .last_write
                        .is_none_or(|last| last.elapsed() >= SAVE_INTERVAL)))
            && self
                .writes
                .try_send(QueueWrite {
                    server: server.clone(),
                    queue,
                    done: None,
                })
                .is_ok()
        {
            self.last_write = Some(Instant::now());
        }
        Ok(())
    }

    pub fn quit(
        &self,
        catalog: &Catalog,
        queue: SavedQueue,
        server: Option<&Server>,
    ) -> Result<()> {
        let changed = Self::local_change(catalog, &queue)?;
        catalog.set_state("queue", &queue)?;
        if changed {
            catalog.set_state(MODIFIED_KEY, &wall_ms())?;
        }
        if let Some(server) = server
            && (changed || self.changed || self.was_playing)
        {
            let (done, saved) = flume::bounded(1);
            if self
                .writes
                .try_send(QueueWrite {
                    server: server.clone(),
                    queue,
                    done: Some(done),
                })
                .is_ok()
            {
                let _ = saved.recv_timeout(Duration::from_secs(2));
            }
        }
        Ok(())
    }
}
