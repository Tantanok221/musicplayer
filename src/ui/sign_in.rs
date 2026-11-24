use crate::{server::validate_server_address, service::Connection};

#[derive(Default)]
pub(super) enum SignIn {
    #[default]
    Idle,
    Invalid {
        address: Option<String>,
        username: Option<&'static str>,
    },
    Connecting,
    Failed(String),
}

impl SignIn {
    pub(super) fn begin(&mut self, address: &str, username: &str) -> bool {
        if self.is_connecting() {
            return false;
        }
        let address = validate_server_address(address)
            .err()
            .map(|error| error.to_string());
        let username = username.trim().is_empty().then_some("Enter your username.");
        if address.is_some() || username.is_some() {
            *self = Self::Invalid { address, username };
            return false;
        }
        *self = Self::Connecting;
        true
    }

    pub(super) fn field_errors(&self) -> (Option<&str>, Option<&str>) {
        match self {
            Self::Invalid { address, username } => (address.as_deref(), *username),
            _ => (None, None),
        }
    }

    pub(super) fn is_connecting(&self) -> bool {
        matches!(self, Self::Connecting)
    }

    pub(super) fn fail(&mut self, message: String) -> bool {
        if !self.is_connecting() {
            return false;
        }
        *self = Self::Failed(message);
        true
    }

    pub(super) fn error(&self) -> Option<&str> {
        match self {
            Self::Failed(message) => Some(message),
            _ => None,
        }
    }

    pub(super) fn dismiss_error(&mut self) {
        if self.error().is_some() {
            *self = Self::Idle;
        }
    }
}

#[derive(Default)]
pub(super) struct LibraryConnection {
    pub editing: bool,
    pub syncing: bool,
    next_check: u64,
    check: Option<(u64, Option<Result<(), String>>)>,
}

impl LibraryConnection {
    pub fn status(&self, connection: &Connection) -> &'static str {
        match connection {
            Connection::Online if self.syncing => "Syncing library…",
            Connection::Online => "Connected",
            Connection::Connecting => "Connecting…",
            Connection::Offline(_) | Connection::None => "Offline",
        }
    }

    pub fn check_pending(&self) -> bool {
        matches!(self.check, Some((_, None)))
    }

    pub fn begin_check(&mut self) -> Option<u64> {
        if self.check_pending() {
            return None;
        }
        self.next_check += 1;
        self.check = Some((self.next_check, None));
        Some(self.next_check)
    }

    pub fn complete_check(&mut self, id: u64, result: Result<(), String>) {
        if matches!(self.check, Some((current, None)) if current == id) {
            self.check = Some((id, Some(result)));
        }
    }

    pub fn check_feedback(&self) -> Option<(&str, bool)> {
        Some(match self.check.as_ref()?.1.as_ref() {
            None => ("Checking connection…", false),
            Some(Ok(())) => ("Connection successful.", false),
            Some(Err(reason)) => (reason, true),
        })
    }

    pub fn account_changed(&mut self) {
        self.editing = false;
        self.check = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_active_attempt_can_fail_and_retry_clears_its_error() {
        let mut sign_in = SignIn::default();
        assert!(!sign_in.fail("stale".into()));
        assert!(sign_in.begin("https://music.example.com", "listener"));
        assert!(!sign_in.begin("https://music.example.com", "listener"));
        assert!(sign_in.fail("first failure".into()));
        assert_eq!(sign_in.error(), Some("first failure"));
        assert!(sign_in.begin("https://music.example.com", "listener"));
        assert_eq!(sign_in.error(), None);
        sign_in.dismiss_error();
        assert!(sign_in.is_connecting());
        assert!(sign_in.fail("second failure".into()));
        sign_in.dismiss_error();
        assert_eq!(sign_in.error(), None);
    }

    #[test]
    fn reports_both_invalid_fields_and_clears_them_on_valid_submit() {
        let mut sign_in = SignIn::default();
        assert!(!sign_in.begin("ftp://music.example.com", " "));
        let (address, username) = sign_in.field_errors();
        assert!(address.is_some());
        assert!(username.is_some());
        assert!(sign_in.begin("http://localhost:4533", "demo"));
        assert_eq!(sign_in.field_errors(), (None, None));
    }

    #[test]
    fn a_saved_server_check_ignores_stale_results() {
        let mut connection = LibraryConnection::default();
        let stale = connection.begin_check().unwrap();
        assert!(connection.begin_check().is_none());
        connection.account_changed();
        let active = connection.begin_check().unwrap();
        connection.complete_check(stale, Ok(()));
        assert!(connection.check_pending());
        connection.complete_check(active, Err("Unreachable".into()));
        assert_eq!(connection.check_feedback(), Some(("Unreachable", true)));
    }
}
