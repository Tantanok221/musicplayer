//! Automatic update checks on the worker's runtime.
use super::Event;
use crate::update::{COMMIT, Checked, UpdateState, UpdateStatus, Updater};
use anyhow::Result;
use flume::Sender;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{runtime::Handle, sync::mpsc};

/// Long enough after launch not to compete with the library sync.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

pub(super) struct Updates {
    updater: Option<Arc<Updater>>,
    wake: Option<mpsc::UnboundedSender<()>>,
    /// The release this process installed, which runs after a restart.
    installed: Arc<Mutex<Option<String>>>,
}

impl Updates {
    /// Starts checking with `updater`, or reports why this build can't update.
    pub fn start(
        updater: Result<Updater, Option<String>>,
        runtime: &Handle,
        events: Sender<Event>,
    ) -> Self {
        let installed = Arc::new(Mutex::new(None));
        let updater = match updater {
            Ok(updater) => Arc::new(updater),
            Err(reason) => {
                let _ = events.send(Event::Update(UpdateStatus {
                    current: COMMIT.to_owned(),
                    state: UpdateState::Unavailable(reason),
                }));
                return Self {
                    updater: None,
                    wake: None,
                    installed,
                };
            }
        };
        let (wake, mut woken) = mpsc::unbounded_channel();
        let (task_updater, task_installed) = (updater.clone(), installed.clone());
        runtime.spawn(async move {
            let status = |state| {
                Event::Update(UpdateStatus {
                    current: task_updater.current().to_owned(),
                    state,
                })
            };
            let _ = events.send(status(UpdateState::Idle));
            let mut next = FIRST_CHECK;
            loop {
                tokio::select! {
                    () = tokio::time::sleep(next) => {}
                    woke = woken.recv() => if woke.is_none() { return },
                }
                next = INTERVAL;
                let _ = events.send(status(UpdateState::Checking));
                let installed = task_installed.lock().ok().and_then(|slot| slot.clone());
                let state = match task_updater.check(installed.as_deref()).await {
                    Ok(Checked::Installed(commit)) => {
                        if let Ok(mut slot) = task_installed.lock() {
                            *slot = Some(commit.clone());
                        }
                        UpdateState::Ready(commit)
                    }
                    // An update this process installed stays ready after a failed check.
                    Ok(Checked::UpToDate) => {
                        installed.map_or(UpdateState::UpToDate, UpdateState::Ready)
                    }
                    Err(error) => installed.map_or_else(
                        || UpdateState::Failed(format!("{error:#}")),
                        UpdateState::Ready,
                    ),
                };
                let _ = events.send(status(state));
            }
        });
        Self {
            updater: Some(updater),
            wake: Some(wake),
            installed,
        }
    }

    pub fn check(&self) {
        if let Some(wake) = &self.wake {
            let _ = wake.send(());
        }
    }

    /// Starts the installed update after this process exits; the UI then quits.
    pub fn restart(&self, events: &Sender<Event>) -> Result<()> {
        let ready = self.installed.lock().is_ok_and(|slot| slot.is_some());
        let Some(updater) = self.updater.as_ref().filter(|_| ready) else {
            return Ok(());
        };
        updater.relaunch()?;
        events.send(Event::Relaunching)?;
        Ok(())
    }
}
