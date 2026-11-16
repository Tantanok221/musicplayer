# Agent verification guide

Implementation status and handoff notes: [AGENT_TESTING_PROGRESS.md](AGENT_TESTING_PROGRESS.md).
Broader roadmap: [AGENT_TESTING_GAPS.md](AGENT_TESTING_GAPS.md).

## Commands

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features test-support -- -D warnings
cargo test --locked
cargo test --locked --features test-support
scripts/test-service.sh
scripts/test-ui.sh
# Occasional real-thread/runtime stability check (20 runs):
scripts/test-service-repeat.sh
```

### Does `cargo test` include integration tests?

Yes, but required features matter:

| Command | Tests executed |
| --- | --- |
| `cargo test` or `cargo test --locked` | Ordinary unit and integration tests, including the existing HTTP/catalog/audio tests in `tests/`; skips the feature-gated UI, real-worker and paused-player targets |
| `cargo test --locked --features test-support` | All ordinary tests **plus** the UI, real-worker and paused-player integration targets |
| `cargo test --locked --features test-support --test player_queue` | Real paused-player queue integration only |
| `scripts/test-service.sh` | Real-worker integration only, with saved diagnostics |
| `scripts/test-ui.sh` | Production-view UI integration only, with saved diagnostics |

`--locked` controls dependency resolution, not test selection. `--all-targets`
does not enable missing features. Formatting and Clippy do not execute tests.
For a single command that runs every current test, use:

```sh
cargo test --locked --features test-support
```

The focused script runs the `ui` integration target (`tests/ui.rs` and its modules) with the
`test-support` feature. The equivalent Cargo command is
`cargo test --locked --features test-support --test ui -- --nocapture`.
It preserves Cargo's exit status while saving
`target/ui-test-artifacts/test.log` and `environment.txt`. CI runs the same
script on macOS and Linux and uploads these files. The progress document
records which commands have actually been validated and on which platform.
Both focused runners accept optional test filters after the script name.
For example, `scripts/test-ui.sh worker::` runs only the cross-layer UI scenario.

## Test layout

Integration tests are split by **layer** (Cargo target) and then by **feature**
(module). Each module name is also its filter prefix, so an agent working on a
feature runs only that feature's scenarios:

```sh
scripts/test-ui.sh tabs::
scripts/test-service.sh sync::
cargo test --locked --features test-support --test ui album_feedback:: -- --nocapture
```

| Layer | Target and root | Feature modules | Shared fixture |
| --- | --- | --- | --- |
| Production view, controlled service | `ui`: `tests/ui.rs` | `tests/ui/{tabs,album_feedback,explorer,playback,playlists,layout,downloads,lyrics}.rs` | `tests/common/ui/` |
| Production view, real worker + loopback HTTP | `ui`: `tests/ui.rs` | `tests/ui/worker.rs` | `tests/service_worker/fixture.rs` |
| Real service worker + loopback HTTP + SQLite | `service_worker`: `tests/service_worker.rs` | `tests/service_worker/{feedback,sync,preferences,metadata,lifecycle}.rs` | `tests/service_worker/fixture.rs` |
| Real paused player actor | `player_queue`: `tests/player_queue.rs` | single file | `playback::test_support` |
| Ordinary domain/HTTP/catalog tests | one target per `tests/*.rs` | n/a | per file |

Rules:

- Target roots (`tests/ui.rs`, `tests/service_worker.rs`) only declare modules.
  Never put scenarios in them.
- Add a feature's scenarios to its existing module. A new feature gets a new
  module, declared in the root with `#[path = "<target>/<feature>.rs"]`, and a
  row in this table. Do not add new `[[test]]` targets for a feature: each target
  is a separate binary that links the whole app and GPUI.
- The UI fixture is split by domain: `mod.rs` (setup, `pump()` dispatch,
  `settle`, diagnostics), `seed.rs`, `input.rs`, `library.rs`, `feedback.rs`,
  `playback.rs` and `tabs.rs`. `pump()` arms only delegate to domain methods.
  Prefer scenario-local `Fixture::expect` replies over new shared defaults.
- Scenario names describe the behavior and outcome. Refer to a scenario by its
  full `module::name` in docs and review evidence.

## First protected workflow

`tabs::new_tab_artist_stays_rightmost_and_survives_restart` mounts the production
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
| `tabs::closing_and_reopening_restores_the_artist_tab_in_place` | Close an artist tab with a right neighbor; correct neighbor activates; keyboard reopen restores the complete tab state and position |
| `tabs::back_and_forward_histories_stay_with_each_tab` | Navigate between two artists, Back/Forward, switch tabs, and independently resume both histories |
| `album_feedback::album_favorite_is_optimistic_blocks_duplicates_and_confirms` | Optimistic favorite state, disabled pending controls, duplicate-click suppression, confirmation, and a subsequent unfavorite |
| `album_feedback::failed_album_favorite_rolls_back_and_retry_sends_the_same_intent` | Rollback to the catalog album, visible accessible error/Retry, exact retry intent, successful recovery |
| `album_feedback::album_rating_sets_and_clears_without_changing_song_rating` | Initial album/song rating distinction, optimistic rating, confirmed update, selected-star clearing to zero, unrelated data preserved |

Run one scenario independently by adding its name before `--`:

```sh
cargo test --locked --features test-support --test ui album_feedback::album_rating_sets_and_clears_without_changing_song_rating -- --exact --nocapture
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
HTTP fixture shared with the `service_worker` target. Actual pointer clicks select
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

Batch 3 adds six scenarios in the `service_worker` target, one module per
feature under `tests/service_worker/`, with strict HTTP and catalog helpers in
`tests/service_worker/fixture.rs`:

| Test | Behavior checked |
| --- | --- |
| `feedback::album_feedback_reaches_http_catalog_and_real_events_without_editing_tracks` | Album favorite/unfavorite, rating set/clear, exact `albumId` versus `id` HTTP parameters, SQLite/event agreement, unchanged songs and unrelated albums; separate song favorite/rating commands leave the album unchanged |
| `feedback::failed_album_feedback_publishes_rollback_before_failure_and_retry_repeats_http_intent` | Favorite and rating failures publish the original catalog album before `MutationFailed`; SQLite remains unchanged; resending the Retry intent makes exactly one further identical HTTP request |
| `sync::interrupted_sync_keeps_cached_music_then_a_complete_refresh_reconciles` | A real later-page failure after 500 imports preserves cached songs/albums/playlists; a successful pass during scanning also preserves them; a later complete non-scanning pass reconciles and persists the result |
| `preferences::preferences_save_through_the_worker_and_restore_after_clean_restart` | The real Preferences handler writes SQLite and updates the player; a joined worker restarts on the same paths and restores preferences through Ready and the player endpoint |
| `metadata::worker_metadata_uses_injected_musicbrainz_and_listenbrainz_endpoints` | A worker command traverses the loopback Navidrome artist-ID lookup and both injected metadata endpoints, publishing the resulting discography |
| `lifecycle::shutdown_finishes_the_worker_and_runtime_with_metadata_still_pending` | Shutdown completes the queue-save handshake, joins the worker, and shuts down its private runtime even with an HTTP response held pending |

```sh
scripts/test-service.sh
# One exact scenario, still saving diagnostics:
scripts/test-service.sh sync::interrupted_sync_keeps_cached_music_then_a_complete_refresh_reconciles --exact
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

## Queue and playback controls

Batch 4 adds five scenarios in `tests/ui/playback.rs`. Together with the
cross-layer worker scenario, the merged UI target contains seventeen scenarios:

| Test | Behavior checked |
| --- | --- |
| `playback::album_play_next_and_queue_last_send_one_intended_command_each` | Actual album context-menu commands emit the intended album ID, Next/Last placement, and exactly one service intent per click; the unacknowledged queue remains unchanged |
| `playback::duplicate_queue_occurrences_select_move_remove_and_undo_through_real_player` | Duplicate occurrence selection, keyboard move, nested Remove, one-level Undo and pointer drag; real player snapshots determine visible order and retained selection |
| `playback::obsolete_queue_menu_edits_and_snapshots_cannot_claim_success` | An old rendered menu emits its captured revision while the actor has advanced; the real actor rejects the edit, and an older delayed snapshot cannot replace the current UI queue or Undo state |
| `playback::seek_drag_previews_survives_position_updates_and_sends_once_on_release` | Pointer-down/move preview, no held-drag Seek, elapsed-time preview and unchanged thumb through a position event, one release Seek, then normal position updates resume |
| `playback::space_types_in_search_but_toggles_once_with_queue_focus` | Space types into the focused Search input without a player command; with queue focus it emits exactly one Toggle and a delivered playing snapshot changes Play to Pause |

```sh
cargo test --locked --features test-support --test ui playback:: -- --nocapture
cargo test --locked --features test-support --test player_queue -- --nocapture
```

For one exact UI case, use its full `playback::...` name before `--exact`:

```sh
cargo test --locked --features test-support --test ui playback::obsolete_queue_menu_edits_and_snapshots_cannot_claim_success -- --exact --nocapture
```

`playback::test_support::PausedPlayer` runs the production player command loop
on a joined thread with a private runtime. Its platform adapter disables device
discovery and OS media controls. The endpoint accepts only paused queue commands,
rejecting transport/server commands, so it cannot start a decoder or audio
device. Readiness, snapshots and shutdown have bounded waits. Tests send
commands sequentially and consume real snapshots; the UI fixture contains no
second queue algorithm.

Two scenarios in `tests/player_queue.rs` cover duplicate occurrence IDs,
stale/current/past edit rejection, Undo, repeated Undo, Next/Last insertion,
clear-upcoming, position retention and metadata updates while Undo is pending.
The UI and binary now share playback and queue keyboard bindings. The original
keymap unit test remains. The stale-snapshot scenario exposed a real UI bug;
`Event::Playback` now rejects lower queue revisions.

Seek assertions use the existing slider's scoped track/thumb geometry and the
elapsed label derived from the same duration text that is rendered. These are
in-process observations, not numeric AX delivery, pixels or physical playback.
Existing decoder, PCM and consumed-audio tests remain separate evidence.
CI's feature-enabled all-tests command includes the paused-player target, while
the UI and worker scripts still retain their separate diagnostic artifacts.

## Playlists, layout, downloads and lyrics

Batch 5 adds nine scenarios, bringing the merged UI suite to 26. These are
controlled-backend UI tests: actual production input handlers and visible state,
with scenario-local service replies and real SQLite preference persistence.
They do not exercise playlist HTTP, slskd transfers, audio consumption or a
packaged app. The worker and paused-player targets remain separate checks.

| Test | Behavior checked |
| --- | --- |
| `playlists::create_failure_keeps_the_name_and_retry_creates_one_playlist` | Empty submission emits nothing; rejected Unicode name remains in the editor; actual Retry repeats the intent, displays the created playlist and clears the error/editor |
| `playlists::rename_and_add_an_album_use_the_selected_playlist` | Rename starts with the selected name, updates the visible sidebar, then a real nested album menu appends the intended album to that playlist and shows confirmation |
| `playlists::cancel_playlist_deletion_keeps_the_playlist_without_a_mutation` | Actual Cancel retains the playlist and emits no deletion |
| `playlists::confirmed_playlist_deletion_removes_the_sidebar_entry` | Confirm sends the exact playlist ID once and removes its visible sidebar entry after acknowledgement |
| `layout::resized_navigator_and_information_visibility_survive_catalog_reload` | Actual divider drag and menu visibility toggle persist to SQLite; a fresh view restores the measured divider position and hidden panel; the menu can show it again |
| `layout::narrow_window_tab_arrows_reach_both_ends_and_keep_new_tab_usable` | At 760px width, create many tabs, scroll to both ends, activate them and create another tab through a reachable New Tab control |
| `downloads::search_and_folder_failures_recover_without_marking_partial_downloads_complete` | Select a missing release; search failure retains the query; Retry gets candidates; folder refusal preserves choices; an alternative folder shows partial progress then failure rather than completion; dismissal restores folder selection |
| `downloads::completed_download_waits_for_library_scan_and_clear_sends_only_dismiss` | Completed transfer still says waiting for Navidrome; Refresh emits Refresh; Clear only dismisses that release |
| `lyrics::timed_lyrics_seek_only_the_playing_track_and_keep_selected_source_distinct` | Playing lyrics emit exactly one offset-adjusted Seek; Selected lyrics show another track and emit no Seek; switching back preserves the two sources |

Run a feature module independently, retaining diagnostics:

```sh
scripts/test-ui.sh playlists::
scripts/test-ui.sh layout::
scripts/test-ui.sh downloads::
scripts/test-ui.sh lyrics::
```

`Fixture::expect(label, response)` queues a one-shot scenario-local command
assertion and its replies. Unexpected commands still fail; the fixture never
Debug-dumps a command. Call `assert_script_finished()` to reject missing intents.
The shared pump retains only its existing catalog/preferences/navigation and
Batch 1/4 defaults; no playlist, Soulseek or lyrics algorithm was added to it.
Download scrolling uses actual wheel input in the library viewport. Observation
IDs and accessibility labels derive from the same names/captions shown onscreen.

The narrow-window scenario exposed a real bug: New Tab was inside the scrolling
strip and could be unreachable. It now stays beside the strip while the tabs
scroll. The failing pre-fix log and the playlist/lyrics negative checks are
recorded in the progress document.

## Add another feature scenario

- Follow the layout rules in "Test layout" above: one feature module per
  target, its name used as the filter, and fixture helpers in the matching
  `tests/common/ui/` domain file.
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
