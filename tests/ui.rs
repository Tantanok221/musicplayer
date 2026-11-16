//! UI integration scenarios using the production library view.
//! Run with `scripts/test-ui.sh` or `cargo test --features test-support --test ui`.
#[path = "ui/album_feedback.rs"]
mod album_feedback;
mod common;
#[path = "ui/downloads.rs"]
mod downloads;
#[path = "ui/explorer.rs"]
mod explorer;
#[path = "ui/layout.rs"]
mod layout;
#[path = "ui/lyrics.rs"]
mod lyrics;
#[path = "ui/playback.rs"]
mod playback;
#[path = "ui/playlists.rs"]
mod playlists;
#[path = "ui/tabs.rs"]
mod tabs;
#[path = "ui/worker.rs"]
mod worker;
// This target shares the strict HTTP fixture, not its worker-only scenarios.
#[allow(dead_code)]
#[path = "service_worker/fixture.rs"]
mod worker_fixture;
