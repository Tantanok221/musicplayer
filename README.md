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

## Sync playback across devices

Run one Sonora sync relay for your Navidrome server. On each Sonora device, sign in to the same Navidrome account and enter the relay's HTTPS URL under **Settings → Device sync → Sync server URL**. Open **Devices** in the player bar to see what's playing elsewhere, control the active device, or move playback here. Shared preferences such as theme and ReplayGain follow the account; volume, paths, and Last.fm credentials stay on each device.

To start a new Navidrome and relay together, set `MUSIC_DIR` to your music folder and run:

```sh
docker compose -f compose.sync.example.yaml up -d --build
curl --fail http://127.0.0.1:8088/health
```

If you already run Navidrome, add only the relay service to your Compose project and set `NAVIDROME_URL` to the address the relay can reach, including any Navidrome subpath. Keep the relay's `/data` volume to retain its SQLite database across restarts. Without Docker, use `cargo build --locked --release -p sonora-sync` and run `target/release/sonora-sync` with `NAVIDROME_URL`, `BIND_ADDR` (default `127.0.0.1:8088`), and `DB_PATH` (default `sonora-sync.sqlite`).

Use TLS for access from other devices: the relay authenticates with the saved Subsonic token and salt. Keep the relay bound to loopback on the host and forward its endpoints through a reverse proxy that supports WebSocket upgrades on `/ws`. For example, a Caddy instance running on the same host can use:

```caddyfile
sync.example.com {
    reverse_proxy 127.0.0.1:8088
}
```

For private tailnet access instead, run `tailscale serve --bg 8088` on the relay host and use the HTTPS URL it prints on your devices. Use Tailscale Serve, not Funnel, to keep the endpoint private. The relay doesn't proxy your music: each device still needs access to Navidrome for streaming.

### Local network sync

Sonora uses the relay when one is configured and reachable. Without a relay, or while it's offline, devices on the same network link to each other directly. Open Sonora on each device and sign in to the same Navidrome server with the same username. There is no pairing code: Sonora trusts the devices listed in a private playlist called **Sonora devices (sync keys — do not edit)**. The list holds each device's name, public key and local network addresses. It is hidden in Sonora but may appear in other apps; don't edit it. No password, login token or private key is shared with other devices. Use HTTPS for Navidrome on networks you don't trust.

The device that last started playing is the active one, and starting playback on one device pauses the device that was playing. Other devices keep showing their own player. To control the playing device from another one, open **Devices** and choose it (it's marked **Playing**). The player bar and queue panel then show that device: you can play, pause, skip, seek, pick a song or album to play there, and add, remove or reorder songs in its queue. Undo only applies to this device's own queue. To go back to this device's own player, choose **This device** in **Devices**. Control also ends when another device starts playing.

While this device is playing, choosing another device in **Devices** moves playback there and keeps controlling it. If that device doesn't respond, playback stays where it was.

**Settings → Paired devices** lists each trusted device and whether it is **Connected**, **Found** at an address, or **Not found on this network**. Sonora remembers the device list while Navidrome is offline. **Revoke** removes trust on this device immediately and updates the Navidrome list when it's reachable. Other devices learn about a revocation the next time they refresh the list.

Sonora picks a listening port the first time it runs and keeps it, so the addresses in the device list stay valid after a restart. To choose the port yourself, type it in **Settings → Paired devices** and choose **Save port**. On macOS or Windows, allow Sonora to accept incoming connections on your private network if prompted. On Linux, allow the port through your firewall. Only one of the two devices needs to accept incoming connections. Don't forward the port to the internet. Client-isolated guest networks may block direct connections entirely; use the relay there.

## Offline use

Your library, playlists, artist information and previously viewed artwork stay available without a connection. Playing music still needs the server. Sonora reconnects automatically, and a failed or interrupted refresh never removes music you already have.

## Scrobbling, ListenBrainz and Last.fm

Sonora reports plays only to Navidrome. To scrobble to ListenBrainz or Last.fm, link your account in Navidrome's personal settings ([Navidrome scrobbling guide](https://www.navidrome.org/docs/usage/features/scrobbling/)). A play counts after half the track or four minutes, whichever comes first. Plays made offline are sent when you reconnect, and a play is never sent twice.

Artist biographies, related artists and popular songs also come from Navidrome, through its metadata agents. Deezer and ListenBrainz need no API key; set Navidrome's `ND_AGENTS` (for example `deezer,listenbrainz,lastfm`) to choose the order ([Navidrome external integrations](https://www.navidrome.org/docs/usage/integration/external-services/)).

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

Then choose Edit Tags… on a song, select several songs with ⌘-click or shift-click and choose Edit Tags of N Songs…, or choose Edit Album Tags… on an album. With several songs, track number, disc, title and artist are edited per song in a table; the other fields apply to all of them, and fields that differ show `<multiple values>` and are kept unless you change or clear them. Find on MusicBrainz searches for the album as it is tagged, lets you pick the edition, matches songs to its tracks and fills in the fields you tick, including MusicBrainz IDs. For a single song it searches recordings by title, artist and length instead. Songs from several albums are looked up one album at a time: pick each album's edition or skip it, then review everything together. You can also paste a MusicBrainz release or recording link, choose how many of the release's genres to fill, embed the front cover from the Cover Art Archive, or fill fields from file names with a pattern such as `%track% - %title%`. With an AcoustID application key in Settings, Identify by Sound fingerprints a song's file and looks its recording up on AcoustID. Nothing is written until you review the changes and confirm. Sonora checks that each file's size matches what Navidrome reports and records the change so you can undo it. When the new tags fit in a FLAC file's tag area, only that area is rewritten, which takes a moment even over a network share; its original is kept until the write finishes and put back if Sonora is stopped partway. Otherwise Sonora writes to a copy and renames it over the original. The editor closes as soon as you confirm and a bar shows the progress, so you can keep browsing and queue further edits. Sonora also asks Navidrome to rescan when your account is an administrator. Otherwise Navidrome's file watcher picks the change up. Favorites, ratings and play counts stay in Navidrome. When a rescan gives a song a new ID (Navidrome bases IDs on tags for files without MusicBrainz IDs), Sonora carries its favorite and rating over.

## License

MIT. Dependencies keep their own licenses; see `Cargo.lock`.
