//! Pending artwork survives reconnects; failed transfers have a bounded retry policy.
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

const MAX_IN_FLIGHT: usize = 4;
const MAX_ATTEMPTS: u8 = 3;
struct Request {
    attempts: u8,
    in_flight: bool,
    ready_at: Instant,
}
#[derive(Default)]
pub struct ArtworkQueue {
    requests: HashMap<String, Request>,
    pending: VecDeque<String>,
}
impl ArtworkQueue {
    pub fn request(&mut self, id: String, now: Instant) {
        if id.is_empty() || self.requests.contains_key(&id) {
            return;
        }
        self.requests.insert(
            id.clone(),
            Request {
                attempts: 0,
                in_flight: false,
                ready_at: now,
            },
        );
        self.pending.push_back(id);
    }
    pub fn take_ready(&mut self, now: Instant) -> Vec<String> {
        let active = self
            .requests
            .values()
            .filter(|request| request.in_flight)
            .count();
        let mut remaining = MAX_IN_FLIGHT.saturating_sub(active);
        let mut ready = Vec::new();
        for _ in 0..self.pending.len() {
            let id = self.pending.pop_front().unwrap();
            let Some(request) = self.requests.get_mut(&id) else {
                continue;
            };
            if remaining > 0 && request.ready_at <= now && request.attempts < MAX_ATTEMPTS {
                request.in_flight = true;
                request.attempts += 1;
                remaining -= 1;
                ready.push(id);
            } else {
                self.pending.push_back(id);
            }
        }
        ready
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
        self.pending.push_back(id.into());
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
        self.pending
            .iter()
            .filter_map(|id| self.requests.get(id))
            .filter(|request| !request.in_flight && request.attempts < MAX_ATTEMPTS)
            .map(|request| request.ready_at.saturating_duration_since(now))
            .min()
    }
    pub fn retry_failed(&mut self, now: Instant) {
        for (id, request) in &mut self.requests {
            if !request.in_flight {
                request.attempts = 0;
                request.ready_at = now;
                if !self.pending.contains(id) {
                    self.pending.push_back(id.clone());
                }
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
        queue.request("cover".into(), now);
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
        queue.request("cover".into(), now);
        queue.request("cover".into(), now);
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
        queue.request("cover".into(), now);
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
    fn only_four_downloads_run_at_once() {
        let mut queue = ArtworkQueue::default();
        let now = Instant::now();
        for i in 0..10 {
            queue.request(i.to_string(), now);
        }
        let first = queue.take_ready(now);
        assert_eq!(first.len(), 4);
        assert!(queue.take_ready(now).is_empty());
        queue.finished(&first[0], true, now);
        assert_eq!(queue.take_ready(now).len(), 1);
    }
}
