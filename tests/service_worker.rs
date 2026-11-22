//! Real actor → loopback HTTP → SQLite → production events.
#[path = "service_worker/accounts.rs"]
mod accounts;
#[path = "service_worker/artists.rs"]
mod artists;
#[path = "service_worker/artwork.rs"]
mod artwork;
#[path = "service_worker/connection.rs"]
mod connection;
#[path = "service_worker/discover.rs"]
mod discover;
#[path = "service_worker/feedback.rs"]
mod feedback;
#[path = "service_worker/fixture.rs"]
mod fixture;
#[path = "service_worker/lifecycle.rs"]
mod lifecycle;
#[path = "service_worker/metadata.rs"]
mod metadata;
#[path = "service_worker/playlists.rs"]
mod playlists;
#[path = "service_worker/preferences.rs"]
mod preferences;
#[path = "service_worker/queue.rs"]
mod queue;
#[path = "service_worker/scrobbles.rs"]
mod scrobbles;
#[path = "service_worker/sync.rs"]
mod sync;
