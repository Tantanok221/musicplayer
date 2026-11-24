# Saved-state compatibility fixtures

These are frozen examples of Sonora's JSON stored in SQLite `state.data`
(`preferences`, `queue`, `lastfm-history:v1:listener`) and `playlists.data`.
The `*-empty` and `preferences-default` snapshots record today's defaults;
the populated examples exercise non-default values and Unicode. Preferences
and playlist entries intentionally omit some defaulted fields to also cover
older, sparse records. History here is listening history saved by Last.fm for the removed Discover page; `sonora::history::History` still reads it, not the
in-memory Back/Forward stack.

Do not regenerate these to make a compatibility failure disappear. Preserve
old fixtures and add new ones when intentionally migrating the saved format.
