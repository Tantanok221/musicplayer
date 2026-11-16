# Agent UI testing: implementation progress

Scope approved 2026-11-16: enable GPUI testing, controlled service/player
handles, queryable tabs/navigator, real input regression, CI diagnostics, and
restart persistence through an isolated catalog. No commit/push is implied.

## Status

- [x] Configure GPUI Kit test support for development builds.
- [x] Add controlled service/player handles without starting network/audio/OS services.
- [x] Register stable tab and navigator test targets.
- [x] Exercise new tab → artist through actual input handlers; assert identity/order/layout.
- [x] Configure the CI scenario and diagnostic artifacts (remote run pending).
- [x] Close/recreate the view with preferences read from an isolated SQLite catalog.
- [x] Pass fmt, Clippy, full tests, and local bundle checks.

## Implementation decisions

- Keep the production `MusicApp` and existing command/event boundaries.
- Integration tests belong in `tests/`, per the user's follow-up on 2026-11-16.
  The binary and tests share the library-owned production UI; private view
  fields stay private, with read-only test inspection behind `test-support`.
- Enable test handles behind a `test-support` Cargo feature; enable GPUI's
  observation support through the matching version's development dependency.
- Use in-process GPUI interaction/layout tests, not pixel or packaged-app tests.
- Persistence coverage must pass through the UI's debounced Preferences command
  into a real temporary SQLite database, then create a fresh view from saved state.
- Test diagnostics must use an allowlist of safe IDs/state, never Debug-dump
  commands containing credentials or authenticated URLs.

## Current checkpoint

### Publication checkpoint (2026-11-16)

The user authorized committing and pushing the completed testing work before
starting Batch 2. Implementation and CI wiring are committed as `9724a3d`
(`test: add desktop UI integration suite`). The testing guides, audit, roadmap,
and this progress log are committed separately with this entry. The destination
is `origin/main`; verify remote parity and the resulting CI run after pushing.
The earlier references to uncommitted work/no push describe historical checkpoints.
Batch 2 remains planned and has not been started.

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

Batch 1 is complete. No required local implementation or validation remains.

The next integration-test expansion is Batch 2 (Explorer) in
[INTEGRATION_TEST_PLAN.md](INTEGRATION_TEST_PLAN.md). The five Batch 1 tests are
implemented; later batches have not been implemented or counted as coverage.

1. Finish/verify the authorized publication described above, then inspect the
   resulting macOS/Linux CI jobs and their diagnostic artifacts. Use short
   conventional commits without coauthors for any later authorized changes.
2. If CI fails, run the exact failing command from `docs/TESTING.md`; inspect
   the uploaded `environment.txt` and `test.log`. Linux execution has not been
   verified locally; do not claim it passed until the remote job does.
3. For a later slice, reuse controlled handles for ratings rollback/retry or
   add real service-worker/loopback-HTTP integration. The current scenario
   intentionally does not start the service worker, a new process, native AX,
   a renderer for pixels, or an audio device.
