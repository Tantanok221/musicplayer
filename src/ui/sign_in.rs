#[derive(Default)]
pub(super) enum SignIn {
    #[default]
    Idle,
    Connecting,
    Failed(String),
}

impl SignIn {
    pub(super) fn begin(&mut self) -> bool {
        if self.is_connecting() {
            return false;
        }
        *self = Self::Connecting;
        true
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

#[cfg(test)]
mod tests {
    use super::SignIn;

    #[test]
    fn only_an_active_attempt_can_fail_and_retry_clears_its_error() {
        let mut sign_in = SignIn::default();
        assert!(!sign_in.fail("stale".into()));
        assert!(sign_in.begin());
        assert!(!sign_in.begin());
        assert!(sign_in.fail("first failure".into()));
        assert_eq!(sign_in.error(), Some("first failure"));
        assert!(sign_in.begin());
        assert_eq!(sign_in.error(), None);
        sign_in.dismiss_error();
        assert!(sign_in.is_connecting());
        assert!(sign_in.fail("second failure".into()));
        sign_in.dismiss_error();
        assert_eq!(sign_in.error(), None);
    }
}
