For commits, never add coauthors. Keep messages simple and short, using conventional prefixes such as feat, fix, test, and docs.

This is a Rust macOS desktop app. Keep the UI, catalog, server, playback, and platform boundaries intact. Do not log authenticated URLs or credentials. Never delete cached records after an incomplete sync.

Run cargo fmt --check, cargo clippy --all-targets -- -D warnings, and cargo test for substantive changes. Use scripts/bundle.sh to create a local .app; no public distribution or remote Git changes are implied.
