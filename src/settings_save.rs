#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveOutcome {
    Saved(&'static str),
    Failed,
}

#[derive(Default)]
pub(crate) struct SaveFeedback {
    latest: u64,
    visible: Option<(u64, SaveOutcome)>,
}

impl SaveFeedback {
    pub(crate) fn begin(&mut self) -> u64 {
        self.latest += 1;
        self.latest
    }

    pub(crate) fn complete(&mut self, request: u64, outcome: SaveOutcome) -> bool {
        if request != self.latest {
            return false;
        }
        self.visible = Some((request, outcome));
        true
    }

    pub(crate) fn expire(&mut self, request: u64) -> bool {
        if self.visible.is_some_and(|(shown, _)| shown == request) {
            self.visible = None;
            return true;
        }
        false
    }

    pub(crate) fn outcome(&self) -> Option<SaveOutcome> {
        self.visible.map(|(_, outcome)| outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_save_replaces_the_outcome_and_owns_its_expiry() {
        let mut feedback = SaveFeedback::default();
        let first = feedback.begin();
        assert_eq!(feedback.outcome(), None);
        assert!(feedback.complete(first, SaveOutcome::Saved("Settings saved")));
        let second = feedback.begin();
        assert!(feedback.complete(second, SaveOutcome::Failed));
        assert!(!feedback.expire(first));
        assert_eq!(feedback.outcome(), Some(SaveOutcome::Failed));
        assert!(feedback.expire(second));
        assert_eq!(feedback.outcome(), None);
    }

    #[test]
    fn a_stale_completion_does_not_show_a_toast() {
        let mut feedback = SaveFeedback::default();
        let first = feedback.begin();
        let second = feedback.begin();
        assert!(!feedback.complete(first, SaveOutcome::Saved("Settings saved")));
        assert_eq!(feedback.outcome(), None);
        let saved = SaveOutcome::Saved("Music folder saved · verified");
        assert!(feedback.complete(second, saved));
        assert_eq!(feedback.outcome(), Some(saved));
    }
}
