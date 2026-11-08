# Music Explorer sorting proposal

Research checked on 2026-11-08. After trying the implementation, the user requested removal of the custom editor and saved presets. The shipped UI now uses built-in presets with simple direction choices; Artist discography exposes oldest/newest album order directly. Natural Unicode keys, metadata-backed ordering, persistence, and pagination checks remain. The custom-editor sections below record the earlier proposal and are not part of the application. See the README for current behavior.

## Recommendation

Replace the single-field menu with saved, ordered sorting rules. Offer useful presets immediately, and put customization behind **Custom sorting…**. Treat album ordering, track ordering, and visual grouping as distinct choices. Keep Sonora's compact album panels and simple track rows.

Recommended album default: **Artist discography** — album artist A–Z, original release date oldest first, then album title A–Z. Inside each album, retain disc/track order. Offer **Artist / Album** alongside it for alphabetical browsing. Remember the user's choice rather than restoring a default on launch.

## Current behavior and concrete gaps

- `src/ui.rs` exposes five choices: Album Artist / Album, Album, Year, Track title, and Rating. It has no selected-item indicator or direction control.
- `LibraryQuery` stores one `Sort` and one `descending` flag. Its sorting choice survives workspace navigation in memory, but it is not part of persisted preferences and starts from the default after relaunch.
- `Catalog::view` only implements artist, year, and album-name ordering for albums. Track title and Rating both fall through to album-name ordering in album layouts.
- `Catalog::order` interprets `Sort::Artist` as track artist, although the menu calls it Album Artist / Album. Ties go to opaque album IDs instead of an intentional artist/album hierarchy.
- Added, Plays, Duration, and Format exist in the enum but are not offered by the menu. Added currently means SQLite row insertion order, not the server's date added.
- Album tracks are deliberately requested in album/disc/track order. Preserve this behavior independently of the outer album sort.
- Album and Tracks inserts an artist heading whenever the artist differs from the preceding album, even when albums are sorted by name or year. That can produce repeated artist headings instead of meaningful groups.
- Sorting uses SQLite `NOCASE`, whose case folding covers ASCII rather than full Unicode. Existing search normalization does not change this ordering behavior. [SQLite documentation](https://www.sqlite.org/datatype3.html#collation)

## Lessons from other players

| Reference | Relevant behavior | Design lesson for Sonora |
| --- | --- | --- |
| MusicBee | Custom sorting sets combine fields with individual directions. Its developer recommends an album-level date-added field to preserve album track order. | Model an ordered sequence of rules; order album objects independently of their tracks. [Sorting example](https://getmusicbee.com/forum/index.php?topic=6841.0), [developer's album-date explanation](https://getmusicbee.com/forum/index.php?topic=25812.0) |
| MusicBee | Custom sort values and configurable ignored words coexist. Developer posts in 2026 also address consistency between sorting, browsers, and type-to-find. | Keep display labels separate from sort keys, and make jump navigation use the same keys. [Developer discussion](https://getmusicbee.com/forum/index.php?topic=38931.0) |
| foobar2000 / Facets | Its documented features include a default sort pattern, field-based formatting, ignoring a leading article without changing the displayed name, and aggregate statistics with explicit meanings. | Support reusable rules and define album statistics explicitly. This refers to the documented Facets component, not every foobar2000 interface. [Official component listing](https://www.foobar2000.org/components/view/foo_facets), [linked documentation](https://wiki.hydrogenaudio.org/index.php?title=Foobar2000%3AComponents%2FFacets_%28foo_facets%29) |
| Roon | Album and track browsers have separate sort controls; browser preferences cover release dates, artist names, and compilations. Display sorting is separate from play actions. | Remember settings by browsing scope and expose musical preferences in understandable terms. [Roon documentation](https://help.roonlabs.com/portal/en/kb/articles/sorting) |

MusicBee's older forum examples establish the design precedent; they are not a claim that every current view has identical behavior. Some direct forum/wiki fetches were blocked, so those findings also use indexed excerpts. The recommendations below are Sonora-specific decisions.

## User interface

The toolbar should show a compact label such as **Sort: Artist discography** with a menu. On narrow windows, retain the icon and put the full rule summary in its tooltip. Checkmark the active preset.

Album and Tracks and Album Covers share album-order preferences; Tracks remembers a separate track-order preference. Switching scope restores its last choice. Searching, clearing filters, changing workspaces, and relaunching preserve sorting.

The album menu offers:

| Preset | Ordering |
| --- | --- |
| Artist discography | Album artist A–Z → original release date oldest first → album title A–Z |
| Artist / Album | Album artist A–Z → album title A–Z |
| Album title | Album title A–Z → album artist A–Z |
| Release date | Original release date newest first → album artist A–Z → album title A–Z |
| Recently added | Server album date added newest first → album artist A–Z → album title A–Z |
| Highest rated | Server album rating highest first → album artist A–Z → album title A–Z |
| Most played | Server album play count highest first → album artist A–Z → album title A–Z |

Tracks offers Title, Track artist / Title, Album order, Recently added, Highest rated, Most played, and Duration. **Track artist** and **Album artist** are distinct fields throughout.

Each simple preset has an explicit primary direction: A–Z/Z–A, oldest/newest, highest/lowest, or shortest/longest. Changing it reverses the primary rule only; the rest remain visible in the summary. Artist discography uses **Custom sorting…** to change its inner date direction without reversing artist order.

**Custom sorting…** opens a compact sheet with ordered rows:

```text
Custom sorting

Order albums by
  Album artist          A–Z             ↑ ↓ Remove
  Original release date Newest first    ↑ ↓ Remove
  Album title           A–Z             ↑ ↓ Remove
  + Add rule

Group headings: None
Within albums: Disc → Track number

Save as preset…                  Cancel  Apply
```

Allow adding, removing, and moving rules, with independent directions and named presets. Show a preview from the current filtered library before Apply. Start with a field picker, rather than a scripting language. The sheet edits album rules in album scope and track rules in track scope; it never presents fields that silently do nothing.

Visual grouping is explicit: None, Album artist, Year, or Release type. Selecting a group creates a visible outer rule with its own direction. Sort rules then apply inside each group. A global **Recently added** preset selects no grouping; an **Artist discography** preset can select album-artist grouping. No hidden grouping rule should override the chosen order. Use single-valued group fields initially, so multi-genre/multi-artist membership does not duplicate albums.

## Ordering contract

1. Album layouts sort albums as units identified by server album ID, preserving separate editions and compilations. Sorting never merges releases by a matching title.
2. Album contents remain disc number, track number, title, then stable track ID. A missing disc can use disc 1 for comparison without rewriting metadata; missing track numbers follow numbered tracks.
3. Tracks scope supports global song ordering. Album-order presets explicitly include album identity before disc/track fields.
4. Unknown dates and unrated items sort last in either direction. A known zero play count remains a real number. Missing and zero are not automatically interchangeable.
5. Partial release dates retain their actual precision. Within the same year, month/day break ties when present; choose and test an explicit rule for less precise dates rather than presenting invented January 1 metadata.
6. Original release date falls back to edition release date, then year. Show that fallback in the date preference help. Offer edition release date as a separate field for collectors.
7. Text ordering handles Unicode normalization and natural numbers: `Volume 2` precedes `Volume 10`. Provide a documented locale/collation choice for linguistic ordering; normalization alone does not provide Japanese reading order.
8. Sort labels remain unchanged. Use an explicit server sort name where supplied, otherwise the display label. Offer configurable leading articles, initially `The`, scoped to artist names. Do not infer surnames or romanize Japanese names. Future overrides belong to individual artist identities, never every member of a combined credit. [MusicBee's developer on multi-artist ambiguity](https://getmusicbee.com/forum/index.php?topic=21624.0)
9. Add stable IDs after meaningful tie breakers. Apply the same ordering before pagination across the whole filtered result. Never sort only the currently loaded 500 tracks. A refresh that changes ordered values starts a fresh result generation so old and new pages cannot mix.
10. Alphabet jumps and group headings follow effective sort keys. Hide alphabet navigation for date/rating orders; do not jump by raw title while sorting by artist or an ignored article.
11. A sort change preserves filters, selection by ID, and current playback. Preserve the visible anchor when possible. Existing Play Album and Play Now retain their documented album/disc/track behavior.

## Metadata and implementation

OpenSubsonic's album response defines `created`, and optional `played`, `sortName`, `isCompilation`, `userRating`, `playCount`, and release dates. Sonora already retains some of these, but discards others during deserialization. Server availability still needs verification; an optional protocol field is not a guarantee that every Navidrome version populates it. [Album response contract](https://opensubsonic.netlify.app/docs/responses/albumid3/)

Use server album rating/play count for the corresponding album presets. Do not silently substitute average track rating or summed track plays. Such aggregates can be separate, explicitly named fields later. Statistics and dates remain properties of the complete album even when a search matches only one track. Use a persisted first-seen timestamp only as an explicitly documented fallback when server date added is unavailable; do not label a fresh initial sync as historical date added.

Suggested boundaries:

- Model: typed `SortSpec`, `SortRule`, field/direction enums, grouping, and serializable saved presets with backward-compatible defaults.
- UI: preset menu, custom sheet, rule summary, context-specific choices, and jump/anchor behavior.
- Catalog: one validated sort compiler for each scope; precomputed sortable text keys, scalar metadata columns, indexes for common presets, and additive migrations/backfill. Pick a Unicode collation implementation after checking its Rust support and indexing needs. Keep these keys separate from search keys.
- Server: preserve documented optional metadata without inventing absent values or adding per-row network requests.
- Service: load/save sorting preferences through existing account-local state, invalidate queries when any active rule's inputs change, and discard stale paginated responses. The present special case for `Sort::Rating` must become a dependency check across all rules.
- Playback/platform: maintain existing responsibilities. Sorting does not mutate the playing queue.

Do not reset caches to migrate sorting. Existing records remain available while new keys/metadata are populated, and incomplete syncs still preserve cached records. A sorting-key version allows rebuilding derived keys when collation preferences change.

## Delivery sequence and acceptance

1. **Correctness and useful presets:** introduce the scope-aware model, consistent artist/album fields, per-rule direction, selected preset indicator, persistence, chronological/title/artist presets, stable pagination, natural Unicode-aware comparison, and matching jump behavior. Keep unsupported metadata choices hidden or clearly unavailable.
2. **Metadata-backed browsing:** preserve/index dates and server sort names, add recently added/rated/played presets, define null and partial-date behavior, and update mutation invalidation. Existing cached history cannot be reconstructed if earlier versions discarded it.
3. **Personalization:** custom rule sheet, saved presets, grouping controls, article/collation preferences, and explicit identity-based overrides. Random discovery can follow separately with a stable seed and a deliberate reshuffle action.

Each increment should ship usable behavior. Verify mixed ascending/descending rules, collaborations and Various Artists, duplicate album titles and editions, multi-disc track order, Unicode/full-width text, natural numbers, missing/partial dates, stable ties, and pagination beyond 500 matching tracks. Test persistence, view changes, rating/play-count updates, scroll/selection preservation, unchanged playback actions, and interrupted sync preservation. Check common sort query plans and responsiveness on a large catalog. Run the repository's required fmt, Clippy, and test checks when implementation changes begin.
