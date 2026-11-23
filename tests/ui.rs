//! UI integration scenarios using the production library view.
//! Run with `scripts/test-ui.sh` or `cargo test --features test-support --test ui`.
#[path = "ui/album_feedback.rs"]
mod album_feedback;
mod common;
#[path = "ui/devices.rs"]
mod devices;
#[path = "ui/discover.rs"]
mod discover;
#[path = "ui/downloads.rs"]
mod downloads;
#[path = "ui/explorer.rs"]
mod explorer;
#[path = "ui/layout.rs"]
mod layout;
#[path = "ui/lyrics.rs"]
mod lyrics;
#[path = "ui/metadata.rs"]
mod metadata;
#[path = "ui/playback.rs"]
mod playback;
#[path = "ui/playlists.rs"]
mod playlists;
#[path = "ui/smart_playlists.rs"]
mod smart_playlists;
#[path = "ui/tabs.rs"]
mod tabs;
#[path = "ui/worker.rs"]
mod worker;
// This target shares the strict HTTP fixture, not its worker-only scenarios.
#[path = "ui/appearance.rs"]
mod appearance;
#[path = "ui/artists.rs"]
mod artists;
#[path = "ui/backdrop.rs"]
mod backdrop;
#[path = "ui/large_library.rs"]
mod large_library;
#[path = "ui/settings.rs"]
mod settings;
#[path = "ui/song_feedback.rs"]
mod song_feedback;
#[path = "ui/tag_editor.rs"]
mod tag_editor;
#[allow(dead_code)]
#[path = "service_worker/fixture.rs"]
mod worker_fixture;
