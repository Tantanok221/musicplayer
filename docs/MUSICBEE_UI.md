# MusicBee UI research and Sonora's implementation

The reference is MusicBee's library workspace, especially **Album and Tracks**, rather than a streaming-service home screen. Research was checked against the publisher's Microsoft Store screenshot and MusicBee's own community manual/screenshots. The manual pages describe older versions, so screenshots are used to corroborate their panel structure.

References:

- Publisher screenshot: https://apps.microsoft.com/detail/9p4clt2rj1rs
- Main views and context commands: https://musicbee.fandom.com/wiki/Main_Panel_Views
- Thumbnail browser, library explorer, cascading column browser: https://musicbee.fandom.com/wiki/Library_Navigation_Elements
- Panel arrangement and playing/selected track distinction: https://musicbee.fandom.com/wiki/Layout_Preferences
- Playback and queue behavior: https://musicbee.fandom.com/wiki/Playback
- Album and Tracks screenshot: https://getmusicbee.com/forum/index.php?topic=37524.0

## Interaction contract

1. Compact top workspace tabs switch Music, Now Playing, Playlists, and Music Explorer. Music and Explorer remember their own views and search/filter state.
2. The left panel contains navigation, playlists, and an Album Artist thumbnail browser. Choosing an artist filters the main panel in place. All Artists restores that stage.
3. Optional Genre → Album Artist → Album browser columns filter left to right. Changing an upstream selection resets downstream selections; Command-click unions/toggles values inside a column. Counts and choices reflect upstream filters.
4. The central view switches between Album and Tracks, Album Covers, and Tracks. Album and Tracks shows successive album blocks with artwork beside compact track lists. Covers selects/expands an album in place. No separate oversized album landing page.
5. Selecting an album or track does not start playback or replace the queue. Double-click/Play Now starts playback; Queue Next/Queue Last preserve the current playing track. The play action includes all matching tracks, not just the first loaded page.
6. The right sidebar simultaneously shows Queue and track information. Details, Lyrics, and Artwork tabs have an explicit Selected/Playing source selector. Opening Lyrics follows the playing track; synchronized lyrics highlight and follow the active line, including after seeking. Clicking a timed line seeks to it. Plain lyrics remain readable without fabricated timing.
7. Navigation and information panels resize with dividers and can be hidden. Their visibility, widths, browser visibility, view choices, and information-panel choices persist. Restore layout restores these settings.
8. The player is a compact, flat strip with previous, play/pause, and next controls, progress, volume, and technical playback information. There is no Stop button. Status reports the current library/selection and queue separately.
9. Track lists stay simple: number/title, duration, favorite, and five-star rating; no spreadsheet table controls. This preserves the user's explicit preference.

## Server boundary

The existing user-entered Navidrome connection and Keychain credentials remain the source of music. No demonstration library, hardcoded credentials, fake waveforms, folder-management UI, or nonfunctional radio/device nodes are added. Local file tagging and MusicBee's broader plugin/device/CD features are beyond this UI adaptation.
