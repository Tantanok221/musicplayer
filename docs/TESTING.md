# Agent verification guide

Implementation status and handoff notes: [AGENT_TESTING_PROGRESS.md](AGENT_TESTING_PROGRESS.md).
Broader roadmap: [AGENT_TESTING_GAPS.md](AGENT_TESTING_GAPS.md).

## Commands

```sh
cargo fmt --check
cargo clippy --locked --all-targets --features test-support -- -D warnings
cargo test --locked
scripts/test-ui.sh
```

The focused script runs the `tests/ui.rs` integration target with the
`test-support` feature. The equivalent Cargo command is
`cargo test --locked --features test-support --test ui -- --nocapture`.
It preserves Cargo's exit status while saving
`target/ui-test-artifacts/test.log` and `environment.txt`. CI runs the same
script on macOS and Linux and uploads these files. The progress document
records which commands have actually been validated and on which platform.

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

The UI target now contains six scenarios. The five additional scenarios are:

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
the real HTTP/service-worker mutation path remains future integration work.

## Add another feature scenario

- Put production-view integration scenarios in `tests/ui.rs` or child modules
  under `tests/ui/`. Put reusable fixtures in `tests/common/ui.rs`.
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
