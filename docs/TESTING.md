# Testing guide for agents

## Commands

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features test-support -- -D warnings
cargo test --locked --features test-support
scripts/test-ui.sh
scripts/test-service.sh
```

`cargo test --locked --features test-support` runs everything: unit tests,
ordinary integration tests, and the feature-gated `ui`, `service_worker` and
`player_queue` targets. Plain `cargo test` skips those three targets and is not
a substitute before pushing.

`scripts/test-ui.sh` and `scripts/test-service.sh` run one target, accept a test
filter (`scripts/test-ui.sh playlists::`, add `--exact` for one scenario), save
`test.log` and `environment.txt` under `target/{ui,service}-test-artifacts/`,
and fail when no test ran. `scripts/test-service-repeat.sh [runs]` repeats the
worker suite to expose flaky timing.

## Feature map

| Feature | UI code (`src/ui/`) | Pure logic | Tests |
| --- | --- | --- | --- |
| Tabs, Back/Forward | `tabs.rs`, `navigation.rs` | `browser::{Tabs, History}` | `tests/ui/tabs.rs`, `tests/browser.rs` |
| Music Explorer, navigator, sorting, cover settling | `library.rs`, `navigator.rs`, `sorting_controls.rs` | `catalog`, `sorting`, `browser`, `ui::library::ArtSettle` | `tests/ui/{explorer,large_library}.rs`, `tests/{sorting,catalog,navigator}.rs`, `ui::library` unit tests |
| Album and song favorites/ratings | `feedback.rs`, `album_feedback.rs` | `feedback::{Feedback, optimistic_*}` | `tests/ui/{album_feedback,song_feedback,worker}.rs`, `tests/service_worker/feedback.rs`, `feedback` unit tests |
| Artist pages, albums | `artist_page.rs`, `albums.rs` | `artists`, `discography` | `tests/ui/artists.rs`, `tests/service_worker/{artists,metadata}.rs` |
| Discover (Memory lane, Last.fm history) | `discover.rs`, `settings.rs` | `discover` (`rediscover`, `stale_seasons`, `Visit`), `lastfm`, `Catalog::rediscover_candidates` | `tests/ui/discover.rs`, `tests/ui/settings.rs`, `tests/service_worker/discover.rs`, `tests/discover.rs` |
| Missing releases, downloads | `downloads.rs` | `slskd` | `tests/ui/downloads.rs`, `tests/slskd.rs` |
| Queue and playback controls | `queue_controls.rs`, `tracks.rs`, `player_bar.rs` | `playback`, `playback::queue` | `tests/ui/playback.rs`, `tests/player_queue.rs`, `tests/service_worker/queue.rs` |
| Now Playing, lyrics, backdrop | `now_playing.rs`, `information.rs` | `lyrics`, `backdrop`, `visualizer` | `tests/ui/{lyrics,backdrop}.rs`, `tests/service_worker/artwork.rs` |
| Playlists | `playlists.rs` | `ui::playlists::PlaylistEntry` | `tests/ui/playlists.rs`, `tests/service_worker/playlists.rs` |
| Sign-in, slskd settings, accounts, connection | `settings.rs`, `events.rs`, `chrome.rs` | `platform` | `tests/ui/{settings,worker}.rs`, `tests/service_worker/{accounts,connection}.rs` |
| Light/dark theme | `settings.rs` | | `tests/ui/appearance.rs` |
| Volume, mute, shuffle, repeat | `player_bar.rs` | | `tests/ui/playback.rs`, `tests/service_worker/preferences.rs` |
| Panel layout, window size and position | `settings.rs`, `window.rs` | `ui::restored_window_bounds` | `tests/ui/layout.rs`, `tests/service_worker/preferences.rs` |
| Service events reaching the view | `events.rs` | | all UI scenarios |
| Sync, scrobbles, shutdown | | `service`, `catalog`, `listening` | `tests/service_worker/{sync,scrobbles,lifecycle}.rs` |

`src/ui.rs` holds only the `MusicApp` struct, `new()`, actions and `Render`.

## Test layers

| Layer | Target and root | Modules | Fixture |
| --- | --- | --- | --- |
| Production view, controlled service | `ui`: `tests/ui.rs` | `tests/ui/<feature>.rs` | `tests/common/ui/` |
| Production view, real worker and loopback HTTP | `ui`: `tests/ui.rs` | `tests/ui/worker.rs` | `tests/service_worker/fixture.rs` |
| Real service worker, loopback HTTP, SQLite | `service_worker`: `tests/service_worker.rs` | `tests/service_worker/<feature>.rs` | `tests/service_worker/fixture.rs` |
| Real paused player actor | `player_queue`: `tests/player_queue.rs` | single file | `playback::test_support` |
| Unit and ordinary integration tests | `#[cfg(test)]` modules and `tests/*.rs` | per file | per file |

Rules:

- Target roots only declare modules. Put scenarios in the feature module; add a
  new module (`#[path = "<target>/<feature>.rs"]`) and a feature map row for a
  new feature. Do not add a `[[test]]` target per feature.
- The module name is the filter. Refer to scenarios as `module::name`.
- Pure state and transition logic belongs outside the view, with unit tests that
  include failure and edge cases. Interaction and wiring belong in UI or worker
  scenarios. Use both when a feature has logic and interaction.

## Writing UI scenarios

- Start with `common::ui::start(cx)`. It mounts the production `MusicApp` with a
  `ControlledService`, an isolated SQLite catalog and a small seeded library.
- Drive real input: `fixture.click`, `press`, `search`, `menu_item`, or
  `window.input`/scroll. Never call `MusicApp::event` or click callbacks
  directly, and never assign the model field the feature should update.
- Assert visible state (labels, selected/disabled state, geometry) together with
  the emitted command or persisted result. `tab_state()` is a read-only helper.
- Answer commands with `Fixture::expect(label, |command, catalog| events)` and
  finish with `assert_script_finished()`. Unexpected commands fail the scenario.
  Keep shared defaults in `pump()` minimal.
- Held responses: `hold_queries`/`deliver_query` for stale results,
  `confirm_mutation`/`fail_mutation` for album feedback, `capture_playback` for
  play intents. Emit `Event::Playback` to drive playback snapshots.
- Use the bounded `settle` pump and `cx.executor().advance_clock`, never sleeps.
- Targets use domain IDs (`("workspace-tab", id)`, `album-favorite-{id}`,
  `navigator-facet-{mode}-{id}`), registered with `.test_support()`.

## Writing worker scenarios

- `Fixture::new(Library { .. })` starts a strict loopback HTTP server with
  isolated paths. `seed()` pre-fills the catalog; `start()` or `start_paused()`
  runs the real worker.
- Declare mutations with `expect_mutation(request, succeeds)`, wait with
  `wait_request` and the `event`/`synced`/`view` helpers, and end with
  `worker.shutdown(WAIT)` and `fixture.finish()`, which fails on unexpected or
  unconsumed requests.
- All waits use deadlines on real signals.

## Diagnostics and boundaries

Never Debug-dump `Command` values or authenticated URLs; some carry credentials.
Fixture strings stay synthetic and logged fields stay allowlisted.

These suites run in process. They do not cover the packaged app, native
accessibility, pixels or physical audio. Backdrop scenarios check image-layer
identity, geometry and fallback, not blur or animation. The GPUI test platform
cannot inject native window moves, so window-position coverage starts from a
nonzero origin instead of dragging. Say so in review evidence when a change
depends on those.
