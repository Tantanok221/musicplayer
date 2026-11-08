//! Explorer ordering. Display labels, album identity, and playback order stay independent.
use crate::model::{Album, Sort, Track, normalize};
use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    #[default]
    Albums,
    Tracks,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Field {
    AlbumArtist,
    Album,
    Artist,
    Title,
    OriginalDate,
    ReleaseDate,
    Added,
    Rating,
    Plays,
    Duration,
    Format,
    ReleaseType,
}
impl Field {
    pub fn choices(scope: Scope) -> &'static [Self] {
        use Field::*;
        match scope {
            Scope::Albums => &[
                AlbumArtist,
                Album,
                OriginalDate,
                ReleaseDate,
                Added,
                Rating,
                Plays,
                Duration,
                ReleaseType,
            ],
            Scope::Tracks => &[
                Title,
                Artist,
                AlbumArtist,
                Album,
                OriginalDate,
                ReleaseDate,
                Added,
                Rating,
                Plays,
                Duration,
                Format,
            ],
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::AlbumArtist => "Album artist",
            Self::Album => "Album title",
            Self::Artist => "Track artist",
            Self::Title => "Track title",
            Self::OriginalDate => "Original release date",
            Self::ReleaseDate => "Edition release date",
            Self::Added => "Date added",
            Self::Rating => "Rating",
            Self::Plays => "Play count",
            Self::Duration => "Duration",
            Self::Format => "Format",
            Self::ReleaseType => "Release type",
        }
    }
    pub fn direction_label(self, descending: bool) -> &'static str {
        match (self, descending) {
            (Self::OriginalDate | Self::ReleaseDate | Self::Added, false) => "Oldest first",
            (Self::OriginalDate | Self::ReleaseDate | Self::Added, true) => "Newest first",
            (Self::Rating | Self::Plays, false) => "Lowest first",
            (Self::Rating | Self::Plays, true) => "Highest first",
            (Self::Duration, false) => "Shortest first",
            (Self::Duration, true) => "Longest first",
            (_, false) => "A–Z",
            (_, true) => "Z–A",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub field: Field,
    pub descending: bool,
}
impl Rule {
    pub fn new(field: Field, descending: bool) -> Self {
        Self { field, descending }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grouping {
    #[default]
    None,
    AlbumArtist,
    Year,
    ReleaseType,
}
impl Grouping {
    pub const ALL: [Self; 4] = [Self::None, Self::AlbumArtist, Self::Year, Self::ReleaseType];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::AlbumArtist => "Album artist",
            Self::Year => "Release year",
            Self::ReleaseType => "Release type",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SortSpec {
    pub scope: Scope,
    pub rules: Vec<Rule>,
    pub grouping: Grouping,
    pub group_descending: bool,
    pub ignore_the: bool,
}
impl Default for SortSpec {
    fn default() -> Self {
        Preset::ArtistAlbum.spec(Scope::Albums)
    }
}
impl SortSpec {
    pub fn validated(&self, scope: Scope) -> Self {
        let mut result = self.clone();
        result.scope = scope;
        result
            .rules
            .retain(|rule| Field::choices(scope).contains(&rule.field));
        let mut fields = Vec::new();
        result.rules.retain(|rule| {
            if fields.contains(&rule.field) {
                false
            } else {
                fields.push(rule.field);
                true
            }
        });
        result.rules.truncate(8);
        if result.rules.is_empty() {
            result.rules.push(Rule::new(
                if scope == Scope::Albums {
                    Field::Album
                } else {
                    Field::Title
                },
                false,
            ));
        }
        if scope == Scope::Tracks {
            result.grouping = Grouping::None;
        }
        result
    }
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.grouping != Grouping::None {
            parts.push(format!(
                "Group: {} {}",
                self.grouping.label(),
                if self.group_descending { "↓" } else { "↑" }
            ));
        }
        parts.extend(self.rules.iter().map(|r| {
            format!(
                "{} · {}",
                r.field.label(),
                r.field.direction_label(r.descending)
            )
        }));
        parts.join(" → ")
    }
    pub fn label(&self) -> &'static str {
        self.preset().map_or("Album title", Preset::label)
    }
    pub fn preset(&self) -> Option<Preset> {
        Preset::choices(self.scope).iter().copied().find(|preset| {
            let mut base = preset.spec(self.scope);
            if let Some(rule) = self.rules.first() {
                base.rules[0].descending = rule.descending;
                if base.grouping == Grouping::AlbumArtist {
                    base.group_descending = rule.descending;
                }
            }
            if *preset == Preset::Discography
                && let Some(rule) = self.rules.get(1)
            {
                base.rules[1].descending = rule.descending;
            }
            base == *self
        })
    }
    pub fn alphabetical_field(&self) -> Option<Field> {
        if self.grouping == Grouping::AlbumArtist {
            return Some(Field::AlbumArtist);
        }
        if self.grouping != Grouping::None {
            return None;
        }
        self.rules
            .first()
            .and_then(|r| matches!(r.field, Field::AlbumArtist | Field::Album).then_some(r.field))
    }
    pub fn album_letter(&self, album: &Album) -> char {
        let value = match self.alphabetical_field() {
            Some(Field::AlbumArtist) => artist_text(&album.artist, self.ignore_the),
            _ => normalize(if album.sort_name.is_empty() {
                &album.name
            } else {
                &album.sort_name
            }),
        };
        value
            .chars()
            .next()
            .filter(char::is_ascii_alphabetic)
            .map_or('#', |c| c.to_ascii_uppercase())
    }
    pub fn group_label(&self, album: &Album) -> String {
        match self.grouping {
            Grouping::None => String::new(),
            Grouping::AlbumArtist => {
                if album.artist.is_empty() {
                    "Unknown artist".into()
                } else {
                    album.artist.clone()
                }
            }
            Grouping::Year => date_parts(
                album.original_release_date.as_ref(),
                album.release_date.as_ref(),
                album.year,
            )[0]
            .map_or("Unknown year".into(), |y| y.to_string()),
            Grouping::ReleaseType => release_label(release_rank(&album.release_types)).into(),
        }
    }
    pub fn depends_on(&self, field: Field) -> bool {
        self.rules.iter().any(|r| r.field == field)
    }
    pub fn legacy(sort: Sort, descending: bool, scope: Scope) -> Self {
        let field = match sort {
            Sort::Album => Field::Album,
            Sort::Artist => Field::AlbumArtist,
            Sort::Title => {
                if scope == Scope::Tracks {
                    Field::Title
                } else {
                    Field::Album
                }
            }
            Sort::Year => Field::OriginalDate,
            Sort::Added => Field::Added,
            Sort::Duration => Field::Duration,
            Sort::Rating => Field::Rating,
            Sort::Plays => Field::Plays,
            Sort::Format => Field::Format,
        };
        Self {
            scope,
            rules: vec![Rule::new(field, descending)],
            grouping: Grouping::None,
            group_descending: false,
            ignore_the: true,
        }
        .validated(scope)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Discography,
    ArtistAlbum,
    Album,
    ReleaseDate,
    Added,
    Rating,
    Plays,
    Title,
    ArtistTitle,
    Duration,
}
impl Preset {
    pub fn choices(scope: Scope) -> &'static [Self] {
        use Preset::*;
        match scope {
            Scope::Albums => &[
                Discography,
                ArtistAlbum,
                Album,
                ReleaseDate,
                Added,
                Rating,
                Plays,
            ],
            Scope::Tracks => &[
                Title,
                ArtistTitle,
                ArtistAlbum,
                Added,
                Rating,
                Plays,
                Duration,
            ],
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Discography => "Artist discography",
            Self::ArtistAlbum => "Artist / Album",
            Self::Album => "Album title",
            Self::ReleaseDate => "Release date",
            Self::Added => "Recently added",
            Self::Rating => "Highest rated",
            Self::Plays => "Most played",
            Self::Title => "Track title",
            Self::ArtistTitle => "Track artist / Title",
            Self::Duration => "Duration",
        }
    }
    pub fn spec(self, scope: Scope) -> SortSpec {
        use Field::*;
        let fields = match self {
            Self::Discography => vec![(AlbumArtist, false), (OriginalDate, false), (Album, false)],
            Self::ArtistAlbum => vec![(AlbumArtist, false), (Album, false)],
            Self::Album => vec![(Album, false), (AlbumArtist, false)],
            Self::ReleaseDate => vec![(OriginalDate, true), (AlbumArtist, false), (Album, false)],
            Self::Added => vec![(Added, true), (AlbumArtist, false), (Album, false)],
            Self::Rating => vec![(Rating, true), (AlbumArtist, false), (Album, false)],
            Self::Plays => vec![(Plays, true), (AlbumArtist, false), (Album, false)],
            Self::Title => vec![(Title, false), (Artist, false)],
            Self::ArtistTitle => vec![(Artist, false), (Title, false)],
            Self::Duration => vec![(Duration, false), (Title, false)],
        };
        SortSpec {
            scope,
            rules: fields.into_iter().map(|(f, d)| Rule::new(f, d)).collect(),
            grouping: if self == Self::Discography && scope == Scope::Albums {
                Grouping::AlbumArtist
            } else {
                Grouping::None
            },
            group_descending: false,
            ignore_the: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SortingPreferences {
    pub albums: SortSpec,
    pub tracks: SortSpec,
}
impl Default for SortingPreferences {
    fn default() -> Self {
        Self {
            albums: SortSpec::default(),
            tracks: Preset::Title.spec(Scope::Tracks),
        }
    }
}
impl SortingPreferences {
    pub fn get(&self, scope: Scope) -> SortSpec {
        let spec = match scope {
            Scope::Albums => &self.albums,
            Scope::Tracks => &self.tracks,
        }
        .validated(scope);
        if spec.preset().is_some() {
            spec
        } else {
            match scope {
                Scope::Albums => SortSpec::default(),
                Scope::Tracks => Preset::Title.spec(scope),
            }
        }
    }
    pub fn set(&mut self, spec: SortSpec) {
        match spec.scope {
            Scope::Albums => self.albums = spec.validated(Scope::Albums),
            Scope::Tracks => self.tracks = spec.validated(Scope::Tracks),
        }
    }
}

fn artist_text(value: &str, ignore_the: bool) -> String {
    let value = normalize(value.trim());
    if ignore_the {
        value.strip_prefix("the ").unwrap_or(&value).to_string()
    } else {
        value
    }
}

/// Order-preserving binary keys: normalized Unicode text with numeric runs as integers.
/// This is deterministic natural ordering, not language-specific reading/romanization.
pub fn natural_key(value: &str) -> Vec<u8> {
    let normalized = normalize(value.trim());
    let mut key = Vec::new();
    let mut chars = normalized.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_ascii_digit() {
            let mut digits = String::from(ch);
            while chars.peek().is_some_and(char::is_ascii_digit) {
                digits.push(chars.next().unwrap());
            }
            let significant = digits.trim_start_matches('0');
            let significant = if significant.is_empty() {
                "0"
            } else {
                significant
            };
            key.push(1);
            key.extend_from_slice(&(significant.len() as u64).to_be_bytes());
            key.extend_from_slice(significant.as_bytes());
        } else {
            key.push(2);
            key.extend_from_slice(&(ch as u32).to_be_bytes());
        }
    }
    key
}

fn date_parts(
    primary: Option<&serde_json::Value>,
    fallback: Option<&serde_json::Value>,
    year: Option<i32>,
) -> [Option<i32>; 3] {
    for date in [primary, fallback].into_iter().flatten() {
        let parts = if let Some(text) = date.as_str() {
            let mut fields = text.split('-');
            [
                fields.next().and_then(|s| s.parse().ok()),
                fields.next().and_then(|s| s.parse().ok()),
                fields
                    .next()
                    .and_then(|s| s.get(..2).unwrap_or(s).parse().ok()),
            ]
        } else {
            [
                date["year"].as_i64().and_then(|v| i32::try_from(v).ok()),
                date["month"].as_i64().and_then(|v| i32::try_from(v).ok()),
                date["day"].as_i64().and_then(|v| i32::try_from(v).ok()),
            ]
        };
        if parts[0].is_some_and(|y| y > 0) {
            let month = parts[1].filter(|m| (1..=12).contains(m));
            let day = month.and(parts[2].filter(|d| (1..=31).contains(d)));
            return [parts[0], month, day];
        }
    }
    [year.filter(|y| *y > 0), None, None]
}
fn timestamp(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
}
fn release_rank(types: &[String]) -> i32 {
    for (rank, label) in [(0, "album"), (1, "ep"), (2, "single")] {
        if types.iter().any(|s| s.eq_ignore_ascii_case(label)) {
            return rank;
        }
    }
    3
}
fn release_label(rank: i32) -> &'static str {
    match rank {
        0 => "Albums",
        1 => "EPs",
        2 => "Singles",
        _ => "Other releases",
    }
}

pub(crate) fn initialize(conn: &Connection) -> Result<()> {
    for (table, parent) in [("sort_albums", "albums"), ("sort_tracks", "tracks")] {
        conn.execute_batch(&format!("CREATE TABLE IF NOT EXISTS {table} (
            id TEXT PRIMARY KEY REFERENCES {parent}(id) ON DELETE CASCADE,
            title BLOB NOT NULL,artist BLOB NOT NULL,artist_article BLOB NOT NULL,
            album_artist BLOB NOT NULL,album_artist_article BLOB NOT NULL,album BLOB NOT NULL,
            original_year INTEGER,original_month INTEGER,original_day INTEGER,
            release_year INTEGER,release_month INTEGER,release_day INTEGER,
            added INTEGER,rating INTEGER,plays INTEGER,duration REAL,format BLOB NOT NULL,release_type INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS {table}_title ON {table}(title,id);
            CREATE INDEX IF NOT EXISTS {table}_artist_album ON {table}(album_artist_article,album,id);
            CREATE INDEX IF NOT EXISTS {table}_chronology ON {table}(album_artist_article,original_year,original_month,original_day,title,id);
            CREATE INDEX IF NOT EXISTS {table}_added ON {table}(added,id);"))?;
    }
    // Additive cache backfill. Never rewrite or delete source metadata to add sorting.
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt =
            tx.prepare("SELECT data FROM albums WHERE id NOT IN (SELECT id FROM sort_albums)")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            put_album(&tx, &serde_json::from_str(&row?)?)?;
        }
        let mut stmt =
            tx.prepare("SELECT data FROM tracks WHERE id NOT IN (SELECT id FROM sort_tracks)")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            put_track(&tx, &serde_json::from_str(&row?)?)?;
        }
    }
    tx.commit()?;
    Ok(())
}

struct Values<'a> {
    id: &'a str,
    title: &'a str,
    artist: &'a str,
    album_artist: &'a str,
    album: &'a str,
    original: [Option<i32>; 3],
    release: [Option<i32>; 3],
    added: Option<i64>,
    rating: Option<u8>,
    plays: Option<u64>,
    duration: f64,
    format: &'a str,
    release_type: i32,
}
fn put(conn: &Connection, table: &str, v: Values<'_>) -> Result<()> {
    let mut stmt = conn.prepare_cached(&format!("INSERT INTO {table} VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)
        ON CONFLICT(id) DO UPDATE SET title=excluded.title,artist=excluded.artist,artist_article=excluded.artist_article,album_artist=excluded.album_artist,album_artist_article=excluded.album_artist_article,album=excluded.album,original_year=excluded.original_year,original_month=excluded.original_month,original_day=excluded.original_day,release_year=excluded.release_year,release_month=excluded.release_month,release_day=excluded.release_day,added=excluded.added,rating=excluded.rating,plays=excluded.plays,duration=excluded.duration,format=excluded.format,release_type=excluded.release_type"))?;
    stmt.execute(params![
        v.id,
        natural_key(v.title),
        natural_key(v.artist),
        natural_key(&artist_text(v.artist, true)),
        natural_key(v.album_artist),
        natural_key(&artist_text(v.album_artist, true)),
        natural_key(v.album),
        v.original[0],
        v.original[1],
        v.original[2],
        v.release[0],
        v.release[1],
        v.release[2],
        v.added,
        v.rating.filter(|r| *r > 0),
        v.plays.and_then(|p| i64::try_from(p).ok()),
        v.duration,
        natural_key(v.format),
        v.release_type
    ])?;
    Ok(())
}
pub(crate) fn put_track(conn: &Connection, t: &Track) -> Result<()> {
    put(
        conn,
        "sort_tracks",
        Values {
            id: &t.id,
            title: if t.sort_name.is_empty() {
                &t.title
            } else {
                &t.sort_name
            },
            artist: t.artist_label(),
            album_artist: t.album_artist_label(),
            album: &t.album,
            original: date_parts(
                t.original_release_date.as_ref(),
                t.release_date.as_ref(),
                t.year,
            ),
            release: date_parts(t.release_date.as_ref(), None, t.year),
            added: timestamp(t.created.as_deref()),
            rating: t.user_rating,
            plays: t.play_count,
            duration: t.duration,
            format: &t.suffix,
            release_type: 3,
        },
    )
}
pub(crate) fn put_album(conn: &Connection, a: &Album) -> Result<()> {
    let title = if a.sort_name.is_empty() {
        &a.name
    } else {
        &a.sort_name
    };
    put(
        conn,
        "sort_albums",
        Values {
            id: &a.id,
            title,
            artist: &a.artist,
            album_artist: &a.artist,
            album: title,
            original: date_parts(
                a.original_release_date.as_ref(),
                a.release_date.as_ref(),
                a.year,
            ),
            release: date_parts(a.release_date.as_ref(), None, a.year),
            added: timestamp(a.created.as_deref()),
            rating: a.user_rating,
            plays: a.play_count,
            duration: a.duration,
            format: "",
            release_type: release_rank(&a.release_types),
        },
    )
}

pub(crate) const TRACK_JOINS: &str =
    " JOIN sort_tracks s ON s.id=tracks.id LEFT JOIN sort_albums a ON a.id=tracks.album_id";
pub(crate) const ALBUM_JOINS: &str = " JOIN sort_albums a ON a.id=albums.id";

pub(crate) fn available(conn: &Connection, scope: Scope) -> Result<Vec<Field>> {
    let table = if scope == Scope::Albums {
        "sort_albums"
    } else {
        "sort_tracks"
    };
    let mut fields = Field::choices(scope).to_vec();
    for (field, column) in [
        (Field::Added, "added"),
        (Field::Rating, "rating"),
        (Field::Plays, "plays"),
        (Field::OriginalDate, "original_year"),
        (Field::ReleaseDate, "release_year"),
    ] {
        let present: bool = conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {column} IS NOT NULL)"),
            [],
            |row| row.get(0),
        )?;
        if !present {
            fields.retain(|candidate| *candidate != field);
        }
    }
    Ok(fields)
}

fn expression(field: Field, spec: &SortSpec, tracks: bool) -> Vec<String> {
    let alias = if tracks && spec.scope == Scope::Tracks {
        "s"
    } else {
        "a"
    };
    let artist = if spec.ignore_the {
        "album_artist_article"
    } else {
        "album_artist"
    };
    match field {
        Field::AlbumArtist => vec![if tracks {
            format!("coalesce(a.{artist},s.{artist})")
        } else {
            format!("a.{artist}")
        }],
        Field::Album => vec![if tracks {
            "coalesce(a.title,s.album)".into()
        } else {
            "a.title".into()
        }],
        Field::Artist => vec![format!(
            "s.{}",
            if spec.ignore_the {
                "artist_article"
            } else {
                "artist"
            }
        )],
        Field::Title => vec!["s.title".into()],
        Field::OriginalDate | Field::ReleaseDate => {
            let kind = if field == Field::OriginalDate {
                "original"
            } else {
                "release"
            };
            ["year", "month", "day"]
                .iter()
                .map(|part| format!("{alias}.{kind}_{part}"))
                .collect()
        }
        Field::Added => vec![format!("{alias}.added")],
        Field::Rating => vec![format!("{alias}.rating")],
        Field::Plays => vec![format!("{alias}.plays")],
        Field::Duration => vec![format!("{alias}.duration")],
        Field::Format => vec!["s.format".into()],
        Field::ReleaseType => vec!["a.release_type".into()],
    }
}

/// Only enum-selected SQL fragments enter ORDER BY.
pub(crate) fn order(spec: &SortSpec, tracks: bool) -> String {
    let spec = spec.validated(spec.scope);
    let mut terms = Vec::new();
    let mut add = |expressions: Vec<String>, descending: bool| {
        for expr in expressions {
            terms.push(format!("({expr} IS NULL) ASC"));
            terms.push(format!(
                "{expr} {}",
                if descending { "DESC" } else { "ASC" }
            ));
        }
    };
    match spec.grouping {
        Grouping::None => (),
        Grouping::AlbumArtist => add(
            expression(Field::AlbumArtist, &spec, tracks),
            spec.group_descending,
        ),
        Grouping::Year => add(vec!["a.original_year".into()], spec.group_descending),
        Grouping::ReleaseType => add(vec!["a.release_type".into()], spec.group_descending),
    }
    for rule in &spec.rules {
        // A grouping field controls its outer order, rather than being reversed twice.
        if spec.grouping == Grouping::AlbumArtist && rule.field == Field::AlbumArtist {
            continue;
        }
        add(expression(rule.field, &spec, tracks), rule.descending);
    }
    if tracks {
        terms.extend(
            [
                "tracks.album_id ASC",
                "coalesce(tracks.disc,1) ASC",
                "(tracks.track IS NULL OR tracks.track=0) ASC",
                "tracks.track ASC",
                "s.title ASC",
                "tracks.id ASC",
            ]
            .map(str::to_string),
        );
    } else {
        terms.extend(["a.album_artist ASC", "a.title ASC", "albums.id ASC"].map(str::to_string));
    }
    format!(" ORDER BY {}", terms.join(","))
}
