# Agent integration testing: implementation progress

## Settings-first follow-up (2026-11-16)

Added 8 production-view scenarios and 1 worker scenario for settings/sign-in,
Soulseek credentials, volume/mute/shuffle/repeat, theme switching, backdrop
responses and window frame persistence. Current totals: 41 UI, 19 worker and
2 paused-player scenarios. See [coverage, filters and limits](TESTING.md#settings-playback-preferences-appearance-and-window-coverage).

The sign-in scenarios exposed three behavior bugs, fixed after reproduction:

- A failed Connect left status at Connecting and the button disabled.
- A successful Connect left focus on a removed credential input, preventing
  the settings shortcut from reopening the form.
- A late Ready restored tab content and dismissed the first-run sign-in form.

The fixes reset connection status on failure, restore app focus on successful
form submission and retain the settings surface across Ready tab restoration.
Saved frame validation and the settings key binding moved into shared library
functions used by the executable and tests. The fixture now persists SaveWindow
through Paths; no catalog/server/player boundaries changed.

Failure evidence: `target/settings-first-red.log`,
`target/settings-worker-red.log`, `target/settings-slskd-red.log`,
`target/window-frame-red.log` and `target/backdrop-red.log`. The control/theme
scenarios cover existing behavior; fixture/API setup errors were corrected
before recording meaningful assertion evidence. Six intentional regressions
(removing focus restoration, dismissing the form on Ready, disabling shuffle,
omitting theme application, choosing a stale backdrop and dropping the worker's
window save) each fail their owning scenario. Source was restored in `finally`
blocks. Evidence is retained in `target/settings-*-regression.log`.
Final validation follows below.

Only isolated fixtures and synthetic credentials are used. Native window moves,
pixels, crossfade timing and physical audio are not covered. An unrelated
`.claude/` directory appeared during this work and is left untouched. No commit
or push is included in this follow-up.

## Nine-gap follow-up (2026-11-16)

Implemented the nine requested gaps across existing integration targets: 12 new
worker scenarios and 7 UI scenarios, bringing the totals to 18 worker, 33 UI and
2 paused-player scenarios. Module layout, commands and scope are documented in
[TESTING.md](TESTING.md#queue-accounts-reconnect-and-deferred-coverage).

Production fixes found by the scenarios:

- Remap a restored queue's index after missing catalog records, retaining the
  exact duplicate occurrence; reset position/listen when the current song is gone.
- Publish SignInRequired when starting without a saved account.
- Re-sync and retry failed covers when an established offline connection recovers.
- Keep scrobbles pending while the known connection is offline.
- Scope player persistence messages to their account. Restore queue/namespace
  atomically, reject delayed old-account saves, and save the outgoing queue
  before switching catalogs.

Added a real paused-player worker option using the shared production event
bridge, a controllable artwork backoff clock, strict HTTP routes/gates and
observation of song feedback, domain track rows, album panels, connection state,
sign-in fields and release details. UI input still traverses production handlers.
Flat track row keys now use song IDs instead of virtual row indices.

Failing-before-fix evidence is retained in `target/gaps-queue-red.log`,
`gaps-worker-red.log`, `gaps-scrobble-red.log`, `gaps-account-save-red.log`,
`gaps-song-red.log` and `gaps-details-red.log`. Compilation/setup errors were
corrected before recording the scenario failures used as regression evidence.
Positive focused runs and intentional regression checks are retained alongside
them under `target/gaps-*.log`. Four intentional regressions (stale artist,
missing song optimism, disabled paging and removed artwork backoff) each failed
the new scenario as intended; all temporary source changes were restored.

Final local macOS validation:

- `cargo fmt --check`: PASS.
- `cargo clippy --locked --all-targets -- -D warnings`: PASS.
- `cargo clippy --locked --all-targets --features test-support -- -D warnings`: PASS.
- `cargo test --locked`: PASS, 152 ordinary tests.
- `cargo test --locked --features test-support`: PASS, 205 tests total.
- `scripts/test-service.sh`: PASS, all 18 scenarios.
- `scripts/test-ui.sh`: PASS, all 33 scenarios.
- `scripts/bundle.sh`: PASS; `dist/Sonora.app` built locally.
- `codesign --verify --deep --strict dist/Sonora.app`: PASS.
- `git diff --check`: PASS.

Command logs are retained as `target/gaps-final-*.log`; both runners also retain
their standard `target/{ui,service}-test-artifacts/` diagnostics. The pre-existing
upstream `block` future-incompatibility notice remains. No new warnings remain.

The user authorized committing and pushing this follow-up after local
validation. Remote CI results are not included in the local validation above.
All catalogs, credentials and HTTP replies are synthetic and isolated. Offline play is a real UI-to-worker
command assertion, not physical audio or proof of an offline audio-file cache.

## Earlier scope

Scope approved 2026-11-16: enable GPUI testing, controlled service/player
handles, queryable tabs/navigator, real input regression, CI diagnostics, and
restart persistence through an isolated catalog. No commit/push is implied.
Batch 3 (real service-worker integration) was requested on the same date.
Batches 2–3 are now committed in `8689e80`; `b0808a5` adds the full-test push
requirement. Earlier checkpoints below describe their state at implementation
time. Batch 4 was implemented in a separate worktree and merged/pushed at
`f38c40f`. Batch 5 continues locally on that merged baseline.

## Status

- [x] Configure GPUI Kit test support for development builds.
- [x] Add controlled service/player handles without starting network/audio/OS services.
- [x] Register stable tab and navigator test targets.
- [x] Exercise new tab → artist through actual input handlers; assert identity/order/layout.
- [x] Configure CI scenarios/artifacts and verify Batches 1–3 on macOS/Linux.
- [x] Close/recreate the view with preferences read from an isolated SQLite catalog.
- [x] Pass fmt, Clippy, full tests, and local bundle checks.
- [x] Add real-worker/loopback-HTTP scenarios with isolated catalogs and bounded shutdown.
- [x] Add queue/playback UI scenarios paired with the real paused player actor.
- [x] Add playlist/layout/download/lyrics UI scenarios with local command scripts.

## Implementation decisions

- Keep the production `MusicApp` and existing command/event boundaries.
- Integration tests belong in `tests/`, per the user's follow-up on 2026-11-16.
  The binary and tests share the library-owned production UI; private view
  fields stay private, with read-only test inspection behind `test-support`.
- Enable test handles behind a `test-support` Cargo feature; enable GPUI's
  observation support through the matching version's development dependency.
- UI scenarios use in-process GPUI interaction/layout, not pixels or packaged apps.
- UI restart coverage must pass through the debounced Preferences command
  into a real temporary SQLite database, then create a fresh view from saved state.
- Test diagnostics must use an allowlist of safe IDs/state, never Debug-dump
  commands containing credentials or authenticated URLs.

## Current checkpoint

### Feature-module split (2026-11-16)

Split integration scenarios into one module per feature, with no behavior change.
`tests/ui.rs` and `tests/service_worker.rs` now only declare modules. Batch 1
moved to `tests/ui/tabs.rs` and `tests/ui/album_feedback.rs`. Worker scenarios
moved to `tests/service_worker/{feedback,sync,preferences,metadata,lifecycle}.rs`.
The UI fixture moved to `tests/common/ui/`, split by domain, with `pump()`
delegating to per-domain responders. Test names now carry their module prefix.
The before/after `--list` output has the same 26 UI and 6 worker scenario names.
AGENTS.md now requires test-first work, an integration test for every feature
change, and the layout rules. TESTING.md has the layer/feature table.

### Batch 5 checkpoint (2026-11-16)

Before continuing, inspected the user's Batch 4 merge `f38c40f`, its real paused
player adapter, shared bindings, queue revision guard and five UI/two actor
scenarios. The merge preserved the three in-progress Batch 5 files cleanly.
No Batch 4 behavior was replaced. Verified
[merged CI run 43854401169](https://github.com/Tantanok221/musicplayer/actions/runs/43854401169):
macOS-15, Ubuntu-24.04 and formatting passed. The audit fix `bd6a07c` was also
committed/pushed as requested and its
[CI run 43853394810](https://github.com/Tantanok221/musicplayer/actions/runs/43853394810)
passed. Those remote runs cover committed work, not this local Batch 5 diff.

Implemented nine scenarios in `tests/ui/playlists.rs`, `layout.rs`,
`downloads.rs` and `lyrics.rs`:

- Four playlist scenarios cover create/empty submission, retained Unicode input
  after failure and actual Retry, rename plus album-to-playlist submenu intent
  and visible confirmation, cancellation without a delete command, and separate
  confirmed deletion after acknowledgement.
- Two layout scenarios cover a real divider drag and information-panel toggle
  saved to temporary SQLite and restored in a fresh view, plus 760px tab-strip
  navigation to both ends while New Tab remains usable.
- Two download scenarios select a missing release and exercise real wheel input,
  search failure/retry, partial-folder refusal and alternative folder choice,
  duplicate Start suppression, partial transfer/failure captions, dismissal,
  completed-but-waiting-for-library state, Refresh and Clear intents.
- One lyrics scenario keeps Selected and Playing tracks distinct, checks an
  offset-adjusted Seek through a timed playing line, rejects Seek from another
  selected track, and switches back to the playing track's details.

`Fixture::expect()` queues one-shot assertions/replies declared by each scenario;
`assert_script_finished()` rejects missing commands. No new playlist, download
or lyrics service algorithm was added to the shared pump. Diagnostics use safe
expectation labels and existing synthetic data; all persistence uses isolated
SQLite and no network/audio/OS services start in these nine scenarios.

The narrow-window test found a production bug: New Tab was clipped inside the
scroll strip. Moved the existing button beside the strip and reserved its width,
leaving the scrollable tab indices/identity intact. Evidence:
`target/batch5-layout-before-fix.log`; the corrected layout scenario passes.
Added observation/accessibility to existing playlist controls, info/lyric rows,
missing-release/download captions and the content viewport. Captions and labels
come from displayed values; observation is a no-op in normal builds.

Intentional negative checks also failed as expected when hiding the playlist
editor on rejection (`target/batch5-playlist-editor-regression.log`) and when
allowing Selected lyrics to seek another playing track
(`target/batch5-lyrics-source-regression.log`). Source was restored in `finally`
after each check.

Final positive validation on local macOS after restoring both mutations:

- `cargo fmt --check`: PASS (`target/batch5-fmt.log`).
- Locked/all-targets Clippy with `-D warnings`, default and `test-support`: PASS
  (`target/batch5-clippy-{default,feature}.log`).
- `cargo test --locked`: PASS, 152 ordinary tests
  (`target/batch5-default-tests.log`).
- `cargo test --locked --features test-support`: PASS, 186 tests
  (152 ordinary + 2 paused-player + 6 worker + 26 UI;
  `target/batch5-all-tests.log`).
- `scripts/test-ui.sh`: PASS, all 26 scenarios, 3.53s
  (`target/batch5-ui-tests.log`, `target/ui-test-artifacts/`).
- The four Batch 5 feature modules also passed independently:
  `target/batch5-{playlists,layout,downloads,lyrics}.log`.
- `git diff --check`: PASS. Intentional failure logs remain separate from
  positive runner diagnostics.

The user subsequently authorized committing/pushing Batch 5 and explicitly
requested no repeat of suites already run. The final feature-enabled and UI
gates above passed against these source changes; only documentation changed
after those checks. These are in-process controlled UI tests, not playlist
HTTP/slskd transfer/audio or packaged-app tests. Batch 5 remote validation is
pending publication; the earlier linked runs cover the committed baseline.

### Audit follow-up (2026-11-16)

Added one cross-layer scenario in `tests/ui/worker.rs`: actual Explorer album
selection and Favorite/Retry clicks run against `test_support::Worker`, strict
loopback HTTP and temporary SQLite. The harness forwards real events in FIFO
order through MusicApp's receiver without using the controlled UI fixture's
`pump()`. It checks optimistic/disabled feedback, duplicate suppression, visible
rollback/error, identical Retry HTTP intent, persisted success and error removal.
The shared HTTP fixture now also answers strictly validated artist/top-song
requests made when selecting the album.

Added default-feature Clippy alongside feature-enabled Clippy in CI, optional
filtering to the UI runner, and `scripts/test-service-repeat.sh` for 20 occasional
real-worker repetitions with separate retained diagnostics. The expansion plan
now explicitly tracks executable data isolation, packaged smoke, screenshots,
large-catalog scrolling/other layouts and stale artist-page responses. Future
controlled scenarios should declare expected commands/replies locally instead
of growing a second service in `pump()`.

Verified existing [CI run 43850271484](https://github.com/Tantanok221/musicplayer/actions/runs/43850271484)
at `b0808a5`: formatting plus both macOS-15 and Ubuntu-24.04 jobs passed,
including ordinary tests, all six real-worker scenarios and eleven UI scenarios.
This resolves the previously pending Linux evidence for Batches 2–3; it does
not validate the new follow-up changes, which have not been pushed.

Local follow-up validation on macOS:

- `cargo fmt --check`: PASS (`target/audit-followup-fmt.log`).
- Locked/all-targets Clippy with `-D warnings`, default and `test-support`: PASS
  (`target/audit-followup-clippy-{default,feature}.log`).
- `cargo test --locked`: PASS, 152 ordinary tests
  (`target/audit-followup-default-tests.log`).
- `cargo test --locked --features test-support`: PASS, 170 tests
  (152 ordinary + 6 worker + 12 UI; `target/audit-followup-all-tests.log`).
- `scripts/test-ui.sh`: PASS, all 12 scenarios (`target/audit-followup-ui.log`;
  standard diagnostics remain in `target/ui-test-artifacts/`).
- `scripts/test-service-repeat.sh`: PASS, 20/20 repetitions, six worker
  scenarios each (120 passes). Summary: `target/audit-followup-service-repeat.log`;
  per-run diagnostics: `target/service-repeat-artifacts/run-{1..20}/`.
- Removing the production worker's rollback event made the new scenario fail
  on the visible heart label: actual Unfavorite versus expected Favorite.
  Source restored in `finally`; subsequent positive suites passed. Evidence:
  `target/worker-ui-rollback-regression.log`.

No Batch 4/player implementation was changed in this checkpoint. The follow-up
was subsequently committed/pushed as `bd6a07c` and passed macOS/Linux CI in run
43853394810, linked above.

### Batch 4 publication authorization (2026-11-16)

The user subsequently authorized merging Batch 4 into main and pushing it.
Statements about no publication authorization below describe earlier checkpoints.
The user also authorized temporarily stashing main's unrelated uncommitted work,
then restoring it after the push. Publication must rerun the mandatory full-feature
and UI pre-push gates against the final main commit and retain diagnostics.

### Batch 4 checkpoint

Worktree `/Users/tantanok/c/musicplayer-batch4`, branch
`test/batch4-queue-playback`, starts at `b0808a5`. All source edits from this
session stay in that worktree. Independent changes now exist in the original
worktree and were left untouched. No Batch 4 commit/push is authorized.

Implemented five scenarios in `tests/ui/playback.rs`: album command ID/placement/
count, duplicate occurrence selection/move/remove/Undo/drag, stale queue menus
and snapshots, seek preview/held-position/release, and Space with Search versus
queue focus. They use the production view, real input handlers and shared
production key bindings. Queue scenarios pass captured commands through the
production paused actor and deliver its snapshots, rather than duplicating the
queue algorithm in the UI fixture.

Added feature-gated `playback::test_support::PausedPlayer` and two integration
scenarios in `tests/player_queue.rs`. The actor owns the real queue revisions,
occurrence IDs, edits and Undo. Its native output discovery and media-control
adapters are disabled, and the test endpoint rejects transport/server commands.
It owns a private runtime and joined thread, with readiness/snapshot/completion
deadlines. Owning-layer cases cover stale/current/past edits, duplicate IDs,
Undo/repeated Undo, insertion placement, clear-upcoming, paused position and
metadata retained through Undo.

The stale UI scenario first failed with old visible IDs `[1, 2, 3, 4]` replacing
current `[1, 4, 2, 3]`. `Event::Playback` now rejects a lower queue revision.
Evidence: `target/batch4-stale-before-fix.log`; the fixed scenario passes.
Additional negative checks confirmed that sending Seek during Change fails
the held-drag assertion (`target/batch4-seek-regression.log`), and removing the
actor's revision check fails the duplicate occurrence assertion
(`target/batch4-player-stale-regression.log`). Both intentional mutations were
restored in `finally` blocks.

A shared build cache produced a stale-library compile failure during development.
Final validation uses the worktree's own `target/`; dependency caches were
copy-on-write cloned, then this package's copied artifacts were cleaned and
rebuilt. No original source or diagnostic artifacts were removed.

CI now runs `cargo test --locked --features test-support`, so the new player
target is not skipped. UI/service runners still retain their separate logs.

Final validation on local macOS, after restoring both negative mutations:

- `cargo fmt --check`: PASS (`target/batch4-fmt.log`).
- Both locked/all-targets Clippy modes with `-D warnings`: PASS
  (`target/batch4-clippy-default.log`, `target/batch4-clippy-feature.log`).
- `cargo test --locked`: PASS, 152 ordinary tests
  (`target/batch4-default-tests.log`).
- `cargo test --locked --features test-support`: PASS, 176 tests
  (152 ordinary + 2 paused-player + 6 real-worker + 16 UI);
  `target/batch4-all-feature-tests.log`.
- `scripts/test-ui.sh`: PASS, all 16 scenarios, 2.88s;
  `target/batch4-ui-tests.log` and `target/ui-test-artifacts/`.
- `scripts/test-service.sh`: PASS, all 6 scenarios, 0.90s;
  `target/batch4-service-tests.log` and `target/service-test-artifacts/`.
- Each of the five new UI scenarios independently with exact Cargo filters:
  PASS; `target/batch4-isolated-*.log`.
- `scripts/bundle.sh` and `codesign --verify --deep --strict dist/Sonora.app`:
  PASS, release compilation 55.46s. Console transcript and signature result
  retained in `target/batch4-bundle.log`.
- Runner shell syntax and `git diff --check`: PASS.

Batch 4 is complete locally. No current compile/lint/test failures remain.
The existing upstream `block` future-incompatibility notice remains. No commit,
push or remote CI validation was performed.

These are in-process UI and paused-actor checks, not packaged-process relaunch,
native AX, pixels, decoder timing or physical audio-device tests. Existing
decoder/PCM/consumed-audio tests remain distinct evidence.

## Historical implementation checkpoints

### Batch 3 checkpoint (2026-11-16)

Reviewed the testing guide, historical audit, integration plan, progress log,
current Explorer changes, fixtures, runner/CI wiring, and saved validation
artifacts, including the intentionally failing regression logs. Batch 2 was
already complete locally, with eleven passing UI scenarios. No prior changes
were reverted or published.

Implemented six scenarios in `tests/service_worker.rs`, supported by
`tests/service_worker/fixture.rs`:

1. Album favorite/unfavorite and rating set/clear traverse real commands, HTTP,
   SQLite and events. Exact album-versus-song parameters and independent state
   are asserted, including separate track favorite/rating requests.
2. Favorite and rating rejection publish the original album before failure,
   retain SQLite state, then accept exactly one identical retry request.
3. A later sync page fails after 500 imported songs; cached songs, albums and
   playlists remain. A scan-in-progress pass also preserves them, then a
   complete non-scanning refresh reconciles and persists the result.
4. Preferences go through the real handler and player endpoint, then restore
   through Ready and the player after clean worker shutdown/restart.
5. Discography requests use loopback Navidrome, MusicBrainz and ListenBrainz
   endpoints, verifying metadata injection instead of public network access.
6. Shutdown joins the worker and tears down its runtime with metadata HTTP
   still pending, completing the actual queue-save handshake.

The only normal-build change moves metadata-client construction to the worker
caller and passes that client into the existing production command loop.
`service::test_support::Worker` supplies a silent player and a private Tokio
runtime. Completion is signaled after runtime teardown; the fixture also joins
its HTTP server. No audio device or OS media controls start. All waits use
completion signals/deadlines, not test sleeps. Strict HTTP fixtures fail unknown
methods/routes/parameters or unexpected mutations, and log only allowlisted
request summaries plus scenario-tagged event names.

Added `scripts/test-service.sh`, macOS/Linux CI invocation and separate service
diagnostic artifacts. It supports a test name plus `--exact`, preserves Cargo
failures, and fails zero-test runs. `TESTING.md` now explicitly distinguishes
ordinary `cargo test` from the feature-enabled all-tests command.

Batch 3 validation on local macOS:

- `cargo fmt --check`: PASS (`target/batch3-fmt.log`).
- Clippy, locked/all-targets/`-D warnings`, with and without `test-support`: PASS
  (`target/batch3-clippy-default.log`, `target/batch3-clippy-feature.log`).
- `cargo test --locked`: PASS, 152 ordinary tests, neither feature-gated target;
  `target/batch3-default-tests.log`.
- `cargo test --locked --features test-support`: PASS, 169 tests total
  (152 ordinary + 6 real-worker + 11 UI); `target/batch3-all-feature-tests.log`.
- Final `scripts/test-service.sh`: PASS, all six scenarios, 0.86s; diagnostic
  files in `target/service-test-artifacts/`, copied log `target/batch3-service-tests.log`.
- All six worker scenarios separately with exact Cargo filters: PASS;
  `target/batch3-isolated-tests.log`. The documented exact-filter script command
  also passed (`target/batch3-focused-runner.log`). A nonexistent filter returned
  exit 1 as intended (`target/batch3-zero-test-gate.log`).
- `scripts/test-ui.sh`: PASS, all eleven existing UI scenarios, 1.63s;
  `target/batch3-ui-tests.log`, `target/ui-test-artifacts/`.
- Four intentional production regressions each caused a scenario assertion
  failure: album feedback sent as song feedback, missing rollback event, deleting
  stale rows on sync failure (500 versus expected 501), and omitted preference
  persistence. Every mutation was restored in `finally`, before positive checks.
  Evidence: `target/batch3-{routing,rollback,sync,preferences}-regression.log`.
- `scripts/bundle.sh` plus `codesign --verify --deep --strict dist/Sonora.app`:
  PASS, release build 38.35s; `target/batch3-bundle.log`.
- Runner shell syntax and `git diff --check`: PASS.

Batch 3 is complete locally. The existing upstream `block` future-incompatibility
notice remains; no current compiler/Clippy/test failures remain. No commit, push
or remote CI run was performed. Only local macOS execution is confirmed. The
new tests cover a real worker, not a packaged process, native AX, pixels or audio
consumption. UI Retry/optimistic state remains covered separately by Batch 1.

### Batch 2 checkpoint (2026-11-16)

Approved scope is Music Explorer only: Unicode/empty search, genre/year/artist
filtering and clear, sort/direction persistence, genre/year/label metadata links
with Back, and stale search results. Artist-page navigation and stale artist
responses are deferred. Starting from clean `be71419` on `main`.

Implemented five scenarios in `tests/ui/explorer.rs` (test-only child module
of the `tests/ui.rs` integration target). They passed together on the first
focused run: 5 passed, 0 failed, 1.35s. Shared helpers now seed genre/year/label
metadata and can hold/deliver catalog-backed query responses out of order.
The filter scenario checks the rendered queue and rejects playback commands;
all five assert the active page stays Explorer with no artist page.

Added stable observation to the search field, album cover cards, browser
filters, metadata links, and empty state. Covers/browser options use domain
IDs instead of row positions; selection properties come from native state.
Feature-enabled Clippy passed after removing the unused import. Intentional
regression check passed: allowing old `Event::View` epochs made the stale-search
test fail with rendered `album-3` instead of expected `album-2`. Production's
equality guard was restored in a `finally` block. Evidence:
`target/batch2-stale-regression.log`.

Batch 2 validation:

- `cargo fmt --check`: PASS.
- Clippy with and without `--features test-support`, all targets, `-D warnings`: PASS.
- `cargo test --locked`: PASS, 152 existing tests; `target/batch2-default-tests.log`.
- `scripts/test-ui.sh`: PASS, all 11 UI scenarios; `target/batch2-all-ui-tests.log`.
- Each of the five Explorer scenarios separately with exact filters: PASS;
  `target/batch2-isolated-tests.log`.
- Stale-result negative verification: expected failure confirmed, source restored.
- `scripts/bundle.sh` and `codesign --verify --deep --strict dist/Sonora.app`: PASS;
  `target/batch2-bundle.log`.
- `git diff --check`: PASS.

Batch 2 was complete locally at this checkpoint. It was subsequently committed
with Batch 3 in `8689e80` and passed macOS/Linux CI at `b0808a5`. The default Album Covers
view is covered with three fixture albums; alternate layouts, large-library
scrolling, artist pages, real service workers and pixel/native checks remain
separate work.

### Publication checkpoint (2026-11-16)

The user authorized committing and pushing the completed testing work before
starting Batch 2. Implementation and CI wiring are committed as `9724a3d`
(`test: add desktop UI integration suite`). The testing guides, audit, roadmap,
and this progress log are committed separately with this entry. The destination
is `origin/main`; verify remote parity and the resulting CI run after pushing.
The earlier references to uncommitted work/no push describe historical checkpoints.
Publication completed at `be71419` on `origin/main`. The baseline CI run
43826764407 completed successfully (macOS/Linux, verified during Batch 2).
Batch 2 was not part of that baseline run; the later run at `b0808a5` passed
both Batches 2–3 on macOS/Linux.

### Batch 1 checkpoint (2026-11-16)

Implemented the five approved navigation/album-feedback scenarios in
`tests/ui.rs`, alongside the existing restart scenario. The shared fixture now
seeds two artists/three albums with differing album/song feedback, serves
catalog-backed group-track responses, and captures album mutations until a test
chooses confirmation or failure. Responses use real catalog APIs and the
service's event order. Unknown service commands still fail without network.

Registered native observation for tab close buttons, artist release cards and
the error alert; release selection and the alert's actual error text are exposed
through accessibility properties. No product behavior change was required.

Batch 1 is complete and validated locally:

- `scripts/test-ui.sh`: PASS, all 6 scenarios together. Log:
  `target/batch1-ui-tests.log` (also `target/ui-test-artifacts/test.log`).
- All 6 scenarios run separately with exact Cargo filters: PASS. Log:
  `target/batch1-isolated-tests.log`.
- `cargo test --locked`: PASS, 152 existing tests. Log:
  `target/batch1-default-tests.log`.
- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  and the same Clippy command with `--features test-support`: PASS.
- Intentional regression checks: removing the production optimistic favorite
  update failed the expected label assertion; dropping parked tab history
  failed the expected artist assertion. Both source mutations were restored
  in `finally` blocks, and all positive checks passed afterward. Evidence:
  `target/batch1-favorite-regression.log`, `target/batch1-history-regression.log`.
- `scripts/bundle.sh`: PASS, release build 37.87s. Signature verification with
  `codesign --verify --deep --strict dist/Sonora.app`: PASS. Log:
  `target/batch1-bundle.log`.
- `git diff --check`: PASS. Scenario-prefixed diagnostics identify interleaved
  output and include disabled state plus held mutation counts.

No commit, push, or remote CI run has been performed. CI already uses the same
UI runner on macOS/Linux and will include all six cases on the next push.
Only local macOS results are confirmed. No product behavior fix was needed.

### Migration checkpoint (2026-11-16)

Moved the UI scenario to `tests/ui.rs` and reusable fixture code to
`tests/common/ui.rs`. The production UI/artwork-cache modules are now owned by
the library and consumed by the binary and tests. Tab bindings are shared
through `sonora::ui::tab_bindings`; no source files are included through
test-only `#[path]` copies. `MusicApp::tab_state()` is a read-only reader gated
by `test-support`; view fields remain private.

Cargo declares `[[test]] name = "ui"` with `required-features = ["test-support"]`.
The runner now uses `cargo test --locked --features test-support --test ui`;
CI still calls the same script and collects the same diagnostics. The testing
guide and expansion plan use the new paths. Migration checks passed:

- `scripts/test-ui.sh`: PASS, 1 integration scenario in `tests/ui.rs` (0.52s).
- `cargo fmt --check`: PASS.
- `cargo clippy --locked --all-targets -- -D warnings`: PASS.
- `cargo clippy --locked --all-targets --features test-support -- -D warnings`: PASS.
- `cargo test --locked`: PASS, 152 existing tests. Log: `target/test-migration.log`.
- `git diff --check` and `bash -n scripts/test-ui.sh`: PASS.
- `scripts/bundle.sh`: PASS, release build completed in 42.99s.
- `codesign --verify --deep --strict dist/Sonora.app`: PASS.
  Bundle log: `target/bundle-migration.log`.

Migration complete. All integration scenarios/fixtures live under `tests/`;
unit tests of private helpers remain beside the implementation. No additional
feature scenarios were added during this move. This historical checkpoint was
subsequently included in `9724a3d`.

Earlier validation entries below refer to the previous binary-local layout.

Repository baseline: `4ad37ac`. Existing untracked audit document
`docs/AGENT_TESTING_GAPS.md` belongs to this work and must be preserved.
Read current sources and the GPUI Kit coding/testing guides. Installed 0.9.0
contains `TestWindowExt`, `TestSupportExt`, and `TestAppContext`; use these APIs.
Implemented `service::test_support::ControlledService`, a silent player endpoint,
and a shared `persist_preferences` function used by the worker and controlled
handle. Registered custom tabs and stable navigator facet IDs with GPUI's
observation API. This API is a no-op in normal builds.

`tests/ui.rs` contains the production-view scenario; `tests/common/ui.rs` holds
the SQLite fixture, bounded channel/event pump, and safe diagnostics.
`scripts/test-ui.sh` and CI artifact wiring are written.
Tab shortcuts are shared through `sonora::ui::tab_bindings` so tests use the same
bindings as the application. The first scenario compiled and passed locally,
including pointer input, rendered properties/geometry, SQLite persistence and
restoration, and keyboard input. The strengthened scenario also passed, including
499ms/500ms debounce checks, controlled player updates, and keyboard creation from
a middle tab followed by artist selection.

All six implementation items are complete locally. CI configuration is ready
for the next authorized push; no commit, push, or remote run was performed.
The working tree contains the implementation, verification guide, progress log,
updated README/AGENTS instructions, and original audit. See `docs/TESTING.md`
for the next agent's entry point.

## Validation log

- `cargo check --tests --features test-support`: dependency resolution failed
  before compilation. GPUI Kit's optional fast backend pins `taffy =0.15.0`,
  conflicting with the existing lockfile's 0.15.1.
- `cargo update -p taffy --precise 0.15.0`: exposed another optional-backend
  exact pin, `unicode-properties =0.1.3` versus the locked 0.1.5.
- `cargo update -p taffy -p unicode-properties --offline`: missing cached
  `criterion` index entry required by GPUI test support.
- `cargo update -p taffy -p unicode-properties`: succeeded. Downgraded only
  those two existing package versions to 0.15.0 and 0.1.3; added 49 GPUI test
  support / optional-backend lockfile entries. `cargo tree --locked -e normal
  -i gpui-fast` confirms the fast backend is not in the normal dependency graph.
- `scripts/test-ui.sh`: PASS, one scenario; first compile took 44.17s and test
  execution took 0.38s. Added stronger 499ms/500ms debounce checks, controlled
  player command assertions, and keyboard-from-middle artist selection; PASS
  again in 0.50s.
- `cargo clippy --locked --all-targets --features test-support -- -D warnings`:
  PASS (19.63s).
- Intentional regression: temporarily restored after-active insertion in
  `Tabs::new_tab`, ran the UI script, and confirmed it FAILED after the actual
  click with `[1, 2, 5, 3, 4]` instead of `[1, 2, 3, 4, 5]`. Source was restored
  in a `finally` block. Evidence: `target/ui-test-regression/test.log`.
- `cargo fmt --check`: PASS.
- `cargo clippy --all-targets -- -D warnings`: PASS.
- `cargo test --locked`: PASS, 152 tests, 0 failures. Log: `target/test-suite.log`.
- `scripts/test-ui.sh` after restoring production insertion: PASS, 1 scenario,
  0 failures. Log: `target/ui-test-artifacts/test.log`.
- `bash -n scripts/test-ui.sh` and `git diff --check`: PASS.
- `scripts/bundle.sh`: PASS; normal release build completed in 1m 06s.
- `codesign --verify --deep --strict dist/Sonora.app`: PASS.
- Local release build does not enable the `test-support` feature or GPUI test
  observation. Existing upstream `block` future-incompatibility notice remains.

## Next work

The settings-first follow-up brings current totals to 19 worker, 41 UI and
2 paused-player scenarios. Historical checkpoints retain
their original counts and publication status.

1. For CI failures, use the commands in `docs/TESTING.md` and retain the
   corresponding runner `environment.txt` and `test.log`.
2. Use `scripts/test-service-repeat.sh` for occasional thread/runtime checks.
3. Native packaged-process isolation, AX input, pixels, media controls and
   physical audio remain separate work in [INTEGRATION_TEST_PLAN.md](INTEGRATION_TEST_PLAN.md).
4. The settings-first follow-up remains uncommitted; the earlier nine-gap follow-up was already published.
