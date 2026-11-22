//! Read-only MusicBrainz lookups: artist discographies and the releases
//! the tag editor fills tags from.
use crate::autotag::{LookupTrack, Release, ReleaseCandidate, ReleaseQuery};
use crate::discography::{
    Discography, ListenLink, ReleaseDetails, ReleaseGroup, ReleaseTrack, fold, listen_service,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::Mutex, time::Instant};
use url::Url;

const PAGE: usize = 100;
/// Artists with more releases than this are listed by release group instead.
const MAX_RELEASES: usize = 1000;
const MAX_GROUP_PAGES: usize = 10;

#[derive(Clone)]
pub struct MusicBrainz {
    http: reqwest::Client,
    base: Url,
    covers: Url,
    /// MusicBrainz blocks clients that exceed one request per second.
    interval: Duration,
    last: Arc<Mutex<Option<Instant>>>,
}

impl MusicBrainz {
    pub fn new() -> Result<Self> {
        Self::with_endpoints("https://musicbrainz.org/ws/2/", Duration::from_millis(1100))
    }
    pub fn with_endpoints(base: &str, interval: Duration) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(concat!(
                    "Sonora/",
                    env!("CARGO_PKG_VERSION"),
                    " ( desktop Navidrome player )"
                ))
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()?,
            base: Url::parse(base)?,
            covers: Url::parse("https://coverartarchive.org/")?,
            interval,
            last: Arc::default(),
        })
    }

    /// Fetches front covers from `base` instead of Cover Art Archive.
    pub fn with_covers(mut self, base: &str) -> Result<Self> {
        self.covers = Url::parse(base)?;
        Ok(self)
    }

    /// A release's front cover from Cover Art Archive, as a tag value.
    pub async fn front_cover(&self, release: &str) -> Result<String> {
        let url = self.covers.join(&format!("release/{release}/front-500"))?;
        let response = self.http.get(url).send().await.map_err(|error| {
            anyhow::Error::new(error.without_url()).context("Cannot reach Cover Art Archive")
        })?;
        match response.status().as_u16() {
            404 => bail!("Cover Art Archive has no front cover for this release"),
            status if !(200..300).contains(&status) => {
                bail!("Cover Art Archive returned HTTP {status}")
            }
            _ => {}
        }
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .filter(|mime| mime.starts_with("image/"))
            .unwrap_or("image/jpeg")
            .to_string();
        let data = response.bytes().await.map_err(|error| {
            anyhow::Error::new(error.without_url()).context("Cannot download the cover")
        })?;
        Ok(crate::tags::picture_value(&mime, &data))
    }

    async fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Value> {
        let mut url = self.base.join(path)?;
        url.query_pairs_mut()
            .extend_pairs(params)
            .append_pair("fmt", "json");
        for attempt in 0..2 {
            {
                let mut last = self.last.lock().await;
                if let Some(at) = *last {
                    tokio::time::sleep_until(at + self.interval).await;
                }
                *last = Some(Instant::now());
            }
            let response = self.http.get(url.clone()).send().await.map_err(|error| {
                anyhow::Error::new(error.without_url()).context("Cannot reach MusicBrainz")
            })?;
            let status = response.status();
            if status.as_u16() == 503 && attempt == 0 {
                tokio::time::sleep(self.interval * 3).await;
                continue;
            }
            if !status.is_success() {
                bail!("MusicBrainz returned HTTP {}", status.as_u16());
            }
            return response
                .json()
                .await
                .map_err(|_| anyhow::anyhow!("MusicBrainz returned an invalid response"));
        }
        bail!("MusicBrainz is busy")
    }

    /// Every release group credited to the artist. Fetching by release also
    /// yields edition titles and the release IDs Navidrome reports for albums.
    /// `overview` skips straight to the release-group listing for an artist
    /// already known to have too many releases.
    pub async fn discography(&self, artist: &str, overview: bool) -> Result<Discography> {
        let mut groups: Vec<ReleaseGroup> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut offset = 0;
        let mut total = usize::MAX;
        let mut overview = overview;
        if overview {
            groups = self.groups_only(artist).await?;
            total = 0;
        }
        while offset < total {
            let page = self
                .get(
                    "release",
                    &[
                        ("artist", artist),
                        ("inc", "release-groups"),
                        ("limit", "100"),
                        ("offset", &offset.to_string()),
                    ],
                )
                .await?;
            total = page["release-count"].as_u64().unwrap_or(0) as usize;
            if total > MAX_RELEASES {
                groups = self.groups_only(artist).await?;
                overview = true;
                break;
            }
            let releases = page["releases"].as_array().cloned().unwrap_or_default();
            if releases.is_empty() {
                break;
            }
            offset += releases.len();
            for release in releases {
                let Some(mut group) = parse_group(&release["release-group"]) else {
                    continue;
                };
                let at = *index.entry(group.id.clone()).or_insert_with(|| {
                    group.official = false;
                    groups.push(group);
                    groups.len() - 1
                });
                let group = &mut groups[at];
                group.official |= release["status"].as_str() == Some("Official");
                if let Some(id) = release["id"].as_str() {
                    group.releases.push(id.into());
                }
                if let Some(title) = release["title"].as_str()
                    && title != group.title
                    && !group.titles.iter().any(|known| known == title)
                {
                    group.titles.push(title.into());
                }
            }
        }
        Ok(Discography {
            artist: artist.into(),
            groups,
            fetched_ms: chrono::Utc::now().timestamp_millis(),
            overview,
        })
    }

    /// MusicBrainz's own default overview: groups with at least one official release.
    async fn groups_only(&self, artist: &str) -> Result<Vec<ReleaseGroup>> {
        let mut groups = Vec::new();
        for page in 0..MAX_GROUP_PAGES {
            let value = self
                .get(
                    "release-group",
                    &[
                        ("artist", artist),
                        ("release-group-status", "website-default"),
                        ("limit", "100"),
                        ("offset", &(page * PAGE).to_string()),
                    ],
                )
                .await?;
            let items = value["release-groups"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            groups.extend(items.iter().filter_map(parse_group));
            if items.len() < PAGE
                || groups.len() >= value["release-group-count"].as_u64().unwrap_or(0) as usize
            {
                break;
            }
        }
        Ok(groups)
    }

    /// Finds an artist the server has no MusicBrainz ID for. Namesakes are
    /// common, so a candidate counts only if it alone shares a release title
    /// with the library. `titles` are folded library album titles. With no
    /// titles (an artist the library lacks), only a unique name match counts.
    pub async fn resolve_artist(
        &self,
        name: &str,
        titles: &HashSet<String>,
    ) -> Result<Option<String>> {
        let phrase = name.replace(['"', '\\'], " ");
        let found = self
            .get(
                "artist",
                &[
                    (
                        "query",
                        &format!("artist:\"{phrase}\" OR alias:\"{phrase}\""),
                    ),
                    ("limit", "5"),
                ],
            )
            .await?;
        let wanted = fold(name);
        let candidates = found["artists"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|artist| {
                fold(artist["name"].as_str().unwrap_or_default()) == wanted
                    || artist["aliases"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|alias| fold(alias["name"].as_str().unwrap_or_default()) == wanted)
            })
            .filter_map(|artist| artist["id"].as_str().map(str::to_string))
            .take(3)
            .collect::<Vec<_>>();
        if titles.is_empty() {
            return Ok(match candidates.as_slice() {
                [only] => Some(only.clone()),
                _ => None,
            });
        }
        let mut best: Option<(usize, String)> = None;
        let mut tied = false;
        for id in candidates {
            let value = self
                .get("release-group", &[("artist", &id), ("limit", "100")])
                .await?;
            let overlap = value["release-groups"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|group| titles.contains(&fold(group["title"].as_str().unwrap_or_default())))
                .count();
            if overlap == 0 {
                continue;
            }
            match &best {
                Some((score, _)) if *score > overlap => {}
                Some((score, _)) if *score == overlap => tied = true,
                _ => {
                    best = Some((overlap, id));
                    tied = false;
                }
            }
        }
        Ok(best.filter(|_| !tied).map(|(_, id)| id))
    }

    /// Official editions of a release group with their media.
    async fn editions(&self, group: &str, inc: &str) -> Result<Vec<Value>> {
        let value = self
            .get(
                "release",
                &[
                    ("release-group", group),
                    ("inc", inc),
                    ("status", "official"),
                    ("limit", "100"),
                ],
            )
            .await?;
        Ok(value["releases"].as_array().cloned().unwrap_or_default())
    }

    /// The usual number of tracks on official editions of a release group.
    pub async fn track_count(&self, group: &str) -> Result<Option<u32>> {
        Ok(usual_track_count(&self.editions(group, "media").await?))
    }

    /// Track list, label and listening links of the usual edition of a release group.
    pub async fn release_details(&self, group: &str) -> Result<ReleaseDetails> {
        // Streaming pages usually sit on digital editions, not the one listed.
        let editions = self.editions(group, "media url-rels").await?;
        let usual = usual_track_count(&editions);
        let chosen = editions
            .iter()
            .filter(|release| usual.is_none_or(|count| edition_tracks(release) == count))
            .filter(|release| release["id"].as_str().is_some_and(|id| !id.is_empty()))
            // Undated editions sort last.
            .min_by_key(|release| {
                let date = release["date"].as_str().unwrap_or_default();
                (date.is_empty(), date.to_string())
            })
            .and_then(|release| release["id"].as_str())
            .context("MusicBrainz lists no official edition of this release")?
            .to_string();
        let release = self
            .get(
                &format!("release/{chosen}"),
                &[(
                    "inc",
                    "recordings artist-credits labels url-rels release-group-level-rels genres",
                )],
            )
            .await?;
        let mut details = parse_release(&release);
        for link in editions
            .iter()
            .filter_map(|edition| edition["relations"].as_array())
            .flatten()
            .filter_map(|relation| relation["url"]["resource"].as_str())
        {
            if let Some(service) = listen_service(link)
                && !details.links.iter().any(|known| known.url == link)
            {
                details.links.push(ListenLink {
                    service: service.into(),
                    url: link.into(),
                    direct: true,
                });
            }
        }
        Ok(details)
    }
}

impl MusicBrainz {
    /// Editions matching the album as it is tagged now, best match first.
    pub async fn search_releases(&self, query: &ReleaseQuery) -> Result<Vec<ReleaseCandidate>> {
        if query.is_song() {
            let found = self
                .get("recording", &[("query", &query.lucene()), ("limit", "10")])
                .await?;
            return Ok(recording_candidates(&found));
        }
        let found = self
            .get("release", &[("query", &query.lucene()), ("limit", "10")])
            .await?;
        Ok(found["releases"]
            .as_array()
            .into_iter()
            .flatten()
            .map(parse_candidate)
            .collect())
    }

    /// A release with its tracks, credits, IDs and genres.
    pub async fn release(&self, id: &str) -> Result<Release> {
        let release = self
            .get(
                &format!("release/{id}"),
                &[(
                    "inc",
                    "recordings artist-credits labels release-groups genres",
                )],
            )
            .await?;
        if release["id"].as_str().unwrap_or_default().is_empty() {
            bail!("MusicBrainz has no release {id}");
        }
        Ok(parse_lookup(&release))
    }
}

/// Every release the found recordings appear on, best recording first, each
/// scored by its recording.
fn recording_candidates(found: &Value) -> Vec<ReleaseCandidate> {
    let mut candidates: Vec<ReleaseCandidate> = vec![];
    for recording in found["recordings"].as_array().into_iter().flatten() {
        for release in recording["releases"].as_array().into_iter().flatten() {
            let mut candidate = parse_candidate(release);
            if candidates.iter().any(|known| known.id == candidate.id) {
                continue;
            }
            if candidate.artist.is_empty() {
                candidate.artist = credit(&recording["artist-credit"]);
            }
            candidate.score = recording["score"].as_u64().unwrap_or_default() as u32;
            candidates.push(candidate);
        }
    }
    candidates.truncate(10);
    candidates
}

fn parse_candidate(release: &Value) -> ReleaseCandidate {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let label = release["label-info"]
        .as_array()
        .and_then(|labels| labels.first())
        .cloned()
        .unwrap_or_default();
    let mut formats: Vec<String> = vec![];
    for medium in release["media"].as_array().into_iter().flatten() {
        let format = text(&medium["format"]);
        if !format.is_empty() && !formats.contains(&format) {
            formats.push(format);
        }
    }
    let media = release["media"].as_array().map_or(0, Vec::len);
    let format = match formats.as_slice() {
        [one] if media > 1 => format!("{media}×{one}"),
        _ => formats.join(" + "),
    };
    ReleaseCandidate {
        id: text(&release["id"]),
        title: text(&release["title"]),
        artist: credit(&release["artist-credit"]),
        date: text(&release["date"]),
        country: text(&release["country"]),
        format,
        label: text(&label["label"]["name"]),
        catalog: text(&label["catalog-number"]),
        tracks: release["track-count"]
            .as_u64()
            .map(|n| n as u32)
            .unwrap_or_else(|| edition_tracks(release)),
        score: release["score"].as_u64().unwrap_or_default() as u32,
    }
}

fn credits(value: &Value) -> Vec<(String, String)> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|part| {
            let name = part["artist"]["name"]
                .as_str()
                .or(part["name"].as_str())
                .unwrap_or_default();
            let id = part["artist"]["id"].as_str().unwrap_or_default();
            (name.to_string(), id.to_string())
        })
        .collect()
}

fn parse_lookup(release: &Value) -> Release {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let genres = |value: &Value| {
        let mut genres: Vec<(u64, String)> = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|genre| {
                Some((genre["count"].as_u64().unwrap_or(0), text(&genre["name"])))
                    .filter(|(count, name)| *count > 0 && !name.is_empty())
            })
            .collect();
        genres.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        genres
            .into_iter()
            .take(2)
            .map(|(_, name)| name)
            .collect::<Vec<_>>()
    };
    let mut release_genres = genres(&release["genres"]);
    if release_genres.is_empty() {
        release_genres = genres(&release["release-group"]["genres"]);
    }
    let artists = credits(&release["artist-credit"]);
    let tracks = release["media"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .flat_map(|(index, medium)| {
            let disc = medium["position"].as_u64().unwrap_or(index as u64 + 1) as u32;
            let artists = artists.clone();
            medium["tracks"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(move |(position, track)| {
                    let mut credited = credits(&track["artist-credit"]);
                    if credited.is_empty() {
                        credited = credits(&track["recording"]["artist-credit"]);
                    }
                    if credited.is_empty() {
                        credited = artists.clone();
                    }
                    LookupTrack {
                        id: text(&track["id"]),
                        recording_id: text(&track["recording"]["id"]),
                        title: track["title"]
                            .as_str()
                            .or(track["recording"]["title"].as_str())
                            .unwrap_or_default()
                            .to_string(),
                        artists: credited,
                        number: track["position"]
                            .as_u64()
                            .map(|n| n as u32)
                            .unwrap_or(position as u32 + 1),
                        disc,
                        length_ms: track["length"]
                            .as_u64()
                            .or(track["recording"]["length"].as_u64()),
                    }
                })
        })
        .collect();
    Release {
        id: text(&release["id"]),
        title: text(&release["title"]),
        artists,
        group_id: text(&release["release-group"]["id"]),
        date: text(&release["date"]),
        genres: release_genres,
        tracks,
    }
}

fn edition_tracks(release: &Value) -> u32 {
    release["media"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|medium| medium["track-count"].as_u64())
        .sum::<u64>() as u32
}

fn usual_track_count(editions: &[Value]) -> Option<u32> {
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for release in editions {
        let tracks = edition_tracks(release);
        if tracks > 0 {
            *counts.entry(tracks).or_default() += 1;
        }
    }
    // Ties go to the shorter edition, which is the plain one.
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(tracks, _)| tracks)
}

fn credit(value: &Value) -> String {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|part| {
            format!(
                "{}{}",
                part["name"].as_str().unwrap_or_default(),
                part["joinphrase"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

fn parse_release(release: &Value) -> ReleaseDetails {
    let album_credit = credit(&release["artist-credit"]);
    let mut tracks = Vec::new();
    for (index, medium) in release["media"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let disc = medium["position"].as_u64().unwrap_or(index as u64 + 1) as u32;
        for (position, track) in medium["tracks"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let artist = credit(&track["artist-credit"]);
            tracks.push(ReleaseTrack {
                disc,
                number: track["number"]
                    .as_str()
                    .filter(|number| !number.is_empty())
                    .map_or_else(|| (position + 1).to_string(), str::to_string),
                title: track["title"]
                    .as_str()
                    .or_else(|| track["recording"]["title"].as_str())
                    .unwrap_or_default()
                    .into(),
                seconds: track["length"]
                    .as_u64()
                    .or_else(|| track["recording"]["length"].as_u64())
                    .map(|ms| ((ms + 500) / 1000) as u32),
                artist: if artist.is_empty() || artist == album_credit {
                    String::new()
                } else {
                    artist
                },
            });
        }
    }
    let label = release["label-info"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|info| info["label"]["name"].as_str().is_some());
    let mut links: Vec<ListenLink> = Vec::new();
    let relations = release["relations"].as_array().into_iter().flatten().chain(
        release["release-group"]["relations"]
            .as_array()
            .into_iter()
            .flatten(),
    );
    for relation in relations {
        let Some(url) = relation["url"]["resource"].as_str() else {
            continue;
        };
        if let Some(service) = listen_service(url)
            && !links.iter().any(|known| known.url == url)
        {
            links.push(ListenLink {
                service: service.into(),
                url: url.into(),
                direct: true,
            });
        }
    }
    let mut genres = release["genres"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|genre| {
            Some((
                genre["count"].as_u64().unwrap_or(0),
                genre["name"].as_str()?.to_string(),
            ))
        })
        .collect::<Vec<_>>();
    genres.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    ReleaseDetails {
        release: release["id"].as_str().unwrap_or_default().into(),
        date: release["date"].as_str().unwrap_or_default().into(),
        country: release["country"].as_str().unwrap_or_default().into(),
        label: label
            .and_then(|info| info["label"]["name"].as_str())
            .unwrap_or_default()
            .into(),
        catalog_number: label
            .and_then(|info| info["catalog-number"].as_str())
            .unwrap_or_default()
            .into(),
        genres: genres.into_iter().take(4).map(|(_, name)| name).collect(),
        tracks,
        links,
        fetched_ms: chrono::Utc::now().timestamp_millis(),
    }
}

fn parse_group(value: &Value) -> Option<ReleaseGroup> {
    Some(ReleaseGroup {
        id: value["id"].as_str().filter(|id| !id.is_empty())?.into(),
        title: value["title"].as_str().context("untitled").ok()?.into(),
        primary: value["primary-type"].as_str().unwrap_or_default().into(),
        secondary: value["secondary-types"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|kind| kind.as_str().map(str::to_string))
            .collect(),
        date: value["first-release-date"]
            .as_str()
            .unwrap_or_default()
            .into(),
        official: true,
        ..Default::default()
    })
}
