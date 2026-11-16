# Making Sonora easier for coding agents to verify

Audit date: 2026-11-16. Source reviewed at `4ad37ac`.

This document preserves the audit at that revision. The approved first testing
slice is now implemented in the working tree; current validation and handoff
status are in [AGENT_TESTING_PROGRESS.md](AGENT_TESTING_PROGRESS.md), with usage
instructions in [TESTING.md](TESTING.md). Findings below describe the baseline.

The highest-value investment is a repeatable way to drive the production UI
against isolated, predictable services and assert the resulting behavior. More
unit tests alone will not cover the current gap between data models, event
handlers, rendering, persistence, and the packaged application.

This is the broader implementation roadmap; the progress document distinguishes
the completed first slice from later proposed work.

## Current foundation

- The source contains 152 ordinary Rust/Tokio test declarations. Tests cover
  catalog reconciliation, sorting, HTTP adapters, audio decoding, queue logic,
  listening reports, and other domain behavior. This count is not a coverage
  measurement.
- Typed `Command`, `Event`, `PlayerCommand`, and `PlayerEvent` messages already
  separate the interface from services and playback.
- Tests already use temporary SQLite databases, loopback Axum servers, and
  generated audio. These are reusable building blocks for larger scenarios.
- CI checks formatting and runs Clippy and tests on macOS and Linux.
- Many controls have accessible roles/names and domain-derived IDs; tabs and
  album feedback provide useful examples.

Evidence: [service boundary](../src/service.rs), [existing HTTP fixtures](../tests/artist_server.rs),
[audio fixtures](../tests/fixtures/README.md), [CI](../.github/workflows/ci.yml).

## Gaps, in implementation order

### 1. No tests drive the production application view

There are no `#[gpui_kit::test]`, `TestAppContext`, or `VisualTestContext`
tests in Sonora, and no tests construct `MusicApp` or start `Service`.
The tests beside UI code currently exercise helpers rather than rendered
interactions. For example, the tab regression assigns `active_mut().page`
directly; it does not invoke the artist click handler, render the strip,
wait for saving, or reopen the application.

Enable `gpui-kit = { version = "=0.9.0", features = ["test-support"] }` under
development dependencies. The installed 0.9.0 package already contains
`TestAppContext`, `TestWindowExt`, `TestSupportExt`, and compiled UI examples.
Use the production view in tests. Tests can initially live beside `ui` in the
binary; alternatively expose a narrow application-construction seam through
the library if separate integration targets are preferable.

Acceptance: a headless test clicks New tab from a middle tab, selects an artist
through the actual navigator, asserts the active tab's ID and its horizontal
position relative to its neighbors, and verifies its title and selected state.
Check pointer and keyboard creation paths.

Evidence: [dependencies](../Cargo.toml), [UI tests](../src/ui.rs),
[tab regression](../src/browser.rs), [tab handlers](../src/ui/tabs.rs).
Framework reference: [GPUI Kit testing guide](https://gpui-kit.com/docs/test/).
In-process element/layout tests and pixel rendering are separate mechanisms;
neither alone proves behavior of an installed native app.

### 2. Application startup cannot select an isolated test environment

`main` constructs `Paths::new()` and starts the real service. `Paths` can already
represent temporary directories through its public fields, but the executable
offers no way to supply them. Running it normally therefore depends on the
developer's saved account, catalog, queue, window frame, and preferences.

Add explicit launch configuration for an isolated data/cache root and fixture
endpoints. Keep fixture launching in test/development tooling. Seed a small,
versioned catalog using normal catalog APIs: multiple artists, duplicate album
titles, collaborations, Unicode names, missing metadata, playlists, lyrics,
and known artwork. Include a larger generated catalog for virtualization tests.

Acceptance: one documented command launches a known scenario without personal
credentials, personal data, or Internet access. Two runs can execute in parallel
without sharing state. Relaunching with the same test root preserves preferences
and the queue; a fresh root starts clean.

Evidence: [startup](../src/main.rs), [paths](../src/platform.rs),
[catalog APIs](../src/catalog.rs).

### 3. Service and playback dependencies are difficult to replace independently

`Service::start` always starts `PlayerHandle`. The worker constructs
`MusicBrainz::new()` internally, although that client already supports custom
endpoints. Playback inspects the default audio device and creates OS media
controls. These choices make complete feature scenarios harder to isolate.

Introduce narrow constructor seams for the existing boundaries:

- UI tests: channel-backed service/player handles that record commands and
  deliver controlled events through the real event-receive path.
- Service integration tests: real service worker, real temporary SQLite catalog,
  loopback HTTP servers, injectable metadata clients, and a controlled player.
- Playback integration tests: real decoder and queue with a deterministic PCM
  consumer and media-control adapter. Advance consumed audio to advance playback
  time; do not substitute elapsed wall time for consumption.

Keep these layers distinct. A UI test with a fake backend proves UI behavior;
a smaller set of full application scenarios must verify the real wiring too.

Acceptance: test successful sync, interrupted sync, stale search/artist replies,
optimistic mutation success/failure, retry, and shutdown with bounded waits.
All actors and tasks stop after each scenario. Unexpected external requests fail
the scenario instead of silently reaching production services.

Evidence: [service startup and worker](../src/service.rs),
[playback startup/output](../src/playback.rs), [metadata endpoints](../src/musicbrainz.rs).

### 4. UI targets and state assertions are not a consistent test contract

Tabs use stable tab IDs, but navigator facets and several cover/browser rows use
list indexes. Many interactive surfaces are custom `div` elements. No Sonora
element currently opts into GPUI Kit's `.test_support()` observation mechanism.
Accessible names help native automation, but do not automatically make every
custom element queryable by the framework test helpers.

Use stable domain IDs for mutable/reordered collections; queue rows need
occurrence identity because the same song can appear twice. Register important
custom elements for test observation. Expose selection, disabled, loading,
error, and expansion state where relevant. Prefer standard controls when they
already provide the required keyboard/focus behavior.

Acceptance: tests locate targets without coordinates or transient AX indexes.
Filtering, scrolling a virtualized list, changing tab titles, and rerendering
cached sections do not change which domain object a target represents.
Native accessibility smoke tests also verify the complete tree after cached
section updates; in-process observations do not prove native AX behavior.

Evidence: [navigator row IDs](../src/ui.rs), [tab IDs](../src/ui/tabs.rs),
[album controls](../src/ui/album_feedback.rs), [cached sections](../src/ui/sections.rs).

### 5. Feature transitions and timing are intertwined with the large root view

`src/ui.rs` is about 8,300 lines. `MusicApp` owns navigation, service-event
handling, inputs, playback presentation, mutation state, and rendering. The
root render also synchronizes the active tab and can schedule persistence.
Tab saving has a 500 ms debounce; service and playback use real clocks and
background threads. This increases the setup required to test an individual
transition and invites timing-dependent assertions.

Extract coherent state transitions incrementally as features are changed:
navigation/history, pending mutations, and playlist editing are useful first
candidates. Let transitions request effects that existing owners execute.
Make render observational where practical. Control relevant timers in the
harness and inject clocks or random seeds only at boundaries that need them.

Acceptance: tests wait for semantic conditions such as an expected catalog
revision or completed preference write, with a deadline and diagnostic state.
They do not depend on arbitrary sleeps. Debounce/retry tests can advance their
controlled clock. Avoid a whole-app rewrite before the first UI scenario works.

Evidence: [root state/render/event handling](../src/ui.rs),
[debounced saving](../src/ui/tabs.rs), [service loop](../src/service.rs).

### 6. CI provides no UI, native-app, or visual regression evidence

The workflow runs formatting, Clippy, and ordinary tests. It does not launch the
bundle, exercise a feature through the UI, render screenshot comparisons, or
upload UI failure artifacts.

Add three complementary checks:

1. Fast GPUI interaction tests on each PR, using production views and controlled
   dependencies.
2. Selected offscreen pixel/layout scenarios with fixed window size, theme,
   assets, playback position, and animation phase. Configure the actual renderer;
   framework interaction tests alone do not inspect pixels.
3. A small native application suite for packaged launch, accessibility, focus,
   title-bar controls, shortcuts, window close/reopen, and restart persistence.
   Establish a reliable graphical runner before making this a required check.

Failure artifacts should include the scenario/seed, application revision,
expected versus observed state, relevant element bounds, sanitized command/event
history, and a screenshot when rendering is available. Log summaries with IDs;
never dump credential-bearing commands or authenticated URLs.

Acceptance: an agent can investigate a CI failure from the artifacts without
asking a person to reproduce the feature locally. Normal PR runs do not depend
on an audio device or a developer's Navidrome server.

Evidence: [workflow](../.github/workflows/ci.yml), [bundler](../scripts/bundle.sh).

### 7. Agents lack a feature-to-verification map

`AGENTS.md` lists global checks but does not map features to owners, fixtures,
focused commands, acceptance scenarios, or known native-only checks. The UI
contract and README describe behavior, but those promises are not connected to
executable feature scenarios.

Add a short testing guide and feature scenario index. For each feature record
its owner files, setup, stable targets, actions, expected UI and persisted state,
failure cases, and the exact runnable command. Add focused commands only when
they exist and pass. For a UI fix, require a test through the relevant input
handler plus the resulting visible state, rather than only direct model edits.

Acceptance: an agent changing tabs, ratings, or playlists can find and run the
right checks without reading the entire root view. PR evidence names what was
actually exercised and distinguishes simulated, rendered, and native coverage.

Evidence: [agent instructions](../AGENTS.md), [UI contract](MUSICBEE_UI.md).

## Initial feature scenarios

| Feature | First scenario | Important extension |
| --- | --- | --- |
| Tabs/navigation | New tab → artist → same rightmost tab ID/order | Back/Forward, close/reopen, overflow, restart persistence |
| Explorer | Search/filter/sort → correct visible releases | Unicode, empty result, upstream facets, virtualized scrolling |
| Favorites/ratings | Click → optimistic display → confirmed catalog state | Duplicate click blocked, failure rollback, Retry |
| Queue/playback | Play album → queue order → next/pause/seek | Duplicate song occurrences, reorder/undo, restart paused |
| Playlists | Create → add album → rename → reopen | Server failure, delete cancellation, offline cached songs |
| Sync/offline | Interrupted refresh → cached items remain visible | Reconnect → complete reconciliation, account isolation |
| Artist/downloads | Artist → missing release → controlled search/download | Stale artist response, partial download, scan failure/recovery |
| Layout/lyrics | Resize/switch panels → usable controls and saved layout | Narrow window, long titles, timed lyric seek, themes |

## Suggested delivery sequence

1. **First vertical slice:** enable GPUI test support, provide controlled
   service/player handles, register tab/navigator targets, and add the tab →
   artist interaction scenario to CI. This validates the testing approach on a
   real regression before expanding infrastructure.
2. **Real-service isolation:** reusable seeded catalog and loopback servers,
   injectable service dependencies, launch configuration, deterministic output,
   shutdown/readiness helpers, and the restart/offline scenarios.
3. **Feature expansion:** ratings rollback, Explorer, queue, playlists, and
   downloads. Add diagnostics and focused commands as scenarios land.
4. **Native and visual checks:** establish the graphical runner and a small
   stable suite, then expand coverage according to escaped regressions.

Measure progress by features with complete interaction scenarios, escaped
regressions, flaky reruns, and time needed to diagnose a failure. Raw test count
and line coverage do not show whether a user workflow is protected.

Routine interaction/persistence changes should become reproducible agent checks.
Product/design judgment, physical audio-device transitions, and first-time OS
permission behavior retain targeted human review. Reduce that work by making
each automated claim specific and reproducible.
