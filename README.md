# Sonora

A MusicBee-inspired Navidrome player for macOS and Linux. Navigation and a library browser sit on the left, albums with their tracks in the center, and the queue and track information on the right.

## Install

### macOS

Requires Apple Silicon, Apple Command Line Tools (`xcode-select --install`) and [Rust](https://rustup.rs/). The required Rust version is installed automatically.

```sh
./scripts/bundle.sh
open dist/Sonora.app
```

This builds a locally signed `dist/Sonora.app`. To update, rebuild and replace your installed app while Sonora is closed. Rebuilding may make macOS ask for Local Network access again.

### Linux

Requires a Wayland or X11 session with a working Vulkan driver, Rust, a C/C++ toolchain, CMake, pkg-config, ALSA, D-Bus and the [GPUI Kit system libraries](https://gpui-kit.com/docs/installation/). On Arch Linux: `base-devel rust rustfmt cmake fontconfig libxkbcommon libxcb wayland vulkan-icd-loader alsa-lib`, plus your GPU's Vulkan driver.

```sh
./scripts/install-linux.sh
```

This installs `~/.local/bin/sonora` and a **Sonora** launcher entry, without administrator access. To update, run `git pull --ff-only` and the installer again; a failed build keeps the previous version. To uninstall, remove `~/.local/bin/sonora`, `~/.local/share/applications/app.sonora.player.desktop` and `~/.local/share/icons/hicolor/512x512/apps/app.sonora.player.png`.

## Connect your library

Open **Settings**, enter your Navidrome address (including any subpath), username and password, and choose **Connect library**. Sonora saves a Subsonic login token, not your password, in an owner-only file. The token grants access to your server, so keep your user account private. Changing your server password requires signing in again.

## Using Sonora

- **Music Explorer** is the main library page: search (including Japanese/CJK and full-width text), filter by genre, year, artist or record label, and sort by artist, album, release date, recently added, rating or play count. Your sort is remembered per account.
- **Artist pages** show a portrait and biography, popular songs, your most played, similar artists, and a full discography split into albums, EPs and singles.
- **Discover** brings back music you own but have stopped playing: *Old rotation* lists albums you played a lot but not in the last six months, and *Forgotten favorites* lists favorite albums and albums with loved songs you haven't played in six months. Click an album to open it, or shuffle a whole shelf.
- **Queue** separates Now playing from Up next. Drag to reorder, select several entries to remove, and undo the last edit. Right-click any track or album for Play Next, Add to End of Queue, Add to Playlist, Go to Album and Go to Artist.
- **Now Playing** shows the current track over its album art, with synchronized lyrics (click a line to seek) and a spectrum of the audio.
- **Favorites and ratings** for songs and albums update immediately. If saving fails, the previous value comes back with a Retry button.
- **Tabs** each keep their own page and Back/Forward history, and are restored when you reopen Sonora.
- Panels can be resized and hidden, and **Restore initial layout** resets them.
- Playback streams original files, with gapless FLAC/MP3, shuffle, repeat and ReplayGain (Track, Album or Smart). The queue is restored, paused, when you reopen Sonora.

| Shortcut | Action |
| --- | --- |
| `⌘F` | Search |
| `Space` | Play/pause outside text fields |
| `⌘→` / `⌘←` | Next / previous |
| `⌘T` / `⌘W` / `⌘⇧T` | New tab / close tab / reopen closed tab |
| `⌘[` / `⌘]` | Back / forward |
| `⌘1`–`⌘9` | Switch to tab 1–9 |
| `⌘Z` | Undo the last queue edit |
| `⌘R` | Refresh library |
| `⌘,` | Settings |
| `⌘Q` | Quit and save playback state |

Linux uses Ctrl instead of ⌘. On macOS, closing the window keeps music playing; reopen Sonora from the Dock. On Linux, closing the last window quits.

## Offline use

Your library, playlists, artist information and previously viewed artwork stay available without a connection. Playing music still needs the server. Sonora reconnects automatically, and a failed or interrupted refresh never removes music you already have.

## Scrobbling and Last.fm

Sonora reports plays only to Navidrome. To scrobble to Last.fm, set up Navidrome's Last.fm agent and link your account in Navidrome's personal settings ([Navidrome scrobbling guide](https://www.navidrome.org/docs/usage/features/scrobbling/)). A play counts after half the track or four minutes, whichever comes first. Plays made offline are sent when you reconnect, and a play is never sent twice.

## Missing releases and downloads

Artist pages end with **Missing from your library**: official releases from MusicBrainz that your library lacks. Select one to see its details or hide it.

To download, connect your own [slskd](https://github.com/slskd/slskd) in **Settings → Downloads** with its address and an API key (`readwrite` role) or its web login. Choose **Find on Soulseek**, pick a folder from the results, and Sonora follows the download, then asks Navidrome to scan (administrators only). Sonora never searches or downloads on its own. slskd must save into a folder Navidrome scans.

These lookups send artist and release identifiers to MusicBrainz and the Cover Art Archive, never your server address or login.

## Your data

- macOS: `~/Library/Application Support/app.Sonora.Sonora/`, with artwork in the matching Caches folder.
- Linux: `~/.local/share/sonora/library/`, with cache in `~/.cache/sonora/library/`.

Each account has its own library. Sonora never modifies your music files.

## License

MIT. Dependencies keep their own licenses; see `Cargo.lock`.
