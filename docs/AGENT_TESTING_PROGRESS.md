# Agent integration testing: implementation progress

Scope approved 2026-11-16: enable GPUI testing, controlled service/player
handles, queryable tabs/navigator, real input regression, CI diagnostics, and
restart persistence through an isolated catalog. No commit/push is implied.
Batch 3 (real service-worker integration) was requested on the same date;
the completed, uncommitted Batch 2 changes were preserved.

## Status

- [x] Configure GPUI Kit test support for development builds.
- [x] Add controlled service/player handles without starting network/audio/OS services.
- [x] Register stable tab and navigator test targets.
- [x] Exercise new tab → artist through actual input handlers; assert identity/order/layout.
- [x] Configure the CI scenario and diagnostic artifacts (remote run pending).
- [x] Close/recreate the view with preferences read from an isolated SQLite catalog.
- [x] Pass fmt, Clippy, full tests, and local bundle checks.
- [x] Add real-worker/loopback-HTTP scenarios with isolated catalogs and bounded shutdown.

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

Batch 2 is complete locally. Changes are uncommitted and have not run in remote
CI. No new commit/push is authorized by this request. The default Album Covers
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
Batch 2 is now complete locally above; its changes are not part of that CI run.

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
feature scenarios were added during this move. Changes remain uncommitted.

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

Batch 3 is complete locally. No required local implementation or validation remains.

The next planned expansion is Batch 4 (queue and playback controls) in
[INTEGRATION_TEST_PLAN.md](INTEGRATION_TEST_PLAN.md). Eleven UI scenarios and six
real-worker scenarios are implemented; later batches and deferred artist-page
scenarios are not coverage.

1. If the user requests publication, inspect both uncommitted batches and commit/
   push using short conventional commits without coauthors. Earlier publication
   authorization applied only to the foundation/Batch 1, which is already pushed.
2. If CI fails, run the exact failing command from `docs/TESTING.md`; inspect
   the uploaded service/UI `environment.txt` and `test.log`. Only the baseline
   six-test UI suite is confirmed on Linux; Batches 2–3 have local macOS evidence.
3. For the next requested slice, add Batch 4 queue/playback UI coverage, pairing
   controlled snapshots with tests at the real queue/player owning layer.
   Current scenarios do not start a packaged process, native AX, a pixel renderer,
   or an audio device. Keep artist-page scenarios deferred unless scope expands.
