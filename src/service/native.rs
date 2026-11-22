use super::{Command, Event};
use crate::{
    platform::Paths,
    server::{NativeError, Server},
    smart_rules::SmartRules,
};
use anyhow::Result;
use flume::Sender;
use std::collections::VecDeque;

#[derive(Clone)]
pub(super) enum Operation {
    Load(String),
    Save {
        id: Option<String>,
        name: String,
        rules: Box<SmartRules>,
    },
}

#[derive(Clone)]
pub enum Reply {
    Rules(String, Option<SmartRules>),
    Saved(String),
}

#[derive(Default)]
pub(super) struct Editor {
    pub epoch: u64,
    pub busy: bool,
    waiting: bool,
    pending: VecDeque<Operation>,
}

impl Editor {
    pub fn reset(&mut self) {
        *self = Self {
            epoch: self.epoch + 1,
            ..Self::default()
        };
    }

    pub fn require_server(server: Option<&Server>) -> Result<&Server> {
        let server = server
            .ok_or_else(|| anyhow::anyhow!("Connect to Navidrome to edit smart playlists"))?;
        anyhow::ensure!(
            server.is_navidrome,
            "Smart playlist editing requires Navidrome"
        );
        Ok(server)
    }

    pub fn enqueue(&mut self, operation: Operation, events: &Sender<Event>) -> Result<()> {
        self.pending.push_back(operation);
        if self.waiting {
            events.send(Event::NativePasswordRequired)?;
        }
        Ok(())
    }

    pub fn start(
        &mut self,
        server: Option<&Server>,
        paths: &Paths,
        runtime: &tokio::runtime::Handle,
        tx: &Sender<Command>,
    ) {
        if self.busy || self.waiting {
            return;
        }
        let Some(operation) = self.pending.front().cloned() else {
            return;
        };
        let Some(server) = server.cloned() else {
            return;
        };
        self.busy = true;
        let epoch = self.epoch;
        let paths = paths.clone();
        let tx = tx.clone();
        runtime.spawn(async move {
            let result = perform(&server, &paths, operation).await;
            let _ = tx.send(Command::NativeCompleted(epoch, result));
        });
    }

    pub fn sign_in(
        &mut self,
        server: &Server,
        paths: &Paths,
        password: String,
        runtime: &tokio::runtime::Handle,
        tx: &Sender<Command>,
    ) -> Result<()> {
        anyhow::ensure!(
            !self.busy,
            "Wait for the current smart playlist operation to finish"
        );
        self.busy = true;
        let epoch = self.epoch;
        let server = server.clone();
        let paths = paths.clone();
        let tx = tx.clone();
        runtime.spawn(async move {
            let result = async {
                let token = server
                    .native_login(&password)
                    .await
                    .map_err(|error| error.to_string())?;
                paths
                    .save_native_token(&server.account, &token)
                    .map_err(|_| "Could not save the Navidrome login locally".to_string())
            }
            .await;
            let _ = tx.send(Command::NativeSignedIn(epoch, result));
        });
        Ok(())
    }

    pub fn signed_in(&mut self, epoch: u64, result: Result<(), String>) -> Result<()> {
        if epoch != self.epoch {
            return Ok(());
        }
        self.busy = false;
        match result {
            Ok(()) => self.waiting = false,
            Err(message) => anyhow::bail!(message),
        }
        Ok(())
    }

    pub fn complete(
        &mut self,
        epoch: u64,
        result: Result<Reply, NativeError>,
        events: &Sender<Event>,
    ) -> Result<Option<Reply>> {
        if epoch != self.epoch {
            return Ok(None);
        }
        self.busy = false;
        match result {
            Err(NativeError::PasswordRequired) => {
                self.waiting = true;
                events.send(Event::NativePasswordRequired)?;
                Ok(None)
            }
            result => {
                self.pending.pop_front();
                Ok(Some(result?))
            }
        }
    }
}

async fn perform(
    server: &Server,
    paths: &Paths,
    operation: Operation,
) -> Result<Reply, NativeError> {
    let mut token = paths
        .native_token(&server.account)
        .map_err(|_| {
            NativeError::Message("Could not read the saved Navidrome login. Sign in again.")
        })?
        .ok_or(NativeError::PasswordRequired)?;
    let result = match operation {
        Operation::Load(id) => server
            .smart_rules(&mut token, &id)
            .await
            .map(|rules| Reply::Rules(id, rules)),
        Operation::Save { id, name, rules } => match id {
            Some(id) => server
                .update_smart_playlist(&mut token, &id, &name, &rules)
                .await
                .map(|()| Reply::Saved(id)),
            None => server
                .create_smart_playlist(&mut token, &name, &rules)
                .await
                .map(Reply::Saved),
        },
    };
    if matches!(result, Err(NativeError::PasswordRequired)) {
        paths
            .clear_native_token(&server.account)
            .map_err(|_| NativeError::Message("Could not clear the saved Navidrome login"))?;
    } else {
        paths.save_native_token(&server.account, &token).map_err(|_| NativeError::Message("Could not save the Navidrome login locally. Check your playlists before retrying."))?;
    }
    result
}
