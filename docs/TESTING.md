# Agent verification guide

Implementation status and handoff notes: [AGENT_TESTING_PROGRESS.md](AGENT_TESTING_PROGRESS.md).
Broader roadmap: [AGENT_TESTING_GAPS.md](AGENT_TESTING_GAPS.md).

## Commands

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features test-support -- -D warnings
cargo test --locked
scripts/test-service.sh
scripts/test-ui.sh
# Occasional real-thread/runtime stability check (20 runs):
scripts/test-service-repeat.sh
```

### Does `cargo test` include integration tests?

Yes, but required features matter:

| Command | Tests executed |
| --- | --- |
| `cargo test` or `cargo test --locked` | Ordinary unit and integration tests, including the existing HTTP/catalog/audio tests in `tests/`; skips the feature-gated UI and real-worker targets |
| `cargo test --locked --features test-support` | All ordinary tests **plus** the UI and real-worker integration targets |
| `scripts/test-service.sh` | Real-worker integration only, with saved diagnostics |
| `scripts/test-ui.sh` | Production-view UI integration only, with saved diagnostics |

`--locked` controls dependency resolution, not test selection. `--all-targets`
does not enable missing features. Formatting and Clippy do not execute tests.
For a single command that runs every current test, use:

```sh
cargo test --locked --features test-support
```

The focused script runs the `tests/ui.rs` integration target with the
`test-support` feature. The equivalent Cargo command is
`cargo test --locked --features test-support --test ui -- --nocapture`.
It preserves Cargo's exit status while saving
`target/ui-test-artifacts/test.log` and `environment.txt`. CI runs the same
script on macOS and Linux and uploads these files. The progress document
records which commands have actually been validated and on which platform.
Both focused runners accept optional test filters after the script name.
For example, `scripts/test-ui.sh worker::` runs only the cross-layer UI scenario.

## First protected workflow

`new_tab_artist_stays_rightmost_and_survives_restart` mounts the production
`MusicApp`, using its normal event receiver and actual pointer/key handlers:

1. Start from the middle of four default tabs.
2. Click the New tab button and assert the active tab is appended.
3. Click an artist in the navigator. Assert the same tab ID, order, title,
   selected state and rightmost geometry.
4. Advance the GPUI clock to trigger the real debounced save. Handle the emitted
   Preferences command using the same persistence function as the service worker.
5. Close the window, drop the view/backend/catalog, reopen SQLite, and mount a
   fresh production view. Assert its restored artist tab and ordering.
6. Exercise tab creation through the application's shared keyboard bindings.

The SQLite database lives in a fresh temporary directory for each scenario.
The test starts no network, audio-device, or OS media-control services and
does not access a personal account/catalog. It verifies in-process UI
interactions/layout and database reload, not a new executable process, native
AX delivery, pixels, or physical playback. Those remain separate roadmap work.

## Navigation and album feedback coverage

Batch 1 comprises six scenarios. The five added after the original restart test are:

| Test | Behavior checked |
| --- | --- |
| `closing_and_reopening_restores_the_artist_tab_in_place` | Close an artist tab with a right neighbor; correct neighbor activates; keyboard reopen restores the complete tab state and position |
| `back_and_forward_histories_stay_with_each_tab` | Navigate between two artists, Back/Forward, switch tabs, and independently resume both histories |
| `album_favorite_is_optimistic_blocks_duplicates_and_confirms` | Optimistic favorite state, disabled pending controls, duplicate-click suppression, confirmation, and a subsequent unfavorite |
| `failed_album_favorite_rolls_back_and_retry_sends_the_same_intent` | Rollback to the catalog album, visible accessible error/Retry, exact retry intent, successful recovery |
| `album_rating_sets_and_clears_without_changing_song_rating` | Initial album/song rating distinction, optimistic rating, confirmed update, selected-star clearing to zero, unrelated data preserved |

Run one scenario independently by adding its name before `--`:

```sh
cargo test --locked --features test-support --test ui album_rating_sets_and_clears_without_changing_song_rating -- --exact --nocapture
```

The shared catalog contains two artists and three albums. Album and song
favorites/ratings deliberately differ. Mutation commands remain held until the
test calls `confirm_mutation()` or `fail_mutation()`. Confirmation updates real
SQLite through the catalog feedback API, then emits `AlbumUpdated` and
`MutationDone`; failure emits the original `AlbumUpdated` before
`MutationFailed`, matching the service's response order. Tests do not call the
view's mutation/event handlers directly. These are controlled-backend UI tests;
the real HTTP/service-worker mutation path is also exercised with the UI below.

## UI with a real service worker

`worker::favorite_http_failure_rolls_back_in_the_real_view_and_retry_recovers`
in `tests/ui/worker.rs` mounts the production view with a real
`service::test_support::Worker`, temporary SQLite paths and the strict loopback
HTTP fixture shared with `tests/service_worker.rs`. Actual pointer clicks select
an Explorer album, favorite it, attempt a duplicate while pending, and Retry
after HTTP rejection. Assertions cover the optimistic/disabled control, visible
rollback/error, one identical HTTP request on Retry, persisted success and
error dismissal. The HTTP fixture also answers the selection's real artist and
top-song requests with synthetic data.

The harness stages the worker's actual events in FIFO order and forwards each
through the view's normal receiver. It supplies no synthetic mutation responses
and does not call view handlers. Five-second event deadlines bound waits. This
connects the previously separate UI and worker layers: dropping the worker's
rollback event must fail the visible heart assertion. The worker/player endpoint
is silent; this is not packaged-process, OS input or audio-consumption coverage.

## Music Explorer coverage

Batch 2 adds five scenarios in `tests/ui/explorer.rs`, bringing the UI target to
eleven. These remain in Music Explorer and do not open artist pages:

| Test | Behavior checked |
| --- | --- |
| `explorer::unicode_search_empty_results_and_clear_recover_in_explorer` | Unicode input, visible matching albums, accessible empty state, and recovery through Clear search and filters |
| `explorer::genre_year_artist_filters_clear_without_changing_the_queue` | Genre, year and album-artist filters; selected controls; intersection and clearing; rendered queue order and no playback commands |
| `explorer::explorer_sort_and_direction_survive_catalog_reload` | Real preset/direction menu clicks, visible album geometry/order, persisted sorting and checked menu state after SQLite reload |
| `explorer::explorer_metadata_filters_and_back_restore_prior_queries` | Genre/year/record-label links, fresh filters replacing prior filters, Back restoring previous Explorer queries |
| `explorer::stale_search_results_cannot_replace_newer_explorer_results` | Deliver a newer catalog-backed result before an older result; visible albums, input value and active query retain the newer search |

Run this batch with:

```sh
cargo test --locked --features test-support --test ui explorer:: -- --nocapture
```

`seed_explorer()` enriches the existing three albums with predictable genre,
year and record-label metadata. Query responses still use real catalog queries.
`hold_queries()` plus `deliver_query(text)` control result order through the
normal UI event receiver. Tests assert observed album IDs, accessible names and
layout order, not just SQL output. Playback state is a controlled paused queue.

New targets: `music-search`, `explorer-album-{album_id}`,
`browser-value-{column_label}-{facet_id}`, `all-{column_label}`, metadata link
IDs, and `library-empty`. These scenarios cover the default Album Covers view
with the small fixture. Alternate Explorer layouts, large virtualized scrolling,
artist pages, native app processes and pixel checks remain separate work.

## Real service-worker coverage

Batch 3 adds six scenarios in `tests/service_worker.rs`, with strict HTTP and
catalog helpers in `tests/service_worker/fixture.rs`:

| Test | Behavior checked |
| --- | --- |
| `album_feedback_reaches_http_catalog_and_real_events_without_editing_tracks` | Album favorite/unfavorite, rating set/clear, exact `albumId` versus `id` HTTP parameters, SQLite/event agreement, unchanged songs and unrelated albums; separate song favorite/rating commands leave the album unchanged |
| `failed_album_feedback_publishes_rollback_before_failure_and_retry_repeats_http_intent` | Favorite and rating failures publish the original catalog album before `MutationFailed`; SQLite remains unchanged; resending the Retry intent makes exactly one further identical HTTP request |
| `interrupted_sync_keeps_cached_music_then_a_complete_refresh_reconciles` | A real later-page failure after 500 imports preserves cached songs/albums/playlists; a successful pass during scanning also preserves them; a later complete non-scanning pass reconciles and persists the result |
| `preferences_save_through_the_worker_and_restore_after_clean_restart` | The real Preferences handler writes SQLite and updates the player; a joined worker restarts on the same paths and restores preferences through Ready and the player endpoint |
| `worker_metadata_uses_injected_musicbrainz_and_listenbrainz_endpoints` | A worker command traverses the loopback Navidrome artist-ID lookup and both injected metadata endpoints, publishing the resulting discography |
| `shutdown_finishes_the_worker_and_runtime_with_metadata_still_pending` | Shutdown completes the queue-save handshake, joins the worker, and shuts down its private runtime even with an HTTP response held pending |

```sh
scripts/test-service.sh
# One exact scenario, still saving diagnostics:
scripts/test-service.sh interrupted_sync_keeps_cached_music_then_a_complete_refresh_reconciles --exact
# Equivalent Cargo target:
cargo test --locked --features test-support --test service_worker -- --nocapture
```

`service::test_support::Worker` runs the production catalog command loop, not
`ControlledService`. It accepts isolated paths and an injected `MusicBrainz`
client, and creates a silent player endpoint without audio devices or OS media
controls. Each worker owns a private Tokio runtime; its completion signal comes
after runtime teardown. Readiness, events, requests, and shutdown use explicit
signals with five-second test deadlines, not sleep-based waits. The controlled
player answers shutdown with an empty saved queue; these are not queue/playback
algorithm tests.

The loopback server validates methods/endpoints/parameters, rejects unexpected
requests, and checks every expected mutation was consumed. It records only
allowlisted request summaries and scenario-tagged event names, never auth query
parameters, credentials, arbitrary commands, or complete URLs. Every scenario
uses temporary data/cache directories and synthetic saved credentials, and joins
its HTTP server after the worker stops. A held metadata response also exercises
cleanup of an in-flight request.

The runner saves `target/service-test-artifacts/test.log` and `environment.txt`
(override the directory with `SONORA_SERVICE_ARTIFACT_DIR`). Like the UI runner,
it preserves failures and rejects zero-test runs. CI invokes it on macOS/Linux
and uploads its artifacts separately from UI diagnostics.

For occasional repeated runs of all six real-worker scenarios, use
`scripts/test-service-repeat.sh` (20 by default) or pass a positive run count.
Each repetition retains its own `test.log` and `environment.txt` under
`target/service-repeat-artifacts/run-N/`; override the root with
`SONORA_SERVICE_REPEAT_ARTIFACT_DIR`. The runner stops on the first failure and
keeps that run's diagnostics. CI checks both default and feature-enabled Clippy.

These tests verify the service/HTTP/database/event boundary. They do not drive
pointer/key handlers, render the UI, launch a packaged process, inspect pixels,
or play audio. The cross-layer UI scenario above covers the actual Retry button
and optimistic/rollback state against this worker; these worker-only scenarios
resend mutation intents directly.

## Add another feature scenario

- Put production-view integration scenarios in `tests/ui.rs` or child modules
  under `tests/ui/` (the Explorer module is an example). Put reusable fixtures in `tests/common/ui.rs`.
  `sonora::ui::MusicApp` is the same library view used by the binary; import it
  normally rather than including production source files through `#[path]`.
  Its state stays private. The `test-support` feature enables a read-only
  `tab_state()` reader for model assertions, alongside visible UI checks.
  Unit tests of private helpers can remain beside their implementation.
- Use `service::test_support::ControlledService` to obtain a production service
  handle and UI event receiver. Consume commands explicitly and emit results
  through `emit`; do not call `MusicApp::event` or click callbacks directly.
- `player_commands()` captures player commands without opening any device.
  Emit `Event::Playback` to control the UI's playback snapshot. This does not
  simulate decoding or the audio clock; test those in the playback layer.
- Add only the fixture responses required by the scenario. Unexpected commands
  fail instead of reaching real services. The current fixture explicitly defers
  optional artist enrichment and uses real catalog queries for its responses.
- Locate existing domain-derived control IDs. Custom identified elements use
  `.test_support()`; this becomes an identity/no-op in ordinary builds. Native
  properties supply labels and state without a second test-only state model.
- Tab targets use `("workspace-tab", tab_id)`. Navigator facets use
  `navigator-facet-{mode.label()}-{facet.id}`; IDs do not depend on row position.
- Drive `window.click`, `press`, `input`, or scrolling. Pair UI state/geometry
  assertions with application results. Do not replace the action with direct
  assignments to the model field that the feature is supposed to update.
- Complete frames after external updates and leave window borrows before
  running queued tasks. Use the bounded fixture pump and explicit clock
  advancement, not wall-clock sleeps. Add readiness conditions when a future
  fixture introduces a real worker or HTTP server.

## Diagnostics and review

The fixture logs scenario names, stages, tab IDs, active/selected/disabled state,
labels, geometry, visibility, held album mutation counts, preference-write count,
and an allowlisted command history. GPUI
missing-target errors also include observed paths. Add relevant domain
diagnostics before a new scenario's critical assertions.

Never Debug-dump `Command` or authenticated request URLs: some variants carry
credentials. Keep fixture strings synthetic and artifact fields allowlisted.
Screenshots are not produced by this interaction runner.

A useful UI fix demonstrates a failing interaction assertion before the fix
and a passing scenario afterward. State the actual test layer in review
evidence. Do not describe controlled-backend tests as full native end-to-end
coverage or database reload as an executable relaunch.
