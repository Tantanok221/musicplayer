//! Real actor → loopback HTTP → SQLite → production events.
#[path = "service_worker/feedback.rs"]
mod feedback;
#[path = "service_worker/fixture.rs"]
mod fixture;
#[path = "service_worker/lifecycle.rs"]
mod lifecycle;
#[path = "service_worker/metadata.rs"]
mod metadata;
#[path = "service_worker/preferences.rs"]
mod preferences;
#[path = "service_worker/sync.rs"]
mod sync;
