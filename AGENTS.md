For commits, never add coauthors. Keep messages simple and short, using conventional prefixes such as feat, fix, test, and docs.

This is a Rust macOS desktop app. Keep the UI, catalog, server, playback, and platform boundaries intact. Do not log authenticated URLs or credentials. Never delete cached records after an incomplete sync.

Run cargo fmt --check, cargo clippy --all-targets -- -D warnings, and cargo test for substantive changes. Use scripts/bundle.sh to create a local .app; no public distribution or remote Git changes are implied.

Before every push, run cargo test --locked --features test-support and scripts/test-ui.sh against the final changes, and do not push unless both pass. The feature-enabled command runs the full suite, including ordinary tests, real service-worker integration tests, and UI integration tests. Plain cargo test skips the feature-gated integration targets and is not a substitute. Keep the UI runner's diagnostic artifacts.

## Test-driven development

Work test-first for every feature and bug fix:

1. Write or extend the integration scenario that describes the intended behavior.
2. Run it and confirm it fails for the expected reason (a missing or wrong visible state, command, request, or persisted value, not a compile error or a typo). Keep that failing output as evidence.
3. Implement the smallest change that makes it pass, then refactor with the suite green.
4. For a bug fix, the failing scenario must reproduce the reported bug before the fix.

Every new feature, or change to a feature's behavior, must include at least one integration test. Unit tests may be added too but do not replace it. Choose the layer that owns the behavior, and use more than one when the feature crosses layers:

- Visible behavior and input handling: the ui target with the controlled service (tests/ui/<feature>.rs).
- Service commands, HTTP, SQLite, and events: the service_worker target (tests/service_worker/<feature>.rs).
- Behavior that must work from a click through to the server: a real-worker UI scenario (tests/ui/worker.rs or a sibling module).
- Queue and player actor behavior: tests/player_queue.rs.

Include the important failure, stale, or cancel case, not only the success path. If a behavior genuinely cannot be covered by an integration test (native AX, pixels, physical audio), say so explicitly in the review evidence instead of skipping silently.

## Integration test layout

Integration tests are split by layer (Cargo target) and then by feature (module). docs/TESTING.md has the feature map and layer table.

- Target roots (tests/ui.rs, tests/service_worker.rs) only declare modules. Never put scenarios in them.
- Put a feature's scenarios in its existing module. For a new feature, add a new module file and declare it in the root with #[path = "<target>/<feature>.rs"]. Do not add a new [[test]] target per feature; each target is a separate binary linking the whole app.
- The module name is the test filter. Run only your feature while iterating, e.g. scripts/test-ui.sh playlists:: or scripts/test-service.sh sync::, then the full suite before finishing.
- The UI fixture lives in tests/common/ui/ and is split by domain (seed, input, library, feedback, playback, tabs). pump() arms only delegate to domain methods. Prefer scenario-local Fixture::expect replies over new shared defaults so the fixture does not become a second service implementation.
- When you add or move a module, update the feature map in docs/TESTING.md.

## UI code layout

- src/ui.rs holds only the MusicApp struct, new(), actions and Render. Put a feature's handlers and rendering in src/ui/<feature>.rs as an impl MusicApp block; use the feature map in docs/TESTING.md to find the right file, and add a module and map row for a new feature.
- When you touch a feature, move its state and transition logic (what changes on an input or event, independent of GPUI) into a plain type outside the view, such as browser::History or feedback::Feedback, and cover it with unit tests. The view keeps rendering, input wiring and calls into that type.
- Keep helpers used by one feature in that feature's module. Only shared helpers stay in src/ui.rs.

For UI changes, read docs/TESTING.md. Run scripts/test-ui.sh and cargo clippy --locked --all-targets --features test-support -- -D warnings when changing UI scenarios or controlled handles. Exercise actual input handlers and assert the resulting visible state; keep test catalogs isolated and diagnostics free of credentials.
