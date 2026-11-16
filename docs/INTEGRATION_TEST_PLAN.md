# Integration test expansion plan

Prepared 2026-11-16 after the first production-view scenario passed locally.
Batch 1 is implemented in the working tree; see the progress document for
validation. Batches 2–5 below remain proposed work.

Current coverage and commands: [TESTING.md](TESTING.md).
Completed work and validation: [AGENT_TESTING_PROGRESS.md](AGENT_TESTING_PROGRESS.md).

## Approach

Add small independent feature scenarios through production input handlers.
Check the visible outcome, the command emitted, and persisted state when the
feature promises persistence. Keep the current tab/artist/reload scenario.
Avoid combining unrelated behaviors into one long test that hides which
feature failed.

Use controlled services for fast UI feedback, real service-worker tests for
command/event/database/HTTP wiring, and a small native suite for packaged-app
behavior. Report each layer accurately; one cannot stand in for the others.

## Batch 1: navigation and album feedback

Five new production-view tests, bringing the UI suite from one to six scenarios:

| Scenario | Actions | Required assertions |
| --- | --- | --- |
| Close and reopen a tab | Open an artist, close its tab, use Reopen Closed Tab | Correct neighbor activates; the same page/order returns; other tabs keep their identity |
| Per-tab history | Navigate between two artists, Back, Forward, switch tabs and return | Correct page at each step; histories stay independent; unrelated tabs do not move |
| Album favorite success | Open an album, click Favorite, delay the response, then confirm it | Optimistic heart state; pending control disabled; repeated click sends no second mutation; confirmation clears pending state |
| Album favorite failure and Retry | Click Favorite, return the original album plus failure, click Retry, then confirm | Original heart state restored; actionable error visible; Retry emits the intended mutation once; success clears the error |
| Album rating set and clear | Click a rating, confirm, then click the selected rating and confirm | Correct `AlbumRating` value, including zero to clear; album controls agree with catalog state; track rating is unaffected |

The Batch 1 fixture and control support now includes:

- Use the reusable fixture/window/pump/diagnostic helpers in
  `tests/common/ui.rs`; keep feature scenarios in `tests/ui.rs` or focused
  child modules under `tests/ui/`. Import the library-owned production UI.
- The fixture seeds a second artist, multiple albums, and differing album/song
  favorite and rating values in a fresh temporary catalog per scenario.
- A scenario can hold a mutation pending and choose success or failure,
  preserving the service's real response order. In particular, `MutationFailed`
  alone does not roll back optimistic album state: the service also publishes
  the catalog's `AlbumUpdated` value.
- Tab close/history, album feedback, error, and Retry targets are queryable
  through existing domain-derived IDs and native state.
- Safe diagnostics include scenario names, disabled state, held mutation counts,
  and allowlisted album mutation summaries, without arbitrary command dumps.

Acceptance met locally: all six UI scenarios pass independently and together;
no scenario starts HTTP/audio/OS services; responses use controlled scheduling.
Optimistic-favorite and per-tab-history regressions were deliberately introduced,
detected by the new tests, and restored. See the progress document for evidence.

## Batch 2: Explorer and artist navigation

Add these production-view scenarios using the same fixture:

1. Type a Unicode search, check matching visible albums, clear it, and check the
   original results return. Include an empty-result case and its recovery.
2. Select genre/year/artist filters, verify the visible result set and selected
   controls, and clear the filters without changing the playing queue.
3. Select sort and direction through the menu, verify visible album ordering,
   then reload preferences and verify the selection/order survives.
4. Open an album's artist/genre/year links and verify the resulting page/filter
   plus Back behavior.
5. Delay artist A's result, open artist B, deliver A's stale result, and verify
   the active page still shows B. Add the equivalent stale-search case if its
   event path differs.

Use real catalog queries for query/filter/sort responses. Add structured access
to visible release IDs and register album rows; do not satisfy UI assertions by
checking the SQL result alone. Include a generated large catalog when testing
virtualized scrolling, with explicit scroll actions to reach offscreen items.

## Batch 3: real service-worker integration

Add this layer before treating controlled-backend scenarios as proof of complete
feature wiring. The existing fixture emulates selected service responses.

Required infrastructure:

- Start the real service worker with temporary paths and a controlled player.
- Supply loopback Navidrome and metadata endpoints; avoid the worker's current
  internally constructed public metadata client.
- Expose bounded readiness and shutdown helpers; confirm worker tasks finish.
- Make the fixture server fail unexpected requests and record safe request
  summaries. Use explicit completion events instead of sleep-based waits.

Initial scenarios:

1. Album favorite/rating: command → HTTP request → catalog update → real events.
   Cover success and failure/rollback; assert the exact album-versus-track API
   semantics and that Retry produces the intended request.
2. Interrupted sync: seed cached records, fail a later sync page, verify cached
   records remain, then complete a later refresh and verify reconciliation.
3. Preference save: use the real worker command handler, close it cleanly,
   restart from the same isolated paths, and verify restored preferences.

Reuse existing Axum and SQLite helpers where useful. Client tests and catalog
tests remain valuable, but these new tests must actually start the worker.

## Batch 4: queue and playback controls

Production-view scenarios:

1. Play an album and choose Play Next/Queue Last; verify the intended IDs,
   placement, and command count at the UI boundary.
2. Select/reorder/remove queue rows, deliver the resulting snapshot, then Undo.
   Include duplicate occurrences of the same track and stable row identity.
3. Deliver an obsolete queue revision and check that stale edits do not produce
   a misleading successful UI state; use real queue logic at its owning layer.
4. Drag seek: assert the preview while held and one Seek command on release.
   Verify playback position events do not move the thumb during the drag.
5. Type Space in Search and verify it inserts text; press Space outside an input
   and verify it sends Toggle. Keep the existing keymap unit test too.

Controlled snapshots verify rendering and emitted intent. Actual queue edits,
decoder timing, and consumed-audio behavior must also be checked by the player
integration layer; the UI fixture must not invent a second player algorithm.

## Batch 5: playlists, layout, and downloads

Add focused scenarios for:

- Playlist create/add/rename, server failure retaining editor input, and delete
  cancellation sending no delete command. Confirmed deletion is a separate case.
- Layout resize and panel visibility persisted through reload; narrow-window tab
  scrolling keeps the active tab and necessary controls usable.
- Missing-release selection → controlled Soulseek search → folder choice →
  download progress/error. Verify failed or partial downloads do not appear
  complete, and dismiss/retry actions emit the correct intent.
- Synchronized lyric selection emits the intended Seek and maintains the
  Selected/Playing information-source distinction.

Prioritize within this batch according to the feature currently being changed.
Protect the workflow before broadening coverage of its cosmetic variants.

## CI and completion criteria

Continue using `scripts/test-ui.sh` as the UI gate. Add focused feature filtering
to the runner as the suite grows, while keeping an all-scenarios CI run.

For each batch:

1. Use actual pointer/key handlers and assert visible state plus the application
   result. For relevant persistence scenarios, close/recreate state from SQLite.
2. Include the important failure/stale/cancel case, not just successful requests.
3. Demonstrate that a relevant regression makes the scenario fail. Restore any
   intentional source mutation immediately and retain separate negative evidence.
4. Pass formatting, Clippy with test support, existing tests, and the UI suite.
   Inspect macOS/Linux CI diagnostics when a remote run is available.
5. Update the verification guide and progress log with scenario names, commands,
   results, and untested boundaries. Keep every batch independently reviewable.

Track runtime and flaky failures before adding strict time budgets. After the
fast suites are reliable, add a small native packaged-app smoke suite for
launch/reopen, focus, title-bar controls and accessibility. Pixel comparisons
and physical audio-device changes remain separate checks.
