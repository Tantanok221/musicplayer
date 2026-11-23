use super::{Command, Event};
use crate::{
    platform::{Account, NativeToken, Paths},
    server::{NativeError, Server},
    smart_rules::SmartRules,
};
use anyhow::Result;
use flume::Sender;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Renewal {
    epoch: u64,
    busy: bool,
    renewed: Option<Instant>,
}

impl Renewal {
    fn due(&self) -> bool {
        !self.busy
            && self
                .renewed
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(3600))
    }

    pub fn reset(&mut self) {
        *self = Self {
            epoch: self.epoch + 1,
            ..Self::default()
        };
    }

    pub fn start(
        &mut self,
        server: &Server,
        paths: &Paths,
        runtime: &tokio::runtime::Handle,
        tx: &Sender<Command>,
    ) {
        if !server.is_navidrome || !self.due() {
            return;
        }
        let Ok(Some(original)) = paths.native_token(&server.account) else {
            return;
        };
        self.busy = true;
        let epoch = self.epoch;
        let server = server.clone();
        let tx = tx.clone();
        runtime.spawn(async move {
            let mut token = original.clone();
            let result = match tokio::time::timeout(
                Duration::from_secs(5),
                server.renew_native_session(&mut token),
            )
            .await
            {
                Ok(result) => result.map(|()| token),
                Err(_) => Err(NativeError::Message("Navidrome session renewal timed out")),
            };
            let _ = tx.send(Command::NativeRenewed(epoch, original, result));
        });
    }

    pub fn complete(
        &mut self,
        epoch: u64,
        original: NativeToken,
        result: Result<NativeToken, NativeError>,
        account: Option<&Account>,
        paths: &Paths,
    ) {
        if epoch != self.epoch {
            return;
        }
        self.busy = false;
        let Some(account) = account else {
            return;
        };
        let Ok(Some(current)) = paths.native_token(account) else {
            return;
        };
        if current.value() != original.value() {
            return;
        }
        match result {
            Ok(token) => {
                if paths.save_native_token(account, &token).is_ok() {
                    self.renewed = Some(Instant::now());
                }
            }
            Err(NativeError::PasswordRequired) => {
                let _ = paths.clear_native_token(account);
            }
            Err(_) => {}
        }
    }
}

pub(super) fn sign_in_session(
    server: &Server,
    paths: &Paths,
    runtime: &tokio::runtime::Handle,
    password: String,
) -> Option<flume::Receiver<()>> {
    if !server.is_navidrome || password.is_empty() {
        return None;
    }
    let server = server.clone();
    let paths = paths.clone();
    let (done, finished) = flume::bounded(1);
    runtime.spawn(async move {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            server.native_login(&password),
        )
        .await;
        drop(password);
        if let Ok(Ok(token)) = result {
            let _ = paths.save_native_token(&server.account, &token);
        }
        let _ = done.send(());
    });
    Some(finished)
}

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
    session: Option<flume::Receiver<()>>,
}

impl Editor {
    pub fn session(&mut self, session: Option<flume::Receiver<()>>) {
        if session.is_some() {
            self.session = session;
            self.waiting = false;
        }
    }

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
        if let Some(session) = self.session.take() {
            runtime.spawn(async move {
                let _ = session.recv_async().await;
                let _ = tx.send(Command::NativeSessionReady(epoch));
            });
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renewal_is_throttled_for_an_hour_and_while_in_flight() {
        let mut renewal = Renewal::default();
        assert!(renewal.due());
        renewal.busy = true;
        assert!(!renewal.due());
        renewal.busy = false;
        renewal.renewed = Some(Instant::now());
        assert!(!renewal.due());
        renewal.renewed = Some(Instant::now() - Duration::from_secs(3601));
        assert!(renewal.due());
    }

    #[test]
    fn renewal_does_not_overwrite_or_clear_a_newer_login_or_another_account() {
        let directory = tempfile::tempdir().unwrap();
        let paths = Paths {
            data: directory.path().into(),
            cache: directory.path().join("cache"),
        };
        let account = Account {
            url: "http://localhost".into(),
            username: "listener".into(),
        };
        let original = || NativeToken::new("original".into()).unwrap();
        let replacement = || NativeToken::new("replacement".into()).unwrap();
        paths.save_native_token(&account, &replacement()).unwrap();
        let mut renewal = Renewal::default();
        for result in [
            Ok(original()),
            Err(NativeError::PasswordRequired),
            Err(NativeError::Message("Could not reach Navidrome")),
        ] {
            renewal.complete(0, original(), result, Some(&account), &paths);
            assert!(
                paths
                    .native_token(&account)
                    .unwrap()
                    .is_some_and(|t| t.value() == "replacement")
            );
        }
        paths.save_native_token(&account, &original()).unwrap();
        renewal.reset();
        renewal.busy = true;
        for result in [Ok(replacement()), Err(NativeError::PasswordRequired)] {
            renewal.complete(0, original(), result, Some(&account), &paths);
            assert!(renewal.busy);
            assert!(
                paths
                    .native_token(&account)
                    .unwrap()
                    .is_some_and(|t| t.value() == "original")
            );
        }
        renewal.busy = false;
        renewal.complete(
            1,
            original(),
            Err(NativeError::Message("Could not reach Navidrome")),
            Some(&account),
            &paths,
        );
        assert!(
            paths
                .native_token(&account)
                .unwrap()
                .is_some_and(|t| t.value() == "original")
        );
    }
}
