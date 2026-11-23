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

To let Memory lane reach back through Last.fm history, [create a Last.fm API key](https://www.last.fm/api/account/create) and enter it with your Last.fm username in Settings → Library. The key is kept in a private file on this Mac.

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
- **Discover** brings back albums you once had on repeat but haven't played in six months. *Memory lane* groups them by season, newest first, and each album says how often you played it. Without Last.fm, seasons come from Navidrome's play counts and last-played dates. Add your Last.fm username and API key in Settings → Library and Memory lane reaches back through every season you've scrobbled: Sonora reads your public album charts once, one season at a time, keeps them on this Mac and puts each album in the season you played it most. Each season shows its eight most played albums with **See all** for the rest, **Shuffle** plays the whole lane, and albums select and play just like in Music Explorer. Right-click an album for **Not now** (hidden for 30 days) or **Never suggest**.
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

Each account has its own library. Sonora doesn't modify your music files unless you turn on tag editing.

### Tag editing (opt-in)

Navidrome can't save tags, so Sonora can write them into your files itself. This is off by default. To use it, open Settings → Metadata, set Music folder to a folder that holds the same files as Navidrome's library (a mounted SMB/NFS share, or the folder itself when Navidrome runs on this computer), and turn on Write tags to files. In Navidrome, turn on Report Real Path for the Sonora player so Sonora can find each file.

Then choose Edit Tags… on a song, select several songs with ⌘-click or shift-click and choose Edit Tags of N Songs…, or choose Edit Album Tags… on an album. With several songs, track number, disc, title and artist are edited per song in a table; the other fields apply to all of them, and fields that differ show `<multiple values>` and are kept unless you change or clear them. Find on MusicBrainz searches for the album as it is tagged, lets you pick the edition, matches songs to its tracks and fills in the fields you tick, including MusicBrainz IDs. For a single song it searches recordings by title, artist and length instead. Songs from several albums are looked up one album at a time: pick each album's edition or skip it, then review everything together. You can also paste a MusicBrainz release or recording link, choose how many of the release's genres to fill, embed the front cover from the Cover Art Archive, or fill fields from file names with a pattern such as `%track% - %title%`. With an AcoustID application key in Settings, Identify by Sound fingerprints a song's file and looks its recording up on AcoustID. Nothing is written until you review the changes and confirm. Sonora checks that each file's size matches what Navidrome reports, writes to a copy and renames it over the original, records the change so you can undo it, and asks Navidrome to rescan when your account is an administrator. Otherwise Navidrome's file watcher picks the change up. Favorites, ratings and play counts stay in Navidrome. When a rescan gives a song a new ID (Navidrome bases IDs on tags for files without MusicBrainz IDs), Sonora carries its favorite and rating over.

## License

MIT. Dependencies keep their own licenses; see `Cargo.lock`.
