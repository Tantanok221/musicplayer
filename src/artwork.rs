//! Pending artwork survives reconnects; failed transfers have a bounded retry policy.
//! Covers on screen go first, then their neighbors, newest first: what the user is
//! looking at, or about to scroll to, matters more than covers already scrolled past.
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const MAX_IN_FLIGHT: usize = 8;
const MAX_ATTEMPTS: u8 = 3;
/// Head start of an on-screen cover over a neighbor requested at the same moment.
/// Older on-screen requests fall behind fresh neighbors once the user has moved on.
const VISIBLE_LEAD: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// On screen now.
    Visible,
    /// Just outside the viewport, so it is ready before it scrolls in.
    Nearby,
}
struct Request {
    attempts: u8,
    in_flight: bool,
    ready_at: Instant,
    /// Higher downloads sooner.
    rank: Instant,
}
#[derive(Default)]
pub struct ArtworkQueue {
    requests: HashMap<String, Request>,
}
impl ArtworkQueue {
    /// Queues `id`, or raises the rank of a request that is already waiting.
    pub fn request(&mut self, id: String, now: Instant, priority: Priority) {
        if id.is_empty() {
            return;
        }
        let rank = match priority {
            Priority::Visible => now + VISIBLE_LEAD,
            Priority::Nearby => now,
        };
        let request = self.requests.entry(id).or_insert(Request {
            attempts: 0,
            in_flight: false,
            ready_at: now,
            rank,
        });
        request.rank = request.rank.max(rank);
    }
    pub fn take_ready(&mut self, now: Instant) -> Vec<String> {
        let active = self
            .requests
            .values()
            .filter(|request| request.in_flight)
            .count();
        let remaining = MAX_IN_FLIGHT.saturating_sub(active);
        if remaining == 0 {
            return Vec::new();
        }
        let mut waiting = self
            .requests
            .iter()
            .filter(|(_, request)| {
                !request.in_flight && request.ready_at <= now && request.attempts < MAX_ATTEMPTS
            })
            .map(|(id, request)| (request.rank, id.clone()))
            .collect::<Vec<_>>();
        waiting.sort_unstable_by(|a, b| b.cmp(a));
        waiting.truncate(remaining);
        for (_, id) in &waiting {
            if let Some(request) = self.requests.get_mut(id) {
                request.in_flight = true;
                request.attempts += 1;
            }
        }
        waiting.into_iter().map(|(_, id)| id).collect()
    }
    /// Returns true when the automatic retry budget has been exhausted.
    pub fn finished(&mut self, id: &str, success: bool, now: Instant) -> bool {
        if success {
            self.requests.remove(id);
            return false;
        }
        let Some(request) = self.requests.get_mut(id) else {
            return false;
        };
        request.in_flight = false;
        if request.attempts >= MAX_ATTEMPTS {
            return true;
        }
        request.ready_at = now + Duration::from_secs(1 << request.attempts);
        false
    }
    pub fn next_retry(&self, now: Instant) -> Option<Duration> {
        if self
            .requests
            .values()
            .filter(|request| request.in_flight)
            .count()
            >= MAX_IN_FLIGHT
        {
            return None;
        }
        self.requests
            .values()
            .filter(|request| !request.in_flight && request.attempts < MAX_ATTEMPTS)
            .map(|request| request.ready_at.saturating_duration_since(now))
            .min()
    }
    pub fn retry_failed(&mut self, now: Instant) {
        for request in self.requests.values_mut() {
            if !request.in_flight {
                request.attempts = 0;
                request.ready_at = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_queue_has_no_timer_and_backoff_uses_its_deadline() {
        let mut queue = ArtworkQueue::default();
        let now = Instant::now();
        assert_eq!(queue.next_retry(now), None);
        queue.request("cover".into(), now, Priority::Visible);
        assert_eq!(queue.next_retry(now), Some(Duration::ZERO));
        queue.take_ready(now);
        assert_eq!(queue.next_retry(now), None);
        queue.finished("cover", false, now);
        assert_eq!(queue.next_retry(now), Some(Duration::from_secs(2)));
        queue.take_ready(now + Duration::from_secs(2));
        queue.finished("cover", true, now);
        assert_eq!(queue.next_retry(now), None);
    }
    #[test]
    fn pending_requests_survive_the_connection_wait_and_are_deduplicated() {
        let mut queue = ArtworkQueue::default();
        let now = Instant::now();
        queue.request("cover".into(), now, Priority::Visible);
        queue.request("cover".into(), now, Priority::Visible);
        // No work is dispatched while disconnected. The first connected pump sees it.
        assert_eq!(queue.take_ready(now + Duration::from_secs(30)), ["cover"]);
        assert!(queue.take_ready(now + Duration::from_secs(30)).is_empty());
        assert!(!queue.finished("cover", true, now));
        assert!(queue.take_ready(now + Duration::from_secs(60)).is_empty());
    }
    #[test]
    fn failed_downloads_retry_then_refresh_can_retry_exhausted_requests() {
        let mut queue = ArtworkQueue::default();
        let now = Instant::now();
        queue.request("cover".into(), now, Priority::Visible);
        assert_eq!(queue.take_ready(now), ["cover"]);
        assert!(!queue.finished("cover", false, now));
        assert!(queue.take_ready(now + Duration::from_secs(1)).is_empty());
        assert_eq!(queue.take_ready(now + Duration::from_secs(2)), ["cover"]);
        assert!(!queue.finished("cover", false, now + Duration::from_secs(2)));
        assert_eq!(queue.take_ready(now + Duration::from_secs(6)), ["cover"]);
        assert!(queue.finished("cover", false, now + Duration::from_secs(6)));
        assert!(queue.take_ready(now + Duration::from_secs(60)).is_empty());
        queue.retry_failed(now + Duration::from_secs(60));
        assert_eq!(queue.take_ready(now + Duration::from_secs(60)), ["cover"]);
    }
    #[test]
    fn only_eight_downloads_run_at_once() {
        let mut queue = ArtworkQueue::default();
        let now = Instant::now();
        for i in 0..10 {
            queue.request(i.to_string(), now, Priority::Visible);
        }
        let first = queue.take_ready(now);
        assert_eq!(first.len(), 8);
        assert!(queue.take_ready(now).is_empty());
        queue.finished(&first[0], true, now);
        assert_eq!(queue.take_ready(now).len(), 1);
    }
    #[test]
    fn newest_and_revisited_covers_download_first() {
        let mut queue = ArtworkQueue::default();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        for (ms, id) in [(0, "scrolled-past"), (10, "middle"), (20, "on-screen")] {
            queue.request(id.into(), at(ms), Priority::Visible);
        }
        // Seeing an older cover again moves it ahead of the rest.
        queue.request("scrolled-past".into(), at(30), Priority::Visible);
        assert_eq!(
            queue.take_ready(at(30)),
            ["scrolled-past", "on-screen", "middle"]
        );
    }
    #[test]
    fn on_screen_covers_beat_neighbors_but_stale_ones_do_not() {
        let mut queue = ArtworkQueue::default();
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        queue.request("long-gone".into(), at(0), Priority::Visible);
        queue.request("on-screen".into(), at(5_000), Priority::Visible);
        queue.request("below".into(), at(5_000), Priority::Nearby);
        assert_eq!(
            queue.take_ready(at(5_000)),
            ["on-screen", "below", "long-gone"]
        );
    }
}
