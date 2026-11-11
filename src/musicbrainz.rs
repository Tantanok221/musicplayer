//! Read-only MusicBrainz and ListenBrainz lookups for artist discographies.
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
    listens: Url,
    /// MusicBrainz blocks clients that exceed one request per second.
    interval: Duration,
    last: Arc<Mutex<Option<Instant>>>,
}

impl MusicBrainz {
    pub fn new() -> Result<Self> {
        Self::with_endpoints(
            "https://musicbrainz.org/ws/2/",
            "https://api.listenbrainz.org/1/",
            Duration::from_millis(1100),
        )
    }
    pub fn with_endpoints(base: &str, listens: &str, interval: Duration) -> Result<Self> {
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
            listens: Url::parse(listens)?,
            interval,
            last: Arc::default(),
        })
    }

    async fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Value> {
        let mut url = self.base.join(path)?;
        url.query_pairs_mut()
            .extend_pairs(params)
            .append_pair("fmt", "json");
        for attempt in 0..2 {
            let response = {
                // Held across the request so calls are spaced, not merely started, a second apart.
                let mut last = self.last.lock().await;
                if let Some(at) = *last {
                    tokio::time::sleep_until(at + self.interval).await;
                }
                let response = self.http.get(url.clone()).send().await;
                *last = Some(Instant::now());
                response.map_err(|error| {
                    anyhow::Error::new(error.without_url()).context("Cannot reach MusicBrainz")
                })?
            };
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
    pub async fn discography(&self, artist: &str) -> Result<Discography> {
        let mut groups: Vec<ReleaseGroup> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut offset = 0;
        let mut total = usize::MAX;
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
        // Popularity only orders the list, so its absence is not an error.
        if let Ok(listens) = self.listens(artist).await {
            for group in &mut groups {
                group.listens = listens.get(&group.id).copied().unwrap_or(0);
            }
        }
        Ok(Discography {
            artist: artist.into(),
            groups,
            fetched_ms: chrono::Utc::now().timestamp_millis(),
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

    async fn listens(&self, artist: &str) -> Result<HashMap<String, u64>> {
        let url = self.listens.join(&format!(
            "popularity/top-release-groups-for-artist/{artist}"
        ))?;
        let value: Value = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|error| anyhow::Error::new(error.without_url()))?
            .error_for_status()
            .map_err(|error| anyhow::Error::new(error.without_url()))?
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("ListenBrainz returned an invalid response"))?;
        Ok(value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                Some((
                    item["release_group_mbid"].as_str()?.to_string(),
                    item["total_listen_count"].as_u64()?,
                ))
            })
            .collect())
    }

    /// Finds an artist the server has no MusicBrainz ID for. Namesakes are
    /// common, so a candidate counts only if it alone shares a release title
    /// with the library. `titles` are folded library album titles.
    pub async fn resolve_artist(
        &self,
        name: &str,
        titles: &HashSet<String>,
    ) -> Result<Option<String>> {
        if titles.is_empty() {
            return Ok(None);
        }
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
