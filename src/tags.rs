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
/// The original start of a file a write changes in place: its length and
/// tag area, to put back if the write is cut off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeadBackup {
    pub length: u64,
    pub head: Vec<u8>,
}

/// Writes `fields` into the file, returning its tags before and after.
///
/// When the new tags fit in the file's tag area, only that area is rewritten
/// in place: on a network share that is a few kilobytes instead of the whole
/// file twice. `keep` gets the original area first, so a write that is cut off
/// can be put back with `restore_head`. Otherwise the file is replaced by a
/// finished copy, as `write` does.
pub fn write_keeping(
    path: &Path,
    fields: &BTreeMap<TagField, EditValue>,
    keep: &mut dyn FnMut(&HeadBackup) -> Result<()>,
) -> Result<(TagValues, TagValues)> {
    match write_in_place(path, fields, keep)? {
        Some(done) => Ok(done),
        None => write_copy(path, fields),
    }
}

/// `write_keeping` without storing the original tag area anywhere.
pub fn write(
    path: &Path,
    fields: &BTreeMap<TagField, EditValue>,
) -> Result<(TagValues, TagValues)> {
    write_keeping(path, fields, &mut |_| Ok(()))
}

/// Puts back the start of a file an in-place write was cut off in. Refuses a
/// file whose size changed since, as it is no longer the one backed up.
pub fn restore_head(path: &Path, backup: &HeadBackup) -> Result<()> {
    use std::io::{Seek, Write};
    let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
    let length = file.metadata()?.len();
    if length != backup.length {
        bail!(
            "{} changed size since it was backed up ({length} bytes, was {})",
            path.display(),
            backup.length
        );
    }
    file.seek(std::io::SeekFrom::Start(0))?;
    file.write_all(&backup.head)?;
    flush(&file)?;
    Ok(())
}

/// Audio read along with a tag area, so the tag library can parse the stream.
const AUDIO_SAMPLE: u64 = 64 * 1024;
const FLAC_PADDING: u8 = 1;

/// Rewrites a FLAC file's metadata blocks where they are, when the new ones
/// fit. Returns `None` when the file has to be replaced instead.
fn write_in_place(
    path: &Path,
    fields: &BTreeMap<TagField, EditValue>,
    keep: &mut dyn FnMut(&HeadBackup) -> Result<()>,
) -> Result<Option<(TagValues, TagValues)>> {
    use std::io::{Read, Seek, Write};
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Couldn't open {}", path.display()))?;
    let length = file.metadata()?.len();
    let mut magic = [0; 4];
    if file.read_exact(&mut magic).is_err() || &magic != b"fLaC" {
        return Ok(None);
    }
    // The tag area is at most a few megabytes; read it and a little audio.
    let mut head = magic.to_vec();
    let audio = loop {
        let mut header = [0; 4];
        file.read_exact(&mut header)?;
        let size = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        head.extend_from_slice(&header);
        let start = head.len();
        head.resize(start + size, 0);
        file.read_exact(&mut head[start..])?;
        if header[0] & 0x80 != 0 {
            break head.len();
        }
    };
    let mut sample = head;
    let take = AUDIO_SAMPLE.min(length.saturating_sub(audio as u64));
    (&mut file).take(take).read_to_end(&mut sample)?;
    drop(file);

    let name = path
        .file_name()
        .context("Not a file")?
        .to_string_lossy()
        .into_owned();
    let local_dir = tempfile::Builder::new().prefix("sonora-tags").tempdir()?;
    let local = local_dir.path().join(&name);
    std::fs::write(&local, &sample)?;
    let before = read(&local)?;
    set_tags(&local, &name, fields)?;
    let edited = std::fs::read(&local)?;
    let Some((blocks, edited_audio)) = flac_blocks(&edited) else {
        return Ok(None);
    };
    if edited[edited_audio..] != sample[audio..] {
        return Ok(None);
    }
    let Some(new_head) = pack_flac_head(&edited, &blocks, audio) else {
        return Ok(None);
    };
    // Check the packed area reads back as the edit before touching the file.
    let mut packed = new_head.clone();
    packed.extend_from_slice(&sample[audio..]);
    std::fs::write(&local, &packed)?;
    let after = read(&local)?;
    if after != read_edited(&edited, local_dir.path(), &name)? {
        return Ok(None);
    }
    if new_head == sample[..audio] {
        return Ok(Some((before, after)));
    }
    keep(&HeadBackup {
        length,
        head: sample[..audio].to_vec(),
    })?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("Couldn't open {} for writing", path.display()))?;
    file.seek(std::io::SeekFrom::Start(0))?;
    file.write_all(&new_head)
        .with_context(|| format!("Couldn't write tags to {name}"))?;
    flush(&file).with_context(|| format!("Couldn't save {name} to disk"))?;
    Ok(Some((before, after)))
}

/// The tags lofty wrote into `edited`, read from a file of their own.
fn read_edited(edited: &[u8], dir: &Path, name: &str) -> Result<TagValues> {
    let check = dir.join(format!("edited.{name}"));
    std::fs::write(&check, edited)?;
    read(&check)
}

/// A FLAC metadata block: its type and where its content is.
type FlacBlock = (u8, std::ops::Range<usize>);

/// A FLAC file's metadata blocks, as type and content range, and the offset
/// its audio starts at.
fn flac_blocks(bytes: &[u8]) -> Option<(Vec<FlacBlock>, usize)> {
    if bytes.get(..4)? != b"fLaC" {
        return None;
    }
    let mut at = 4;
    let mut blocks = vec![];
    loop {
        let header = bytes.get(at..at + 4)?;
        let size = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        let content = at + 4..at + 4 + size;
        if content.end > bytes.len() {
            return None;
        }
        blocks.push((header[0] & 0x7f, content.clone()));
        at = content.end;
        if header[0] & 0x80 != 0 {
            return Some((blocks, at));
        }
    }
}

/// Lays the non-padding `blocks` of `bytes` out in exactly `size` bytes,
/// filling what is left with one padding block, or `None` if they don't fit.
fn pack_flac_head(bytes: &[u8], blocks: &[FlacBlock], size: usize) -> Option<Vec<u8>> {
    let kept: Vec<_> = blocks
        .iter()
        .filter(|(kind, _)| *kind != FLAC_PADDING)
        .collect();
    let used = 4 + kept
        .iter()
        .map(|(_, content)| 4 + content.len())
        .sum::<usize>();
    let spare = size.checked_sub(used)?;
    // A padding block needs its 4-byte header.
    if spare != 0 && spare < 4 || spare.saturating_sub(4) > 0xFF_FFFF {
        return None;
    }
    let mut head = b"fLaC".to_vec();
    for (index, (kind, content)) in kept.iter().enumerate() {
        let last = spare == 0 && index + 1 == kept.len();
        head.push(kind | if last { 0x80 } else { 0 });
        head.extend_from_slice(&(content.len() as u32).to_be_bytes()[1..]);
        head.extend_from_slice(&bytes[content.clone()]);
    }
    if spare != 0 {
        head.push(FLAC_PADDING | 0x80);
        head.extend_from_slice(&((spare - 4) as u32).to_be_bytes()[1..]);
        head.resize(size, 0);
    }
    Some(head)
}

/// Replaces the file with an edited copy, renamed over it once complete.
fn write_copy(
    path: &Path,
    fields: &BTreeMap<TagField, EditValue>,
) -> Result<(TagValues, TagValues)> {
    let name = path
        .file_name()
        .context("Not a file")?
        .to_string_lossy()
        .into_owned();
    // Edit a copy on this computer: on a network share the file then crosses
    // the network once each way, and the tag library never works over it.
    let local_dir = tempfile::Builder::new().prefix("sonora-tags").tempdir()?;
    let local = local_dir.path().join(&name);
    std::fs::copy(path, &local).with_context(|| format!("Couldn't copy {}", path.display()))?;
    let before = read(&local)?;
    set_tags(&local, &name, fields)?;
    let after = read(&local)?;
    let staged = staged_path(path);
    let result = (|| -> Result<()> {
        std::fs::copy(&local, &staged)
            .with_context(|| format!("Couldn't copy {name} back to the music folder"))?;
        flush(&std::fs::File::open(&staged)?)
            .with_context(|| format!("Couldn't save {name} to disk"))?;
        std::fs::rename(&staged, path)
            .with_context(|| format!("Couldn't replace {}", path.display()))?;
        if let Some(dir) = path.parent()
            && let Ok(dir) = std::fs::File::open(dir)
        {
            let _ = flush(&dir);
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    Ok((before, after))
}

/// Applies `fields` to the tags of the file at `path`, in place.
fn set_tags(path: &Path, name: &str, fields: &BTreeMap<TagField, EditValue>) -> Result<()> {
    let mut file = lofty::probe::Probe::open(path)?.guess_file_type()?.read()?;
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
    file.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("Couldn't write tags to {name}"))
}

/// Where a write stages the new file beside `path` before renaming it over.
pub fn staged_path(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!(".sonora-tags.{name}"))
}

/// Removes the staged copy a write that was cut off left beside `path`. The
/// file itself is untouched: it is only ever replaced by a finished copy.
pub fn discard_staged(path: &Path) {
    let _ = std::fs::remove_file(staged_path(path));
}

/// Flushes `file` to stable storage.
fn flush(file: &std::fs::File) -> std::io::Result<()> {
    flush_with(file, std::fs::File::sync_all)
}

/// macOS flushes with F_FULLFSYNC, which network shares such as SMB refuse;
/// a plain fsync still makes the server commit the data.
fn flush_with(
    file: &std::fs::File,
    full: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match full(file) {
        Err(error) if unsupported(&error) => plain_fsync(file),
        result => result,
    }
}

fn unsupported(error: &std::io::Error) -> bool {
    // ENOTSUP and EOPNOTSUPP: 45 and 102 on macOS, 95 on Linux.
    const CODES: &[i32] = if cfg!(target_vendor = "apple") {
        &[45, 102]
    } else {
        &[95]
    };
    error.kind() == std::io::ErrorKind::Unsupported
        || error
            .raw_os_error()
            .is_some_and(|code| CODES.contains(&code))
}

#[cfg(unix)]
fn plain_fsync(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn fsync(fd: std::ffi::c_int) -> std::ffi::c_int;
    }
    // SAFETY: the descriptor belongs to `file`, which stays open for the call.
    if unsafe { fsync(file.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn plain_fsync(file: &std::fs::File) -> std::io::Result<()> {
    file.sync_data()
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

/// How far a tag job has got: files finished out of the total, and the song
/// being written now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagProgress {
    pub done: usize,
    pub total: usize,
    pub current: String,
    /// Whether the job puts back an earlier edit.
    pub undo: bool,
}

impl TagProgress {
    /// "Writing 3 of 16 · Title", counting the file being written.
    pub fn label(&self) -> String {
        let verb = if self.undo { "Putting back" } else { "Writing" };
        let at = (self.done + 1).min(self.total.max(1));
        if self.current.is_empty() {
            format!("{verb} {at} of {}", self.total)
        } else {
            format!("{verb} {at} of {} · {}", self.total, self.current)
        }
    }
}

/// Tag jobs handed to the service, which runs them one at a time in order:
/// the edits (with whatever the view needs to reopen them if they fail), how
/// many undos are queued, and how far the running job has got.
#[derive(Debug)]
pub struct TagJobs<T> {
    writes: std::collections::VecDeque<T>,
    undos: usize,
    progress: Option<TagProgress>,
}

impl<T> Default for TagJobs<T> {
    fn default() -> Self {
        Self {
            writes: Default::default(),
            undos: 0,
            progress: None,
        }
    }
}

impl<T> TagJobs<T> {
    pub fn write(&mut self, edit: T) {
        self.writes.push_back(edit);
    }

    pub fn undo(&mut self) {
        self.undos += 1;
    }

    pub fn progress(&mut self, progress: TagProgress) {
        if self.busy() {
            self.progress = Some(progress);
        }
    }

    /// Takes the job `report` is for; a write's edit comes back.
    pub fn finished(&mut self, report: &TagReport) -> Option<T> {
        self.progress = None;
        if report.undo {
            self.undos = self.undos.saturating_sub(1);
            None
        } else {
            self.writes.pop_front()
        }
    }

    pub fn busy(&self) -> bool {
        !self.writes.is_empty() || self.undos > 0
    }

    /// What the progress bar says while any job is queued or running.
    pub fn status(&self) -> Option<String> {
        if !self.busy() {
            return None;
        }
        let waiting = self.writes.len() + self.undos;
        let mut text = match &self.progress {
            Some(progress) => progress.label(),
            None if self.writes.is_empty() => "Putting back tags…".to_string(),
            None => "Writing tags…".to_string(),
        };
        if waiting > 1 {
            text.push_str(&format!(" · {} more waiting", waiting - 1));
        }
        Some(text)
    }
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

    #[test]
    fn new_flac_tags_are_packed_into_the_old_area_or_refused() {
        // fLaC, a 2-byte block, a 3-byte padding block (last).
        let file = [
            b'f', b'L', b'a', b'C', 4, 0, 0, 2, b'a', b'b', 0x81, 0, 0, 3, 0, 0, 0,
        ];
        let (blocks, audio) = flac_blocks(&file).unwrap();
        assert_eq!(audio, file.len());
        assert_eq!(blocks, vec![(4, 8..10), (1, 14..17)]);
        // Same area: the padding is laid out again.
        assert_eq!(pack_flac_head(&file, &blocks, 17).unwrap(), file);
        // Exactly full: the tag block becomes the last one.
        assert_eq!(
            pack_flac_head(&file, &blocks, 10).unwrap(),
            [b'f', b'L', b'a', b'C', 0x84, 0, 0, 2, b'a', b'b']
        );
        // Too small, or a gap a padding header can't fill.
        assert!(pack_flac_head(&file, &blocks, 9).is_none());
        assert!(pack_flac_head(&file, &blocks, 12).is_none());
        assert!(flac_blocks(b"ID3...").is_none());
    }

    #[test]
    fn a_cut_off_in_place_write_is_put_back_unless_the_file_changed_size() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("01 Song.flac");
        std::fs::write(&file, b"fLaC-torn-audio").unwrap();
        let backup = HeadBackup {
            length: 15,
            head: b"fLaC-good".to_vec(),
        };
        restore_head(&file, &backup).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"fLaC-good-audio");
        std::fs::write(&file, b"replaced since").unwrap();
        assert!(restore_head(&file, &backup).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"replaced since");
    }

    #[test]
    fn queued_jobs_report_in_order_and_give_back_failed_edits() {
        let mut jobs = TagJobs::default();
        assert_eq!(jobs.status(), None);
        jobs.write("album");
        jobs.write("song");
        assert_eq!(
            jobs.status().as_deref(),
            Some("Writing tags… · 1 more waiting")
        );
        jobs.progress(TagProgress {
            done: 2,
            total: 16,
            current: "花鳥風月".into(),
            undo: false,
        });
        assert_eq!(
            jobs.status().as_deref(),
            Some("Writing 3 of 16 · 花鳥風月 · 1 more waiting")
        );
        let written = TagReport::default();
        assert_eq!(jobs.finished(&written), Some("album"));
        assert_eq!(jobs.status().as_deref(), Some("Writing tags…"));
        jobs.undo();
        assert_eq!(jobs.finished(&written), Some("song"));
        assert_eq!(jobs.status().as_deref(), Some("Putting back tags…"));
        jobs.progress(TagProgress {
            done: 0,
            total: 1,
            current: "Song".into(),
            undo: true,
        });
        assert_eq!(jobs.status().as_deref(), Some("Putting back 1 of 1 · Song"));
        let undone = TagReport {
            undo: true,
            ..Default::default()
        };
        assert_eq!(jobs.finished(&undone), None);
        assert!(!jobs.busy());
        // Progress for nothing the view queued is ignored.
        jobs.progress(TagProgress::default());
        assert_eq!(jobs.status(), None);
    }

    #[test]
    fn progress_counts_the_file_being_written() {
        let progress = |done, current: &str| TagProgress {
            done,
            total: 16,
            current: current.into(),
            undo: false,
        };
        assert_eq!(
            progress(0, "ベテルギウス").label(),
            "Writing 1 of 16 · ベテルギウス"
        );
        assert_eq!(progress(15, "").label(), "Writing 16 of 16");
        assert_eq!(progress(16, "").label(), "Writing 16 of 16");
    }

    #[test]
    fn flushing_falls_back_to_fsync_where_the_full_flush_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("song.flac")).unwrap();
        // What SMB answers to macOS's F_FULLFSYNC.
        let smb = |_: &std::fs::File| Err(std::io::Error::from_raw_os_error(45));
        if cfg!(target_vendor = "apple") {
            assert!(flush_with(&file, smb).is_ok());
        }
        let refused = |_: &std::fs::File| Err(std::io::Error::from_raw_os_error(95));
        if cfg!(target_os = "linux") {
            assert!(flush_with(&file, refused).is_ok());
        }
        let broken = |_: &std::fs::File| Err(std::io::Error::other("disk gone"));
        assert!(flush_with(&file, broken).is_err());
        assert!(flush(&file).is_ok());
    }

    #[test]
    fn a_write_replaces_the_file_through_a_staged_copy_and_leaves_none() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("01 Song.flac");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone-0.flac"),
            &file,
        )
        .unwrap();
        let fields = BTreeMap::from([(TagField::Title, EditValue::Set("New".into()))]);
        let (before, after) = write(&file, &fields).unwrap();
        assert!(before.get(&TagField::Title).is_none_or(Vec::is_empty));
        assert_eq!(after[&TagField::Title], vec!["New"]);
        assert_eq!(read(&file).unwrap()[&TagField::Title], vec!["New"]);
        assert!(!staged_path(&file).exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        std::fs::write(staged_path(&file), b"cut off").unwrap();
        discard_staged(&file);
        assert!(!staged_path(&file).exists());
        assert_eq!(read(&file).unwrap()[&TagField::Title], vec!["New"]);
    }

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
