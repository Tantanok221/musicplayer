For commits, never add coauthors. Keep messages simple and short, using conventional prefixes such as feat, fix, test, and docs.

This is a Rust macOS desktop app. Keep the UI, catalog, server, playback, and platform boundaries intact. Do not log authenticated URLs or credentials. Never delete cached records after an incomplete sync.

Run cargo fmt --check, cargo clippy --all-targets -- -D warnings, and cargo test for substantive changes. Use scripts/bundle.sh to create a local .app; no public distribution or remote Git changes are implied.

For UI changes, read docs/TESTING.md and docs/AGENT_TESTING_PROGRESS.md. Run scripts/test-ui.sh and cargo clippy --locked --all-targets --features test-support -- -D warnings when changing UI scenarios or controlled handles. Exercise actual input handlers and assert the resulting visible state; keep test catalogs isolated and diagnostics free of credentials.
