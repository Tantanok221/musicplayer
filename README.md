# Sonora

A MusicBee-inspired Navidrome player for Apple Silicon Macs, written in Rust with GPUI Kit. The library uses a compact panel workspace: navigation and a selectable library browser on the left, albums with tracks in the center, and Queue plus information on the right. [UI research and interaction contract](docs/MUSICBEE_UI.md).

## Run

Requires macOS 14+, Apple Silicon, Apple Command Line Tools (`xcode-select --install`), and [Rust](https://rustup.rs/). The repository pins its toolchain; Cargo installs the required version automatically.

```sh
source "$HOME/.cargo/env"
cargo run --release
```

Open **Settings**, enter your Navidrome server address (including any subpath), username, and password, then choose **Connect library**. Use your server's HTTPS address; HTTP is also supported for local servers. Sonora derives a [Subsonic login token](https://opensubsonic.netlify.app/docs/api-reference/#authentication) and saves it locally instead of saving the password or using Keychain. The token is reusable and grants server access: its file is unencrypted, with owner-only permissions (`0600`) inside a private directory (`0700`).

Upgrading from the Keychain version requires signing in once. Sonora opens Settings with the existing server and username filled in; it never reads the old Keychain entry, so no migration prompt is needed. Future launches and rebuilds reuse the local token. Changing the server password requires signing in again.

## Last.fm through Navidrome

Sonora reports listening only to Navidrome. It does not make a second Last.fm submission or require Last.fm credentials locally. In Navidrome, configure its Last.fm agent and link your Last.fm account under Personal Settings. See [Navidrome’s scrobbling setup](https://www.navidrome.org/docs/usage/features/scrobbling/).

For Last.fm artist information to take priority, configure `Agents = "lastfm,deezer,listenbrainz"` on the server, using your own server-side credentials. Sonora respects the server’s configured metadata providers and fallbacks. No API keys belong in this repository. [Metadata-agent configuration](https://www.navidrome.org/docs/usage/integration/external-services/).

A listening instance survives pause/resume, seeking, output recovery, and queue restoration. Tracks longer than 30 seconds qualify after half their duration or four minutes of consumed audio, whichever comes first. Replays and repeats are separate listens. Qualified reports retain their original start timestamp. Unsent reports wait for reconnection; a durable ledger records an attempt before sending, and uncertain attempts are never automatically resent. This favors avoiding duplicates over recovering every play after a connection failure. Confirmation means Navidrome accepted the request, not proof of Last.fm delivery.

Artist profiles are cached per account for 24 hours; missing profiles use a shorter cache. Cached profiles and portraits remain usable offline. Refresh retries metadata without removing cached information. Popular songs follow Navidrome’s ranking and include only playable local tracks. Your most played uses Navidrome’s per-track counts, including artist collaborations and guest appearances. Counts refresh after confirmed submissions. Genres derive from your artist releases rather than invented Last.fm tags.

## Build a Mac application

```sh
./scripts/bundle.sh
open dist/Sonora.app
```

The script generates the icon, builds a release binary, and creates an ad hoc signed local app. Use `./scripts/bundle.sh debug` for a quicker development bundle. Public Developer ID signing, notarization, and automatic updates are not configured.

## Features

- Music Explorer, Queue, Playlists, and Now Playing workspace tabs. Explorer is the main library page and retains its view/search/filter state. Tabs scroll horizontally in narrow windows, with arrow buttons for mouse navigation.
- Now Playing highlights the current album with artwork, details, track favorites/ratings, and album tracks on the left, synchronized lyrics on the right, and a live audio visualization along the bottom. The waveform measures actual audio output and goes quiet when paused or buffering.
- The lower navigator switches between Artist, Album Artist, Year, and Genre and remembers the choice. Artist matches track credits; Album Artist opens discographies; Year and Genre filter Explorer. Counts show distinct albums.
- Album and Tracks groups artwork beside compact track lists. Album Covers emphasizes the album name with the artist as caption and expands the selected album in place; Tracks uses a simple list.
- Album Artist thumbnail browsing and an optional Genre → Album Artist → Album column browser. Columns cascade left to right; Command-click selects multiple values.
- Artist pages include cached portraits and biographies from Navidrome’s metadata agents, library-derived genre links, popular owned songs, personal most-played tracks, and similar artists. Popular songs and Your most played have separate rows with album artwork; similar artists use a circular portrait grid. Portraits also appear in the Artist/Album Artist sidebar. Unowned recommendations use muted cards and open Last.fm; owned artists open their Sonora page.
- Artist links open a full discography with separate Albums, EPs, and Singles sections, using the server's release types. Other/unclassified releases stay in their own section. Artist pages include collaborations, work independently of library filters, and support release search and inline album expansion; Back returns to the previous workspace. Command-click in artist browsers retains multi-filter selection.
- Artist credits split on `&`, `,`, and `;` (including full-width forms). Each artist has a separate link, browser entry, and discography; shared releases are counted once per artist. Individual server IDs are retained, while a combined artist ID is never assigned to every split name.
- Genre and year labels open Music Explorer with that specific filter, including album panels, artist-page release years, track details, and Now Playing. Each navigation starts a fresh filter; visible filter chips let you clear it without affecting playback.
- Simultaneous Queue and Details/Lyrics/Artwork panels, with an explicit Selected/Playing information source.
- Synchronized lyrics fade into a warm active-line highlight with eased focus and scrolling during playback and seeking; click a timed line to seek. Opening Lyrics follows the playing track. Plain lyrics remain available when the server supplies no timing.
- Resizable navigator, browser, and information panels; saved widths, visibility, view choices, and a Restore initial layout action.
- Compact windows use horizontally scrollable workspace tabs and proportionally sized side panels to keep Search and the library visible. The player stays on one horizontal line: current track and favorite on the left, shuffle/transport/repeat with elapsed time, seek, and total time in the center, and rating plus mute/volume on the right. Compact windows fold the stars into a rating menu.
- The layout menu's **Auto-hide navigator** option gives the library the full width; touch the left window edge to float the navigator over it. Queue sidebar rows include favorite and rating controls.
- Simple track lists with inline five-star ratings, favorites, duration, and play controls.
- Local Japanese/CJK and Latin substring search, including full-width and canonically equivalent Unicode text. Artist browsing and genre/year filters are available; aliases and romanization are not generated.
- Background paginated catalog sync. Failed or interrupted refreshes preserve existing records; stale records are removed only after a complete successful refresh when no server scan is reported.
- Idle playback publishes only state changes, and the catalog worker sleeps until a command or artwork retry deadline. Startup sync and sidebar playlists use summaries; full song lists load when opened, and existing offline playlist songs are preserved. Decoded artwork uses a 64 MiB / 128-entry LRU budget, with visible images kept to avoid scrolling reload loops. The Now Playing visualization animates continuously only while its window is active and music is playing.
- Offline metadata and previously cached artwork. Music playback needs the server; temporary audio buffers are not an offline-download library.
- Original-file streaming with the decoded source sample rate and channel count, without automatic resampling or server transcoding. Seeking, next-track buffering, shuffle/repeat, persistent queues, and track/album ReplayGain with peak protection.
- Queue editing, play-next actions, server playlists, favorites, ratings, and scrobbling. Failed writes remain visible and can be retried.
- Mac media keys, Now Playing text, system appearance, persistent local login, and recovery when the default audio device changes. Recovery pauses playback so you choose when to resume.

Selecting music inspects it without replacing playback. Playing an album replaces the queue with that album only. Double-clicking a track or choosing **Play Now** replaces the queue with that track and the remaining tracks on its album in disc/track order, regardless of library search or filters. Playing an existing queue entry jumps within that queue. Explicit **Queue Next** and **Queue Last** actions still add music to the current queue. Ratings and favorites are inline. The save icon above Queue saves the queue as a playlist. Playlist deletion requires an in-app confirmation.

| Shortcut | Action |
| --- | --- |
| `⌘F` | Search |
| `Space` | Play/pause outside text editing |
| `⌘→` / `⌘←` | Next/previous |
| `⌘R` | Refresh |
| `⌘,` | Settings |
| `⌘[` | Back to library |
| `⌘1`–`⌘4` | Music Explorer / Queue / Playlists / Now Playing |
| `⌘Q` | Quit and save playback state |

Closing the window keeps playback running; reopen from the Dock. Relaunch restores the queue paused.

## Validation

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Tests cover listening-session thresholds and restoration, durable duplicate prevention, timestamped Navidrome submissions with HTTP retries disabled, verified artist identities, owned-only popularity, cached biographies, personal rankings, and native 44.1 kHz mono / 96 kHz stereo / 48 kHz stereo streaming, exact PCM preservation, format-change handoff and playback timing, cascading browser selection, upstream facet counts, multi-selection unions, filtered album groups, layout persistence/upgrades, reconciliation, Unicode search, pagination/state, authenticated API requests, decoded FLAC continuity, MP3 delay/padding removal, ReplayGain, PCM boundaries, and seeking. Audio fixtures are generated sine waves, not copyrighted recordings; see `tests/fixtures/README.md`.

The interface has been exercised with a real Navidrome collection. Physical output-device transitions still need broader manual coverage. Rebuilding an ad hoc signed application may make macOS request Local Network approval again. Sonora no longer uses Keychain for login. Same-format FLAC/MP3 playback preserves gapless boundaries. Changing sample rate or channel count reopens the output and may briefly pause playback. Files are requested in their original format; unsupported decoding or output configurations show an error instead of silently transcoding or resampling. AAC gapless, bit-perfect/exclusive output, DSP, local file management, and offline audio downloads are outside this release.

## Architecture and data

One Cargo package separates the GPUI interface, SQLite catalog worker, Tokio networking tasks, playback actor/decoder, and macOS adapters. Typed commands/events connect the UI to services. Only visible artwork and track pages are requested by the renderer; SQLite and HTTP work happen off the UI thread. The output source consumes a bounded PCM channel without waiting on network or decoder work. Decoded PCM retains the source sample rate and channel count. The output opens with that exact configuration, with no alternate-rate fallback; the playback clock and visualization follow the source format. Volume and enabled ReplayGain still adjust sample levels. This is shared CoreAudio output, so downstream macOS, Bluetooth, or hardware processing may still occur; Sonora does not promise bit-perfect output.

Navidrome remains the metadata source of truth. Sonora uses documented Subsonic/OpenSubsonic endpoints and stores optional server fields without fabricating missing credits or technical data. It does not modify music files or require a server fork. The first version supports one active server/account, with separate catalogs for each account.

Data is under `~/Library/Application Support/app.Sonora.Sonora/`; artwork is under the corresponding macOS Caches directory. Paths are resolved through the `directories` crate. Artwork downloads on demand with up to four transfers at a time. Requests wait for reconnection, transient failures retry automatically up to three times, and Refresh retries exhausted requests. Artwork has a 512 MiB budget and is evicted oldest-first. Application state uses atomic file replacement or SQLite transactions. Authenticated URLs are not logged.

Dependencies retain their own licenses, including GPUI Kit (Apache-2.0), Rodio (MIT/Apache-2.0), and Symphonia (MPL-2.0). See `Cargo.lock` for the reproducible dependency set.
