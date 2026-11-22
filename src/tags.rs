//! Opt-in tag editing: the edit model, mapping Navidrome paths to a local
//! folder, and writing tags to files with lofty.
use crate::model::{Track, TrackId};
use anyhow::{Context, Result, bail};
use lofty::{
    config::WriteOptions,
    picture::{MimeType, Picture, PictureType},
    prelude::*,
    tag::{ItemValue, Tag, TagItem},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

/// A tag the editor can change. Ratings, favorites and play counts are
/// Navidrome annotations and never go into files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TagField {
    Title,
    Artist,
    AlbumArtist,
    Album,
    Year,
    Track,
    Disc,
    Genre,
    Composer,
    Comment,
    MusicBrainzTrack,
    MusicBrainzReleaseTrack,
    MusicBrainzRelease,
    MusicBrainzReleaseGroup,
    MusicBrainzArtist,
    MusicBrainzAlbumArtist,
    /// The embedded front cover, held as `mime;base64,data`.
    CoverArt,
}

/// A cover art value still to be fetched from Cover Art Archive for the
/// release ID that follows.
pub const COVER_PREFIX: &str = "coverart:";

/// A picture as a field value, so the journal can put it back.
pub fn picture_value(mime: &str, data: &[u8]) -> String {
    use base64::Engine;
    format!(
        "{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

fn picture_data(value: &str) -> Result<Picture> {
    use base64::Engine;
    if value.starts_with(COVER_PREFIX) {
        bail!("The cover art wasn't downloaded");
    }
    let (mime, data) = value.split_once(";base64,").context("Not a picture")?;
    let data = base64::engine::general_purpose::STANDARD.decode(data)?;
    Ok(Picture::unchecked(data)
        .pic_type(PictureType::CoverFront)
        .mime_type(MimeType::from_str(mime))
        .build())
}

/// Every release whose front cover an edit asks for.
pub fn wanted_covers(edits: &[TagEdit]) -> std::collections::BTreeSet<String> {
    edits
        .iter()
        .filter_map(|edit| match edit.fields.get(&TagField::CoverArt) {
            Some(EditValue::Set(value)) => value.strip_prefix(COVER_PREFIX).map(str::to_string),
            _ => None,
        })
        .collect()
}

/// Puts the downloaded covers in place of the release IDs.
pub fn with_covers(mut edits: Vec<TagEdit>, covers: &BTreeMap<String, String>) -> Vec<TagEdit> {
    for edit in &mut edits {
        if let Some(EditValue::Set(value)) = edit.fields.get_mut(&TagField::CoverArt)
            && let Some(cover) = value
                .strip_prefix(COVER_PREFIX)
                .and_then(|release| covers.get(release))
        {
            *value = cover.clone();
        }
    }
    edits
}

impl TagField {
    pub const ALL: [TagField; 10] = [
        TagField::Title,
        TagField::Artist,
        TagField::AlbumArtist,
        TagField::Album,
        TagField::Year,
        TagField::Track,
        TagField::Disc,
        TagField::Genre,
        TagField::Composer,
        TagField::Comment,
    ];

    /// MusicBrainz identifiers: written by a lookup, not shown in the form.
    pub const IDS: [TagField; 6] = [
        TagField::MusicBrainzTrack,
        TagField::MusicBrainzReleaseTrack,
        TagField::MusicBrainzRelease,
        TagField::MusicBrainzReleaseGroup,
        TagField::MusicBrainzArtist,
        TagField::MusicBrainzAlbumArtist,
    ];

    /// Every field Sonora reads, writes and puts back.
    pub fn every() -> impl Iterator<Item = TagField> {
        Self::ALL
            .into_iter()
            .chain(Self::IDS)
            .chain([TagField::CoverArt])
    }

    pub fn label(self) -> &'static str {
        match self {
            TagField::Title => "Title",
            TagField::Artist => "Artist",
            TagField::AlbumArtist => "Album artist",
            TagField::Album => "Album",
            TagField::Year => "Year",
            TagField::Track => "Track",
            TagField::Disc => "Disc",
            TagField::Genre => "Genre",
            TagField::Composer => "Composer",
            TagField::Comment => "Comment",
            TagField::MusicBrainzTrack => "MusicBrainz recording ID",
            TagField::MusicBrainzReleaseTrack => "MusicBrainz track ID",
            TagField::MusicBrainzRelease => "MusicBrainz release ID",
            TagField::MusicBrainzReleaseGroup => "MusicBrainz release group ID",
            TagField::MusicBrainzArtist => "MusicBrainz artist ID",
            TagField::MusicBrainzAlbumArtist => "MusicBrainz album artist ID",
            TagField::CoverArt => "Cover art",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            TagField::Title => "title",
            TagField::Artist => "artist",
            TagField::AlbumArtist => "album-artist",
            TagField::Album => "album",
            TagField::Year => "year",
            TagField::Track => "track",
            TagField::Disc => "disc",
            TagField::Genre => "genre",
            TagField::Composer => "composer",
            TagField::Comment => "comment",
            TagField::MusicBrainzTrack => "mb-recording",
            TagField::MusicBrainzReleaseTrack => "mb-track",
            TagField::MusicBrainzRelease => "mb-release",
            TagField::MusicBrainzReleaseGroup => "mb-release-group",
            TagField::MusicBrainzArtist => "mb-artist",
            TagField::MusicBrainzAlbumArtist => "mb-album-artist",
            TagField::CoverArt => "cover-art",
        }
    }

    /// Fields that hold several values, entered separated by `;`.
    pub fn multi(self) -> bool {
        matches!(
            self,
            TagField::Artist
                | TagField::AlbumArtist
                | TagField::Genre
                | TagField::Composer
                | TagField::MusicBrainzArtist
                | TagField::MusicBrainzAlbumArtist
        )
    }

    /// Fields that change the Navidrome ID of a file without MusicBrainz IDs.
    pub fn changes_id(self) -> bool {
        matches!(
            self,
            TagField::Title
                | TagField::Album
                | TagField::AlbumArtist
                | TagField::Track
                | TagField::Disc
                | TagField::MusicBrainzTrack
                | TagField::MusicBrainzRelease
                | TagField::MusicBrainzAlbumArtist
        )
    }

    fn keys(self) -> &'static [ItemKey] {
        match self {
            TagField::Title => &[ItemKey::TrackTitle],
            TagField::Artist => &[ItemKey::TrackArtist],
            TagField::AlbumArtist => &[ItemKey::AlbumArtist],
            TagField::Album => &[ItemKey::AlbumTitle],
            TagField::Year => &[ItemKey::RecordingDate, ItemKey::Year],
            TagField::Track => &[ItemKey::TrackNumber],
            TagField::Disc => &[ItemKey::DiscNumber],
            TagField::Genre => &[ItemKey::Genre],
            TagField::Composer => &[ItemKey::Composer],
            TagField::Comment => &[ItemKey::Comment],
            TagField::MusicBrainzTrack => &[ItemKey::MusicBrainzRecordingId],
            TagField::MusicBrainzReleaseTrack => &[ItemKey::MusicBrainzTrackId],
            TagField::MusicBrainzRelease => &[ItemKey::MusicBrainzReleaseId],
            TagField::MusicBrainzReleaseGroup => &[ItemKey::MusicBrainzReleaseGroupId],
            TagField::MusicBrainzArtist => &[ItemKey::MusicBrainzArtistId],
            TagField::MusicBrainzAlbumArtist => &[ItemKey::MusicBrainzReleaseArtistId],
            TagField::CoverArt => &[],
        }
    }

    /// The value Navidrome reports for this field, as the editor shows it.
    pub fn shown(self, track: &Track) -> String {
        let names = |named: &[crate::model::NamedId], fallback: &str| {
            if named.is_empty() {
                fallback.to_string()
            } else {
                named
                    .iter()
                    .map(|n| n.name.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            }
        };
        match self {
            TagField::Title => track.title.clone(),
            TagField::Artist => names(&track.artists, &track.artist),
            TagField::AlbumArtist => names(&track.album_artists, &track.album_artist),
            TagField::Album => track.album.clone(),
            TagField::Year => track.year.map(|y| y.to_string()).unwrap_or_default(),
            TagField::Track => track.track.map(|n| n.to_string()).unwrap_or_default(),
            TagField::Disc => track.disc_number.map(|n| n.to_string()).unwrap_or_default(),
            TagField::Genre => {
                if track.genres.is_empty() {
                    track.genre.clone()
                } else {
                    track
                        .genres
                        .iter()
                        .map(|g| g.name.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                }
            }
            TagField::Composer => track.display_composer.clone(),
            TagField::Comment => track.comment.clone(),
            TagField::MusicBrainzTrack => track.music_brainz_id.clone(),
            _ => String::new(),
        }
    }
}

/// What to do with one field of every selected file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditValue {
    Keep,
    Clear,
    Set(String),
    SetMany(Vec<String>),
}

impl EditValue {
    /// Parses what the user typed into a field.
    pub fn parse(field: TagField, text: &str) -> Self {
        let text = text.trim();
        if text.is_empty() {
            return EditValue::Clear;
        }
        if field.multi() {
            let values = split(text);
            if values.len() > 1 {
                return EditValue::SetMany(values);
            }
        }
        EditValue::Set(text.to_string())
    }

    fn values(&self) -> Option<Vec<String>> {
        match self {
            EditValue::Keep => None,
            EditValue::Clear => Some(vec![]),
            EditValue::Set(value) => Some(vec![value.clone()]),
            EditValue::SetMany(values) => Some(values.clone()),
        }
    }
}

fn split(text: &str) -> Vec<String> {
    text.split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

/// A batch edit: only the fields listed change; every other tag is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagEdit {
    pub tracks: Vec<TrackId>,
    pub fields: BTreeMap<TagField, EditValue>,
}

/// The tag values read from or written to one file.
pub type TagValues = BTreeMap<TagField, Vec<String>>;

/// The editor's form. With several songs, the per-track fields move into a
/// table and the rest are edited once for all of them.
#[derive(Clone, Debug)]
pub struct EditorForm {
    pub tracks: Vec<Track>,
    /// Each shared field's value, or `None` when the songs differ.
    pub fields: Vec<(TagField, Option<String>)>,
    /// The table's columns; empty for a single song.
    pub columns: Vec<TagField>,
}

pub const MULTIPLE_VALUES: &str = "<multiple values>";

/// Fields that usually differ between the songs of an album.
pub const PER_TRACK: [TagField; 4] = [
    TagField::Track,
    TagField::Disc,
    TagField::Title,
    TagField::Artist,
];

/// What the editor holds when the user saves.
#[derive(Clone, Debug, Default)]
pub struct Entries {
    pub shared: Vec<(TagField, String)>,
    /// Shared fields the user cleared on purpose: blank otherwise means keep.
    pub cleared: std::collections::BTreeSet<TagField>,
    /// Per song, the table's cells.
    pub cells: Vec<Vec<(TagField, String)>>,
    /// Per song, values a lookup filled in outside the visible inputs.
    pub extra: Vec<BTreeMap<TagField, String>>,
}

/// One value the save would change, for the review before writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub track: String,
    pub field: TagField,
    pub before: String,
    pub after: String,
}

impl EditorForm {
    pub fn new(tracks: Vec<Track>) -> Self {
        let columns = if tracks.len() > 1 {
            PER_TRACK.to_vec()
        } else {
            vec![]
        };
        let fields = TagField::ALL
            .iter()
            .filter(|field| !columns.contains(field))
            .map(|&field| {
                let mut values = tracks.iter().map(|track| field.shown(track));
                let first = values.next().unwrap_or_default();
                let shared = values.all(|value| value == first).then_some(first);
                (field, shared)
            })
            .collect();
        Self {
            tracks,
            fields,
            columns,
        }
    }

    pub fn shown(&self, field: TagField) -> Option<&str> {
        self.fields
            .iter()
            .find(|(f, _)| *f == field)
            .and_then(|(_, value)| value.as_deref())
    }

    /// Each song's changes: a value counts only when it differs from what
    /// the form showed, and a blank `<multiple values>` field is kept.
    fn per_track(&self, entries: &Entries) -> Vec<BTreeMap<TagField, EditValue>> {
        let shared: BTreeMap<_, _> = entries
            .shared
            .iter()
            .filter_map(|(field, text)| {
                let text = text.trim();
                let changed = match self.shown(*field) {
                    _ if text.is_empty() && entries.cleared.contains(field) => true,
                    Some(shown) => shown.trim() != text,
                    None => !text.is_empty() && text != MULTIPLE_VALUES,
                };
                changed.then(|| (*field, EditValue::parse(*field, text)))
            })
            .collect();
        self.tracks
            .iter()
            .enumerate()
            .map(|(row, track)| {
                let mut fields = shared.clone();
                let cells = entries.cells.get(row).into_iter().flatten();
                let extra = entries.extra.get(row).into_iter().flatten();
                let own = cells.map(|(field, text)| (field, text)).chain(extra);
                for (field, text) in own {
                    if field.shown(track).trim() != text.trim() {
                        fields.insert(*field, EditValue::parse(*field, text));
                    }
                }
                fields
            })
            .collect()
    }

    /// The edits to write, one per group of songs getting the same changes.
    pub fn edits(&self, entries: &Entries) -> Vec<TagEdit> {
        let mut edits: Vec<TagEdit> = vec![];
        for (track, fields) in self.tracks.iter().zip(self.per_track(entries)) {
            if fields.is_empty() {
                continue;
            }
            match edits.iter_mut().find(|edit| edit.fields == fields) {
                Some(edit) => edit.tracks.push(track.id.clone()),
                None => edits.push(TagEdit {
                    tracks: vec![track.id.clone()],
                    fields,
                }),
            }
        }
        edits
    }

    /// Every value the save would change, song by song.
    pub fn changes(&self, entries: &Entries) -> Vec<Change> {
        self.tracks
            .iter()
            .zip(self.per_track(entries))
            .flat_map(|(track, fields)| {
                fields.into_iter().map(move |(field, value)| Change {
                    track: track.title.clone(),
                    field,
                    before: field.shown(track),
                    after: if field == TagField::CoverArt {
                        match value.values().unwrap_or_default().first() {
                            Some(value) if value.starts_with(COVER_PREFIX) => {
                                "Front cover from Cover Art Archive".into()
                            }
                            Some(_) => "Embedded picture".into(),
                            None => String::new(),
                        }
                    } else {
                        value.values().unwrap_or_default().join("; ")
                    },
                })
            })
            .collect()
    }

    /// Edits that can give tracks new Navidrome IDs after the rescan.
    pub fn warns_ids(&self, edits: &[TagEdit]) -> bool {
        edits.iter().any(|edit| {
            edit.fields.keys().any(|field| field.changes_id())
                && self
                    .tracks
                    .iter()
                    .filter(|track| edit.tracks.contains(&track.id))
                    .any(|track| track.music_brainz_id.is_empty())
        })
    }
}

/// Finds the local file for a track under the mapped library folder.
///
/// Navidrome reports either a path relative to its library or, with Report
/// Real Path, the path inside its own filesystem. Leading components are
/// dropped until the rest exists under `root`, and the size must match.
pub fn resolve(root: &Path, track: &Track) -> Result<PathBuf, String> {
    if root.as_os_str().is_empty() {
        return Err("Choose your music folder in Settings → Metadata first.".into());
    }
    if !root.is_dir() {
        return Err(format!(
            "The music folder {} is not available.",
            root.display()
        ));
    }
    let parts: Vec<_> = Path::new(&track.path)
        .components()
        .filter_map(|part| match part {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();
    if parts.is_empty()
        || Path::new(&track.path)
            .components()
            .any(|c| c == Component::ParentDir)
    {
        return Err(format!(
            "Navidrome reported no file path for {}.",
            track.title
        ));
    }
    for start in 0..parts.len() {
        let candidate = parts[start..]
            .iter()
            .fold(root.to_path_buf(), |path, part| path.join(part));
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if let Some(size) = track.size
            && size != meta.len()
        {
            return Err(format!(
                "{} is a different file from the one Navidrome has ({} bytes here, {size} on the server).",
                candidate.display(),
                meta.len()
            ));
        }
        return Ok(candidate);
    }
    Err(format!(
        "Couldn't find {} in {}. Check the music folder, and turn on Report Real Path for Sonora in Navidrome's player settings.",
        track.path,
        root.display()
    ))
}

/// Reads the editable tags of a file.
pub fn read(path: &Path) -> Result<TagValues> {
    let file =
        lofty::read_from_path(path).with_context(|| format!("Couldn't read {}", path.display()))?;
    let tag = file.primary_tag().or_else(|| file.first_tag());
    Ok(TagField::every()
        .map(|field| {
            if field == TagField::CoverArt {
                let cover = tag.and_then(|tag| {
                    tag.get_picture_type(PictureType::CoverFront)
                        .or_else(|| tag.pictures().first())
                });
                let values = cover
                    .map(|picture| {
                        let mime = picture.mime_type().map_or("image/jpeg", MimeType::as_str);
                        vec![picture_value(mime, picture.data())]
                    })
                    .unwrap_or_default();
                return (field, values);
            }
            let values = tag
                .map(|tag| {
                    field
                        .keys()
                        .iter()
                        .map(|key| {
                            tag.get_strings(*key)
                                .map(str::to_string)
                                .collect::<Vec<_>>()
                        })
                        .find(|values| !values.is_empty())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            (field, values)
        })
        .collect())
}

/// Writes the changed fields into a copy beside the file, syncs it and
/// renames it over the original, so a failure leaves the file untouched.
/// Returns the values before and after.
pub fn write(
    path: &Path,
    fields: &BTreeMap<TagField, EditValue>,
) -> Result<(TagValues, TagValues)> {
    let before = read(path)?;
    let name = path.file_name().context("Not a file")?.to_string_lossy();
    let temp = path.with_file_name(format!(".sonora-tags.{name}"));
    let result = (|| -> Result<()> {
        std::fs::copy(path, &temp).with_context(|| format!("Couldn't copy {}", path.display()))?;
        let mut file = lofty::probe::Probe::open(&temp)?
            .guess_file_type()?
            .read()?;
        let kind = file.primary_tag_type();
        if file.primary_tag().is_none() {
            file.insert_tag(Tag::new(kind));
        }
        let tag = file
            .primary_tag_mut()
            .context("The file has no tag to edit")?;
        for (field, value) in fields {
            let Some(values) = value.values() else {
                continue;
            };
            if *field == TagField::CoverArt {
                // MP4 cover atoms carry no picture type and read back as Other.
                tag.remove_picture_type(PictureType::CoverFront);
                tag.remove_picture_type(PictureType::Other);
                for value in values {
                    tag.push_picture(picture_data(&value)?);
                }
                continue;
            }
            for key in field.keys() {
                tag.remove_key(*key);
            }
            for value in values {
                if !tag.push(TagItem::new(field.keys()[0], ItemValue::Text(value))) {
                    bail!("{} can't hold a {} tag", name, field.label());
                }
            }
        }
        file.save_to_path(&temp, WriteOptions::default())
            .with_context(|| format!("Couldn't write tags to {name}"))?;
        std::fs::File::open(&temp)?.sync_all()?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("Couldn't replace {}", path.display()))?;
        if let Some(dir) = path.parent()
            && let Ok(dir) = std::fs::File::open(dir)
        {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok((before, read(path)?))
}

/// The edit that puts back `before` for the fields an edit changed.
pub fn restore(before: &TagValues, changed: &TagValues) -> BTreeMap<TagField, EditValue> {
    changed
        .keys()
        .map(|field| {
            let values = before.get(field).cloned().unwrap_or_default();
            let value = match values.len() {
                0 => EditValue::Clear,
                1 => EditValue::Set(values[0].clone()),
                _ => EditValue::SetMany(values),
            };
            (*field, value)
        })
        .collect()
}

/// Where one file's edit stands in the journal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditState {
    #[default]
    Pending,
    Written,
    Confirmed,
    Failed,
    Undone,
}

impl EditState {
    pub fn as_str(self) -> &'static str {
        match self {
            EditState::Pending => "pending",
            EditState::Written => "written",
            EditState::Confirmed => "confirmed",
            EditState::Failed => "failed",
            EditState::Undone => "undone",
        }
    }
    pub fn parse(text: &str) -> Self {
        match text {
            "written" => EditState::Written,
            "confirmed" => EditState::Confirmed,
            "failed" => EditState::Failed,
            "undone" => EditState::Undone,
            _ => EditState::Pending,
        }
    }
}

/// One file's row in the tag edit journal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JournalEntry {
    pub id: i64,
    pub job: i64,
    pub track_id: TrackId,
    /// The path Navidrome reports, used to find the track after a rescan.
    pub path: String,
    /// The local file that was edited.
    pub file: String,
    pub title: String,
    pub before: TagValues,
    pub after: TagValues,
    pub starred: bool,
    pub rating: Option<u8>,
    pub state: EditState,
}

/// What became of a tag edit or undo, for the editor and the notice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagReport {
    pub job: i64,
    pub undo: bool,
    pub written: usize,
    /// Track title and reason for each file that was not changed.
    pub failures: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str, album: &str, artist: &str) -> Track {
        Track {
            id: id.into(),
            title: format!("Song {id}"),
            album: album.into(),
            artist: artist.into(),
            album_artist: artist.into(),
            ..Default::default()
        }
    }

    fn shared(fields: &[(TagField, &str)]) -> Entries {
        Entries {
            shared: fields.iter().map(|(f, t)| (*f, t.to_string())).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn form_moves_per_track_fields_into_the_table() {
        let form = EditorForm::new(vec![track("a", "Same", "One"), track("b", "Same", "Two")]);
        assert_eq!(form.shown(TagField::Album), Some("Same"));
        assert_eq!(form.shown(TagField::AlbumArtist), None);
        assert_eq!(form.columns, PER_TRACK.to_vec());
        assert!(form.fields.iter().all(|(f, _)| !PER_TRACK.contains(f)));
        let single = EditorForm::new(vec![track("a", "Same", "One")]);
        assert!(single.columns.is_empty());
        assert_eq!(single.fields.len(), TagField::ALL.len());
    }

    #[test]
    fn edit_keeps_untouched_and_multiple_value_fields() {
        let form = EditorForm::new(vec![track("a", "Same", "One"), track("b", "Same", "Two")]);
        let edits = form.edits(&shared(&[
            (TagField::Album, "Same"),
            (TagField::AlbumArtist, MULTIPLE_VALUES),
            (TagField::Genre, "Rock; Pop"),
            (TagField::Year, " 2020 "),
        ]));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].tracks, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            edits[0].clone().fields.into_iter().collect::<Vec<_>>(),
            vec![
                (TagField::Year, EditValue::Set("2020".into())),
                (
                    TagField::Genre,
                    EditValue::SetMany(vec!["Rock".into(), "Pop".into()])
                ),
            ]
        );
    }

    #[test]
    fn table_cells_give_each_song_its_own_edit() {
        let form = EditorForm::new(vec![track("a", "Same", "One"), track("b", "Same", "Two")]);
        let mut entries = shared(&[(TagField::Year, "2001")]);
        entries.cells = vec![
            vec![
                (TagField::Title, "Song a".into()),
                (TagField::Track, "1".into()),
            ],
            vec![
                (TagField::Title, "Song b".into()),
                (TagField::Track, "2".into()),
            ],
        ];
        let edits = form.edits(&entries);
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].tracks, vec!["a".to_string()]);
        assert_eq!(
            edits[1].fields.get(&TagField::Track),
            Some(&EditValue::Set("2".into()))
        );
        assert!(!edits[0].fields.contains_key(&TagField::Title));
        assert_eq!(
            form.changes(&entries)[1],
            Change {
                track: "Song a".into(),
                field: TagField::Track,
                before: "".into(),
                after: "1".into(),
            }
        );
    }

    #[test]
    fn a_blank_mixed_field_is_kept_unless_cleared() {
        let form = EditorForm::new(vec![
            Track {
                comment: "x".into(),
                ..track("a", "A", "One")
            },
            track("b", "A", "One"),
        ]);
        let mut entries = shared(&[(TagField::Comment, "")]);
        assert!(form.edits(&entries).is_empty());
        entries.cleared.insert(TagField::Comment);
        let edits = form.edits(&entries);
        assert_eq!(
            edits[0].fields.get(&TagField::Comment),
            Some(&EditValue::Clear)
        );
        assert_eq!(edits[0].tracks.len(), 2);
    }

    #[test]
    fn clearing_a_shared_field_clears_it() {
        let form = EditorForm::new(vec![Track {
            comment: "old".into(),
            ..track("a", "A", "One")
        }]);
        let edits = form.edits(&shared(&[(TagField::Comment, "")]));
        assert_eq!(
            edits[0].fields.get(&TagField::Comment),
            Some(&EditValue::Clear)
        );
        assert!(!form.warns_ids(&edits));
        assert!(form.warns_ids(&form.edits(&shared(&[(TagField::Album, "B")]))));
    }

    #[test]
    fn lookup_ids_are_written_but_unchanged_ones_skipped() {
        let form = EditorForm::new(vec![Track {
            music_brainz_id: "rec".into(),
            ..track("a", "A", "One")
        }]);
        let entries = Entries {
            extra: vec![BTreeMap::from([
                (TagField::MusicBrainzTrack, "rec".into()),
                (TagField::MusicBrainzRelease, "rel".into()),
            ])],
            ..Default::default()
        };
        let edits = form.edits(&entries);
        assert_eq!(
            edits[0].fields.keys().copied().collect::<Vec<_>>(),
            vec![TagField::MusicBrainzRelease]
        );
    }

    #[test]
    fn resolve_strips_leading_components_and_checks_size() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Artist/Album")).unwrap();
        std::fs::write(dir.path().join("Artist/Album/01.flac"), b"12345").unwrap();
        let mut song = Track {
            path: "/music/Artist/Album/01.flac".into(),
            size: Some(5),
            ..Default::default()
        };
        assert_eq!(
            resolve(dir.path(), &song).unwrap(),
            dir.path().join("Artist/Album/01.flac")
        );
        song.path = "Artist/Album/01.flac".into();
        assert!(resolve(dir.path(), &song).is_ok());
        song.size = Some(9);
        assert!(
            resolve(dir.path(), &song)
                .unwrap_err()
                .contains("different file")
        );
        song.path = "Other/Album/02 - Fake.flac".into();
        assert!(
            resolve(dir.path(), &song)
                .unwrap_err()
                .contains("Report Real Path")
        );
        song.path = "../etc/passwd".into();
        assert!(resolve(dir.path(), &song).is_err());
        assert!(
            resolve(Path::new(""), &song)
                .unwrap_err()
                .contains("Settings")
        );
    }

    fn fixture(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
            &path,
        )
        .unwrap();
        (dir, path)
    }

    #[test]
    fn writes_scalar_and_multi_value_tags_by_format() {
        for name in ["tone-0.flac", "tone-0.mp3", "tone-aac.m4a"] {
            let (_dir, path) = fixture(name);
            let fields = BTreeMap::from([
                (TagField::Title, EditValue::Set("New title".into())),
                (
                    TagField::Artist,
                    EditValue::SetMany(vec!["One".into(), "Two".into()]),
                ),
                (TagField::Track, EditValue::Set("7".into())),
                (TagField::Year, EditValue::Set("2021".into())),
            ]);
            let (_, after) = write(&path, &fields).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(after[&TagField::Title], vec!["New title"], "{name}");
            assert_eq!(after[&TagField::Artist], vec!["One", "Two"], "{name}");
            assert_eq!(after[&TagField::Track], vec!["7"], "{name}");
            assert_eq!(after[&TagField::Year], vec!["2021"], "{name}");
            let (before, _) = write(
                &path,
                &BTreeMap::from([(TagField::Comment, EditValue::Set("c".into()))]),
            )
            .unwrap();
            assert_eq!(
                read(&path).unwrap()[&TagField::Title],
                vec!["New title"],
                "{name} kept"
            );
            let undo = restore(&before, &BTreeMap::from([(TagField::Title, vec![])]));
            assert_eq!(undo[&TagField::Title], EditValue::Set("New title".into()));
            assert!(!path.with_file_name(format!(".sonora-tags.{name}")).exists());
        }
    }

    #[test]
    fn writes_and_puts_back_the_front_cover() {
        for name in ["tone-0.flac", "tone-0.mp3", "tone-aac.m4a"] {
            let (_dir, path) = fixture(name);
            let cover = picture_value("image/png", b"\x89PNG fixture");
            let fields = BTreeMap::from([(TagField::CoverArt, EditValue::Set(cover.clone()))]);
            let (before, after) = write(&path, &fields).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(after[&TagField::CoverArt], vec![cover], "{name}");
            let undo = restore(&before, &BTreeMap::from([(TagField::CoverArt, vec![])]));
            write(&path, &undo).unwrap();
            assert_eq!(
                read(&path).unwrap()[&TagField::CoverArt],
                before[&TagField::CoverArt]
            );
            let unfetched = BTreeMap::from([(
                TagField::CoverArt,
                EditValue::Set(format!("{COVER_PREFIX}rel")),
            )]);
            assert!(write(&path, &unfetched).is_err(), "{name}");
        }
    }

    #[test]
    fn covers_replace_the_release_ids_they_were_fetched_for() {
        let edit = |value: &str| TagEdit {
            tracks: vec!["t".into()],
            fields: BTreeMap::from([(TagField::CoverArt, EditValue::Set(value.into()))]),
        };
        let edits = vec![edit("coverart:a"), edit("image/png;base64,AA==")];
        assert_eq!(wanted_covers(&edits), ["a".to_string()].into());
        let covers = BTreeMap::from([("a".to_string(), "image/jpeg;base64,AQ==".to_string())]);
        let resolved = with_covers(edits, &covers);
        assert_eq!(resolved[0], edit("image/jpeg;base64,AQ=="));
        assert_eq!(resolved[1], edit("image/png;base64,AA=="));
    }

    #[test]
    fn failed_write_leaves_the_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.flac");
        std::fs::write(&path, b"not audio").unwrap();
        assert!(
            write(
                &path,
                &BTreeMap::from([(TagField::Title, EditValue::Set("x".into()))])
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"not audio");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
