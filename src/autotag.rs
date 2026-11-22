//! Filling tags from a MusicBrainz release: candidates, track matching and
//! the values each matched song would get, in the spirit of Picard.
use crate::{model::Track, tags::TagField};
use std::collections::BTreeMap;

/// What a release search looks for: the album as the songs are tagged now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseQuery {
    pub album: String,
    pub artist: String,
    /// The number of songs, or 0 to leave it out.
    pub tracks: usize,
    /// For one song: its title, so the search looks for the recording and
    /// offers the releases it appears on.
    pub title: String,
    pub length_ms: Option<u64>,
    /// A pasted recording ID, searched for the releases it appears on.
    pub recording: String,
}

impl ReleaseQuery {
    pub fn for_tracks(tracks: &[Track]) -> Self {
        let first = |pick: fn(&Track) -> &str| {
            tracks
                .iter()
                .map(pick)
                .find(|value| !value.is_empty())
                .unwrap_or_default()
                .to_string()
        };
        if let [song] = tracks {
            let artist = if song.artist.is_empty() {
                &song.album_artist
            } else {
                &song.artist
            };
            return Self {
                album: song.album.clone(),
                artist: artist.clone(),
                tracks: 0,
                title: song.title.clone(),
                length_ms: (song.duration > 0.).then(|| (song.duration * 1000.) as u64),
                ..Default::default()
            };
        }
        let mut artist = first(|track| &track.album_artist);
        if artist.is_empty() {
            artist = first(|track| &track.artist);
        }
        Self {
            album: first(|track| &track.album),
            artist,
            tracks: if tracks.len() > 1 { tracks.len() } else { 0 },
            ..Default::default()
        }
    }

    /// Whether songs from more than one album are selected, which neither
    /// an album nor a song search can fill in one go.
    pub fn spans_albums(tracks: &[Track]) -> bool {
        tracks.len() > 1
            && tracks
                .iter()
                .any(|track| track.album_id != tracks[0].album_id)
    }

    pub fn is_song(&self) -> bool {
        !self.title.is_empty() || !self.recording.is_empty()
    }

    /// What the search is for, as the editor names it while it runs.
    pub fn subject(&self) -> &str {
        if self.is_song() {
            &self.title
        } else {
            &self.album
        }
    }

    /// The Lucene query MusicBrainz's release search takes.
    pub fn lucene(&self) -> String {
        if !self.recording.is_empty() {
            return format!("rid:{}", self.recording);
        }
        if self.is_song() {
            let mut terms = vec![format!("recording:\"{}\"", escape(&self.title))];
            if !self.artist.is_empty() {
                terms.push(format!("artist:\"{}\"", escape(&self.artist)));
            }
            if let Some(length) = self.length_ms {
                let slack = 3_000;
                terms.push(format!(
                    "dur:[{} TO {}]",
                    length.saturating_sub(slack),
                    length + slack
                ));
            }
            return terms.join(" AND ");
        }
        let mut terms = vec![format!("release:\"{}\"", escape(&self.album))];
        if !self.artist.is_empty() {
            terms.push(format!("artist:\"{}\"", escape(&self.artist)));
        }
        if self.tracks > 0 {
            terms.push(format!("tracks:{}", self.tracks));
        }
        terms.join(" AND ")
    }
}

/// What a pasted MusicBrainz link or bare ID points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Release(String),
    Recording(String),
}

/// Reads a pasted release or recording URL, or a bare ID taken as a release.
pub fn parse_link(text: &str) -> Option<Link> {
    let text = text.trim().to_ascii_lowercase();
    let is_id = |id: &str| {
        id.len() == 36
            && id.char_indices().all(|(at, c)| match at {
                8 | 13 | 18 | 23 => c == '-',
                _ => c.is_ascii_hexdigit(),
            })
    };
    if is_id(&text) {
        return Some(Link::Release(text));
    }
    let path = text.split("musicbrainz.org/").nth(1)?;
    let mut parts = path.split(['/', '?', '#']);
    let kind = parts.next()?;
    let id = parts.next().filter(|id| is_id(id))?.to_string();
    match kind {
        "release" => Some(Link::Release(id)),
        "recording" => Some(Link::Recording(id)),
        _ => None,
    }
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// One edition a search found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseCandidate {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub date: String,
    pub country: String,
    pub format: String,
    pub label: String,
    pub catalog: String,
    pub tracks: u32,
    pub score: u32,
}

impl ReleaseCandidate {
    /// One line telling editions apart.
    pub fn summary(&self) -> String {
        let label = match (self.label.is_empty(), self.catalog.is_empty()) {
            (false, false) => format!("{} {}", self.label, self.catalog),
            (false, true) => self.label.clone(),
            (true, false) => self.catalog.clone(),
            (true, true) => String::new(),
        };
        let tracks = format!(
            "{} {}",
            self.tracks,
            if self.tracks == 1 { "track" } else { "tracks" }
        );
        [&self.date, &self.country, &self.format, &label, &tracks]
            .into_iter()
            .filter(|part| !part.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// A release with its tracks, as a lookup returns it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Release {
    pub id: String,
    pub title: String,
    pub artists: Vec<(String, String)>,
    pub group_id: String,
    pub date: String,
    pub genres: Vec<String>,
    pub tracks: Vec<LookupTrack>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LookupTrack {
    pub id: String,
    pub recording_id: String,
    pub title: String,
    /// Credited artists: name and MusicBrainz ID.
    pub artists: Vec<(String, String)>,
    pub number: u32,
    pub disc: u32,
    pub length_ms: Option<u64>,
}

/// A song's best track on the release and how sure the match is (0–1).
pub type TrackMatch = Option<(usize, f64)>;

/// Below this a song is left unmatched rather than guessed.
pub const MIN_SCORE: f64 = 0.5;

/// Pairs each song with a release track, best scores first, each track used
/// once. Weights follow Picard's: the title counts most, then the length,
/// artist and position.
pub fn match_tracks(songs: &[Track], release: &Release) -> Vec<TrackMatch> {
    let mut pairs: Vec<(f64, usize, usize)> = songs
        .iter()
        .enumerate()
        .flat_map(|(song, track)| {
            release
                .tracks
                .iter()
                .enumerate()
                .map(move |(index, candidate)| (score(track, candidate), song, index))
        })
        .filter(|(score, ..)| *score >= MIN_SCORE)
        .collect();
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut matches = vec![None; songs.len()];
    let mut used = vec![false; release.tracks.len()];
    for (score, song, index) in pairs {
        if matches[song].is_none() && !used[index] {
            matches[song] = Some((index, score));
            used[index] = true;
        }
    }
    matches
}

fn score(song: &Track, candidate: &LookupTrack) -> f64 {
    let mut parts = vec![(22.0, similarity(&song.title, &candidate.title))];
    let credited = names(&candidate.artists);
    if !song.artist.is_empty() && !credited.is_empty() {
        parts.push((6.0, similarity(&song.artist, &credited.join(" "))));
    }
    if let Some(number) = song.track {
        parts.push((6.0, f64::from(u8::from(number == candidate.number))));
    }
    if let Some(disc) = song.disc_number {
        parts.push((5.0, f64::from(u8::from(disc == candidate.disc))));
    }
    if let Some(length) = candidate.length_ms
        && song.duration > 0.0
    {
        let diff = (song.duration - length as f64 / 1000.0).abs();
        parts.push((10.0, (1.0 - diff / 30.0).max(0.0)));
    }
    let weight: f64 = parts.iter().map(|(w, _)| w).sum();
    parts.iter().map(|(w, s)| w * s).sum::<f64>() / weight
}

/// How alike two names are, 0–1, ignoring case, punctuation and spacing.
pub fn similarity(a: &str, b: &str) -> f64 {
    let fold = |text: &str| -> Vec<char> {
        text.to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect()
    };
    let (a, b) = (fold(a), fold(b));
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(prev + usize::from(ca != cb));
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    1.0 - row[b.len()] as f64 / a.len().max(b.len()) as f64
}

fn names(credits: &[(String, String)]) -> Vec<String> {
    credits.iter().map(|(name, _)| name.clone()).collect()
}

fn ids(credits: &[(String, String)]) -> String {
    credits
        .iter()
        .map(|(_, id)| id.as_str())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The tags a song matched to `track` would get. IDs come too, so a later
/// rescan or lookup finds the same recording.
pub fn proposed(release: &Release, track: &LookupTrack) -> BTreeMap<TagField, String> {
    let mut values = BTreeMap::from([
        (TagField::Title, track.title.clone()),
        (TagField::Artist, names(&track.artists).join("; ")),
        (TagField::AlbumArtist, names(&release.artists).join("; ")),
        (TagField::Album, release.title.clone()),
        (TagField::Track, track.number.to_string()),
        (TagField::Disc, track.disc.to_string()),
        (TagField::MusicBrainzTrack, track.recording_id.clone()),
        (TagField::MusicBrainzReleaseTrack, track.id.clone()),
        (TagField::MusicBrainzRelease, release.id.clone()),
        (TagField::MusicBrainzReleaseGroup, release.group_id.clone()),
        (TagField::MusicBrainzArtist, ids(&track.artists)),
        (TagField::MusicBrainzAlbumArtist, ids(&release.artists)),
        (
            TagField::CoverArt,
            format!("{}{}", crate::tags::COVER_PREFIX, release.id),
        ),
    ]);
    if let Some(year) = release.date.get(..4).filter(|y| y.parse::<u32>().is_ok()) {
        values.insert(TagField::Year, year.to_string());
    }
    if !release.genres.is_empty() {
        values.insert(TagField::Genre, release.genres.join("; "));
    }
    values.retain(|_, value| !value.is_empty());
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(title: &str, number: u32, seconds: f64) -> Track {
        Track {
            title: title.into(),
            artist: "Radiohead".into(),
            track: Some(number),
            duration: seconds,
            ..Default::default()
        }
    }

    fn release() -> Release {
        let track = |title: &str, number, ms| LookupTrack {
            id: format!("t{number}"),
            recording_id: format!("r{number}"),
            title: title.into(),
            artists: vec![("Radiohead".into(), "a1".into())],
            number,
            disc: 1,
            length_ms: Some(ms),
        };
        Release {
            id: "rel".into(),
            title: "OK Computer".into(),
            artists: vec![("Radiohead".into(), "a1".into())],
            group_id: "rg".into(),
            date: "1997-05-21".into(),
            genres: vec!["alternative rock".into()],
            tracks: vec![
                track("Airbag", 1, 284_000),
                track("Paranoid Android", 2, 383_000),
                track("Subterranean Homesick Alien", 3, 267_000),
            ],
        }
    }

    #[test]
    fn query_quotes_album_and_artist_and_counts_tracks() {
        let tracks = vec![
            Track {
                album: "Say \"Hi\"".into(),
                artist: "A".into(),
                ..Default::default()
            },
            Track::default(),
        ];
        assert_eq!(
            ReleaseQuery::for_tracks(&tracks).lucene(),
            r#"release:"Say \"Hi\"" AND artist:"A" AND tracks:2"#
        );
        assert_eq!(
            ReleaseQuery::for_tracks(&tracks[..1]).lucene(),
            r#"release:"Say \"Hi\"" AND artist:"A""#
        );
    }

    #[test]
    fn similarity_ignores_case_and_punctuation() {
        assert_eq!(similarity("Paranoid Android", "paranoid android!"), 1.0);
        assert!(similarity("Airbag", "Airbags") > 0.8);
        assert!(similarity("Airbag", "Lucky") < 0.3);
    }

    #[test]
    fn songs_match_their_tracks_even_out_of_order_or_misspelt() {
        let songs = vec![
            song("paranoid andriod", 2, 383.0),
            song("Airbag", 1, 285.0),
            song("Something else entirely", 9, 30.0),
        ];
        let matches = match_tracks(&songs, &release());
        assert_eq!(matches[0].map(|m| m.0), Some(1));
        assert_eq!(matches[1].map(|m| m.0), Some(0));
        assert!(matches[1].unwrap().1 > 0.95);
        assert_eq!(matches[2], None);
    }

    #[test]
    fn each_release_track_is_used_once() {
        let songs = vec![song("Airbag", 1, 284.0), song("Airbag", 1, 284.0)];
        let matches = match_tracks(&songs, &release());
        assert_eq!(matches[0].map(|m| m.0), Some(0));
        assert_ne!(matches[1].map(|m| m.0), Some(0));
    }

    #[test]
    fn proposals_fill_visible_tags_and_ids() {
        let release = release();
        let values = proposed(&release, &release.tracks[1]);
        assert_eq!(values[&TagField::Title], "Paranoid Android");
        assert_eq!(values[&TagField::Year], "1997");
        assert_eq!(values[&TagField::Track], "2");
        assert_eq!(values[&TagField::Genre], "alternative rock");
        assert_eq!(values[&TagField::MusicBrainzTrack], "r2");
        assert_eq!(values[&TagField::MusicBrainzAlbumArtist], "a1");
        assert_eq!(values[&TagField::CoverArt], "coverart:rel");
        assert!(!values.contains_key(&TagField::Comment));
    }

    #[test]
    fn summary_tells_editions_apart() {
        let candidate = ReleaseCandidate {
            date: "1997-05-21".into(),
            country: "GB".into(),
            format: "CD".into(),
            label: "Parlophone".into(),
            catalog: "NODATA 02".into(),
            tracks: 12,
            ..Default::default()
        };
        assert_eq!(
            candidate.summary(),
            "1997-05-21 · GB · CD · Parlophone NODATA 02 · 12 tracks"
        );
    }

    #[test]
    fn one_song_searches_its_recording_with_a_duration_window() {
        let song = Track {
            title: "Pyramid \"Song\"".into(),
            artist: "Radiohead".into(),
            album: "Amnesiac".into(),
            album_id: "a".into(),
            duration: 289.,
            ..Default::default()
        };
        let query = ReleaseQuery::for_tracks(std::slice::from_ref(&song));
        assert!(query.is_song());
        assert_eq!(query.subject(), "Pyramid \"Song\"");
        assert_eq!(
            query.lucene(),
            r#"recording:"Pyramid \"Song\"" AND artist:"Radiohead" AND dur:[286000 TO 292000]"#
        );
        let other = Track {
            album_id: "b".into(),
            ..song.clone()
        };
        assert!(!ReleaseQuery::spans_albums(&[song.clone(), song.clone()]));
        assert!(ReleaseQuery::spans_albums(&[song, other]));
    }

    #[test]
    fn pasted_links_and_ids_are_recognised() {
        let id = "b84ee12a-09ef-421b-82de-0441a926375b";
        assert_eq!(
            parse_link(&format!(" https://musicbrainz.org/release/{id}/discids ")),
            Some(Link::Release(id.into()))
        );
        assert_eq!(
            parse_link(&format!("musicbrainz.org/recording/{id}")),
            Some(Link::Recording(id.into()))
        );
        assert_eq!(parse_link(id), Some(Link::Release(id.into())));
        assert_eq!(
            parse_link(&id.to_uppercase()),
            Some(Link::Release(id.into()))
        );
        assert_eq!(
            parse_link(&format!("https://musicbrainz.org/artist/{id}")),
            None
        );
        assert_eq!(parse_link("b84ee12a-09ef-421b"), None);
        assert_eq!(parse_link(""), None);
    }
}
