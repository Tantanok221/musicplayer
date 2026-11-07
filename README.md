# Sonora

A MusicBee-inspired Navidrome player for Apple Silicon Macs, written in Rust with GPUI Kit. The library uses a compact panel workspace: navigation and artist browsing on the left, albums with tracks in the center, and Queue plus information on the right. [UI research and interaction contract](docs/MUSICBEE_UI.md).

## Run

Requires macOS 14+, Apple Silicon, Apple Command Line Tools (`xcode-select --install`), and [Rust](https://rustup.rs/). The repository pins its toolchain; Cargo installs the required version automatically.

```sh
source "$HOME/.cargo/env"
cargo run --release
```

Open **Settings**, enter your Navidrome server address (including any subpath), username, and password, then choose **Connect library**. Use your server's HTTPS address; HTTP is also supported for local servers. Credentials are saved in macOS Keychain, never in the catalog or a config file.

## Build a Mac application

```sh
./scripts/bundle.sh
open dist/Sonora.app
```

The script generates the icon, builds a release binary, and creates an ad hoc signed local app. Use `./scripts/bundle.sh debug` for a quicker development bundle. Public Developer ID signing, notarization, and automatic updates are not configured.

## Features

- Music, Now Playing, Playlists, and Music Explorer workspace tabs. Music and Explorer keep separate view/search/filter state.
- Album and Tracks groups artwork beside compact track lists. Album Covers emphasizes the album name with the artist as caption and expands the selected album in place; Tracks uses a simple list.
- Album Artist thumbnail browsing and an optional Genre → Album Artist → Album column browser. Columns cascade left to right; Command-click selects multiple values.
- Simultaneous Queue and Details/Lyrics/Artwork panels, with an explicit Selected/Playing information source.
- Synchronized lyrics highlight and follow the current line during playback and seeking; click a timed line to seek. Opening Lyrics follows the playing track. Plain lyrics remain available when the server supplies no timing.
- Resizable navigator, browser, and information panels; saved widths, visibility, view choices, and a Restore initial layout action.
- Simple track lists with inline five-star ratings, favorites, duration, and play controls.
- Local Japanese/CJK and Latin substring search, including full-width and canonically equivalent Unicode text. Artist browsing and genre/year filters are available; aliases and romanization are not generated.
- Background paginated catalog sync. Failed or interrupted refreshes preserve existing records; stale records are removed only after a complete successful refresh when no server scan is reported.
- Offline metadata and previously cached artwork. Music playback needs the server; temporary audio buffers are not an offline-download library.
- FLAC/MP3 direct streaming, seeking, next-track buffering, shuffle/repeat, persistent queues, and track/album ReplayGain with peak protection.
- Queue editing, play-next actions, server playlists, favorites, ratings, and scrobbling. Failed writes remain visible and can be retried.
- Mac media keys, Now Playing text, system appearance, Keychain storage, and recovery when the default audio device changes. Recovery pauses playback so you choose when to resume.

Selecting music inspects it without replacing playback. Double-click or use **Play Now** to play; right-click for **Queue Next** or **Queue Last**. Library playback uses all matching tracks, including those beyond the currently loaded page. Ratings and favorites are inline. The save icon above Queue saves the queue as a playlist. Playlist deletion requires an in-app confirmation.

| Shortcut | Action |
| --- | --- |
| `⌘F` | Search |
| `Space` | Play/pause outside text editing |
| `⌘→` / `⌘←` | Next/previous |
| `⌘R` | Refresh |
| `⌘,` | Settings |
| `⌘[` | Back to library |
| `⌘1`–`⌘4` | Music / Now Playing / Playlists / Music Explorer |
| `⌘Q` | Quit and save playback state |

Closing the window keeps playback running; reopen from the Dock. Relaunch restores the queue paused.

## Validation

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Tests exercise cascading browser selection, upstream facet counts, multi-selection unions, filtered album groups, layout persistence/upgrades, reconciliation, Unicode search, pagination/state, authenticated API requests, decoded FLAC continuity, MP3 delay/padding removal, ReplayGain, PCM boundaries, and seeking. Audio fixtures are generated sine waves, not copyrighted recordings; see `tests/fixtures/README.md`.

The interface has been exercised with a real Navidrome collection. Physical output-device transitions still need broader manual coverage. Rebuilding an ad hoc signed application may make macOS request Keychain or Local Network approval again; approve those prompts to reconnect. Gapless behavior is required for FLAC/MP3 fixtures; AAC gapless, bit-perfect/exclusive output, DSP, local file management, and offline audio downloads are outside this release. Other formats attempt direct decoding and then server MP3 transcoding.

## Architecture and data

One Cargo package separates the GPUI interface, SQLite catalog worker, Tokio networking tasks, playback actor/decoder, and macOS adapters. Typed commands/events connect the UI to services. Only visible artwork and track pages are requested by the renderer; SQLite and HTTP work happen off the UI thread. The output source consumes a bounded PCM channel without waiting on network or decoder work. Audio is converted to 48 kHz stereo before it reaches the system mixer.

Navidrome remains the metadata source of truth. Sonora uses documented Subsonic/OpenSubsonic endpoints and stores optional server fields without fabricating missing credits or technical data. It does not modify music files or require a server fork. The first version supports one active server/account, with separate catalogs for each account.

Data is under `~/Library/Application Support/app.Sonora.Sonora/`; artwork is under the corresponding macOS Caches directory. Paths are resolved through the `directories` crate. Artwork downloads on demand with up to four transfers at a time. Requests wait for reconnection, transient failures retry automatically up to three times, and Refresh retries exhausted requests. Artwork has a 512 MiB budget and is evicted oldest-first. Application state uses atomic file replacement or SQLite transactions. Authenticated URLs are not logged.

Dependencies retain their own licenses, including GPUI Kit (Apache-2.0), Rodio (MIT/Apache-2.0), and Symphonia (MPL-2.0). See `Cargo.lock` for the reproducible dependency set.
