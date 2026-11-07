# Resource-use investigation

Local release-build measurements on macOS / Apple silicon, 2026-11-07.
These are observations after launch with playback idle, not a battery-life benchmark
or a memory ceiling. Library size, artwork, selected page, and memory pressure affect results.

| Measurement | Before | After |
| --- | ---: | ---: |
| Idle process CPU, repeated `top` intervals | 16–33% | 0.0% |
| Physical footprint, `vmmap -summary` | 689.0 MiB | 233.1 MiB |
| Peak physical footprint in those sessions | 869.5 MiB | 290.5 MiB |
| Allocated malloc bytes | 186.7 MiB | 54.8 MiB |

The original three-second stack sample spent most main-thread time drawing and
freeing playlists/tracks. Thirteen sidebar playlists contained 23,713 song entries.
Every playback snapshot caused the UI to deep-copy those songs into click handlers,
then discard them during the next redraw. Playback sent snapshots five times per
second even while paused, persisted the queue every three seconds, and the catalog
polled every 200 ms.

The fixes:

- Sidebar queries return playlist metadata without deserializing songs. Full contents
  remain cached in SQLite for offline access and load when the playlist is opened.
  Startup sync also fetches only summaries, preventing a second allocation spike
  from downloading every playlist song. Metadata refreshes preserve cached songs;
  incomplete refreshes preserve missing playlist records.
  Playlist entries and playback queue snapshots share immutable tracks rather than
  deep-copying them during rendering.
- Paused playback emits a snapshot only when state changes. Queue persistence stops
  repeating while idle; UI and media-key commands still wake the player immediately.
- The catalog waits for commands or the exact next artwork retry deadline. It no
  longer polls SQLite for pending submissions every 200 ms while idle.
- Decoded artwork has a 64 MiB / 128-entry LRU budget, independent of the disk cache.
  Visible images can exceed this soft budget to prevent reload loops; older art is
  evicted as the user navigates. GPU textures are released when images are evicted.
- Continuous Now Playing visualization frames require active-window playback.
  Normal playback progress and lyric updates continue in the background.

In the new three-second idle sample, the main thread waited for AppKit events,
the catalog blocked on its command channel, and the player waited on its selector.
Six consecutive five-second `top` intervals in the final build showed 0.0% CPU
and about 233 MiB. An earlier run settled near 134 MiB; these figures vary with
artwork and macOS memory pressure. The final process footprint peaked near 291 MiB
during the observed session; longer-running sync and browsing can have higher peaks.
Interactive visual testing and long-duration battery measurements were left to the user.
