//! Listening instances are independent of decoder epochs and network attempts.
use crate::model::Track;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListenSession {
    pub id: u64,
    pub account: String,
    pub track_id: String,
    pub started_ms: i64,
    pub played_seconds: f64,
    pub submitted: bool,
    pub announced: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListenReport {
    pub id: u64,
    pub account: String,
    pub track_id: String,
    pub started_ms: i64,
    pub submission: bool,
}
#[derive(Clone)]
pub(crate) struct AudibleListen {
    pub id: u64,
    pub account: String,
    pub track: Arc<Track>,
}
pub(crate) struct AudioProgress {
    pub listen: Arc<AudibleListen>,
    pub seconds: f64,
    pub started_ms: i64,
    pub epoch: u64,
}
#[derive(Default)]
pub(crate) struct ListenTracker {
    sessions: HashMap<u64, ListenSession>,
}
impl ListenTracker {
    pub fn restore(&mut self, session: ListenSession) {
        self.sessions.insert(session.id, session);
    }
    pub fn session(&self, id: Option<u64>) -> Option<ListenSession> {
        id.and_then(|id| self.sessions.get(&id).cloned())
    }
    pub fn observe(&mut self, progress: AudioProgress) -> Vec<ListenReport> {
        let listen = progress.listen;
        let session = self
            .sessions
            .entry(listen.id)
            .or_insert_with(|| ListenSession {
                id: listen.id,
                account: listen.account.clone(),
                track_id: listen.track.id.clone(),
                started_ms: progress.started_ms,
                played_seconds: 0.,
                submitted: false,
                announced: false,
            });
        if progress.seconds.is_finite() && progress.seconds > 0. {
            session.played_seconds += progress.seconds;
        }
        let mut reports = Vec::new();
        if !session.announced {
            session.announced = true;
            reports.push(session.report(false));
        }
        if !session.submitted
            && listen.track.duration > 30.
            && session.played_seconds + 1e-8 >= (listen.track.duration / 2.).min(240.)
        {
            session.submitted = true;
            reports.push(session.report(true));
        }
        reports
    }
}
impl ListenSession {
    fn report(&self, submission: bool) -> ListenReport {
        ListenReport {
            id: self.id,
            account: self.account.clone(),
            track_id: self.track_id.clone(),
            started_ms: self.started_ms,
            submission,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn progress(id: u64, seconds: f64, duration: f64, epoch: u64) -> AudioProgress {
        AudioProgress {
            listen: Arc::new(AudibleListen {
                id,
                account: "fixture-account".into(),
                track: Arc::new(Track {
                    id: "track".into(),
                    duration,
                    ..Default::default()
                }),
            }),
            seconds,
            epoch,
            started_ms: 1700000000000 + epoch as i64,
        }
    }
    #[test]
    fn seeks_recovery_and_resume_keep_one_submission_and_original_timestamp() {
        let mut tracker = ListenTracker::default();
        assert_eq!(tracker.observe(progress(1, 20., 100., 1)).len(), 1);
        assert!(tracker.observe(progress(1, 20., 100., 2)).is_empty());
        let saved = tracker.session(Some(1)).unwrap();
        let json = serde_json::to_string(&saved).unwrap();
        let mut tracker = ListenTracker::default();
        tracker.restore(serde_json::from_str(&json).unwrap());
        let reports = tracker.observe(progress(1, 10., 100., 3));
        assert_eq!(reports.len(), 1);
        assert!(reports[0].submission);
        assert_eq!(reports[0].started_ms, 1700000000001);
        assert!(tracker.observe(progress(1, 70., 100., 4)).is_empty());
        let saved = tracker.session(Some(1)).unwrap();
        let mut restarted = ListenTracker::default();
        restarted.restore(saved);
        assert!(restarted.observe(progress(1, 100., 100., 5)).is_empty());
    }
    #[test]
    fn repeat_and_replayed_identical_tracks_are_distinct_listens() {
        let mut tracker = ListenTracker::default();
        for id in [1, 2, 3] {
            let reports = tracker.observe(progress(id, 50., 100., 1));
            assert_eq!(reports.len(), 2);
            assert!(!reports[0].submission);
            assert!(reports[1].submission);
            assert_eq!(reports[1].id, id);
        }
    }
    #[test]
    fn eligibility_uses_strict_minimum_and_half_or_four_minutes() {
        let mut tracker = ListenTracker::default();
        for (id, duration) in [(1, 0.), (2, 30.)] {
            assert!(
                tracker
                    .observe(progress(id, 300., duration, 1))
                    .iter()
                    .all(|report| !report.submission)
            );
        }
        assert!(
            tracker
                .observe(progress(3, 239., 1200., 1))
                .iter()
                .all(|report| !report.submission)
        );
        assert!(tracker.observe(progress(3, 1., 1200., 2))[0].submission);
        assert!(
            tracker
                .observe(progress(4, f64::NAN, 100., 1))
                .iter()
                .all(|report| !report.submission)
        );
        assert_eq!(tracker.session(Some(4)).unwrap().played_seconds, 0.);
    }
}
