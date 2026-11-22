//! Opt-in tag editing: the edit model, mapping Navidrome paths to a local
//! folder, and writing tags to files with lofty.
use crate::model::{Track, TrackId};
use anyhow::{Context, Result, bail};
use lofty::{
    config::WriteOptions,
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
        }
    }

    /// Fields that hold several values, entered separated by `;`.
    pub fn multi(self) -> bool {
        matches!(
            self,
            TagField::Artist | TagField::AlbumArtist | TagField::Genre | TagField::Composer
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

/// The editor's form: each field's shared value, or `None` when the
/// selected tracks differ.
#[derive(Clone, Debug)]
pub struct EditorForm {
    pub tracks: Vec<Track>,
    pub fields: Vec<(TagField, Option<String>)>,
}

pub const MULTIPLE_VALUES: &str = "<multiple values>";

impl EditorForm {
    pub fn new(tracks: Vec<Track>) -> Self {
        let fields = TagField::ALL
            .iter()
            .map(|&field| {
                let mut values = tracks.iter().map(|track| field.shown(track));
                let first = values.next().unwrap_or_default();
                let shared = values.all(|value| value == first).then_some(first);
                (field, shared)
            })
            .collect();
        Self { tracks, fields }
    }

    pub fn shown(&self, field: TagField) -> Option<&str> {
        self.fields
            .iter()
            .find(|(f, _)| *f == field)
            .and_then(|(_, value)| value.as_deref())
    }

    /// The edit for what the user typed: a field counts as changed only
    /// when its text differs from what the form showed.
    pub fn edit(&self, entered: &[(TagField, String)]) -> TagEdit {
        let fields = entered
            .iter()
            .filter(|(field, text)| match self.shown(*field) {
                Some(shown) => shown.trim() != text.trim(),
                None => text.trim() != MULTIPLE_VALUES && !text.trim().is_empty(),
            })
            .map(|(field, text)| (*field, EditValue::parse(*field, text)))
            .collect();
        TagEdit {
            tracks: self.tracks.iter().map(|track| track.id.clone()).collect(),
            fields,
        }
    }

    /// Edits that can give tracks new Navidrome IDs after the rescan.
    pub fn warns_ids(&self, edit: &TagEdit) -> bool {
        edit.fields.keys().any(|field| field.changes_id())
            && self
                .tracks
                .iter()
                .any(|track| track.music_brainz_id.is_empty())
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
    Ok(TagField::ALL
        .iter()
        .map(|&field| {
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

    #[test]
    fn form_shows_shared_values_and_multiple_values() {
        let form = EditorForm::new(vec![track("a", "Same", "One"), track("b", "Same", "Two")]);
        assert_eq!(form.shown(TagField::Album), Some("Same"));
        assert_eq!(form.shown(TagField::Artist), None);
        assert_eq!(form.shown(TagField::Title), None);
    }

    #[test]
    fn edit_keeps_untouched_and_multiple_value_fields() {
        let form = EditorForm::new(vec![track("a", "Same", "One"), track("b", "Same", "Two")]);
        let edit = form.edit(&[
            (TagField::Album, "Same".into()),
            (TagField::Artist, MULTIPLE_VALUES.into()),
            (TagField::Genre, "Rock; Pop".into()),
            (TagField::Year, " 2020 ".into()),
        ]);
        assert_eq!(edit.tracks, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            edit.fields.into_iter().collect::<Vec<_>>(),
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
    fn clearing_a_shared_field_clears_it() {
        let form = EditorForm::new(vec![Track {
            comment: "old".into(),
            ..track("a", "A", "One")
        }]);
        let edit = form.edit(&[(TagField::Comment, "".into())]);
        assert_eq!(edit.fields.get(&TagField::Comment), Some(&EditValue::Clear));
        assert!(!form.warns_ids(&edit));
        assert!(form.warns_ids(&form.edit(&[(TagField::Album, "B".into())])));
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
