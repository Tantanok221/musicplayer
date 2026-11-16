//! Shelves of owned music that has fallen out of rotation.
use crate::model::Album;
use chrono::{DateTime, Utc};
use std::sync::Arc;

pub const WINDOW_DAYS: i64 = 182;
pub const SHELF_LIMIT: usize = 24;
pub const MIN_ROTATION_PLAYS: u64 = 5;

/// An album with how many of its songs are favorited or rated four stars or more.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub album: Arc<Album>,
    pub loved_songs: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Rediscover {
    pub old_rotation: Vec<Arc<Album>>,
    pub forgotten_favorites: Vec<Arc<Album>>,
}

fn played(album: &Album) -> Option<DateTime<Utc>> {
    let played = album.played.as_deref()?;
    DateTime::parse_from_rfc3339(played)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn plays(album: &Album) -> u64 {
    album.play_count.unwrap_or(0)
}

fn favorite(album: &Album) -> bool {
    album.starred.is_some() || album.user_rating.unwrap_or(0) >= 4
}

/// Old rotation: the most played tenth of the library, not played in the window.
/// Forgotten favorites: favorite albums or albums with two loved songs, not
/// played in the window and not already in old rotation.
pub fn rediscover(candidates: &[Candidate], now: DateTime<Utc>) -> Rediscover {
    let cutoff = now - chrono::Duration::days(WINDOW_DAYS);
    let resting = |album: &Album| played(album).is_none_or(|at| at < cutoff);
    let mut counts = candidates
        .iter()
        .map(|candidate| plays(&candidate.album))
        .filter(|plays| *plays > 0)
        .collect::<Vec<_>>();
    counts.sort_unstable_by(|a, b| b.cmp(a));
    let threshold = counts
        .get(counts.len().div_ceil(10).saturating_sub(1))
        .copied()
        .unwrap_or(u64::MAX)
        .max(MIN_ROTATION_PLAYS);
    let mut old_rotation = candidates
        .iter()
        .map(|candidate| &candidate.album)
        .filter(|album| plays(album) >= threshold && played(album).is_some() && resting(album))
        .cloned()
        .collect::<Vec<_>>();
    old_rotation.sort_by(|a, b| plays(b).cmp(&plays(a)).then_with(|| a.id.cmp(&b.id)));
    old_rotation.truncate(SHELF_LIMIT);
    let mut forgotten = candidates
        .iter()
        .filter(|candidate| {
            (favorite(&candidate.album) || candidate.loved_songs >= 2)
                && resting(&candidate.album)
                && !old_rotation
                    .iter()
                    .any(|album| album.id == candidate.album.id)
        })
        .collect::<Vec<_>>();
    forgotten.sort_by(|a, b| {
        favorite(&b.album)
            .cmp(&favorite(&a.album))
            .then_with(|| b.loved_songs.cmp(&a.loved_songs))
            .then_with(|| played(&a.album).cmp(&played(&b.album)))
            .then_with(|| a.album.id.cmp(&b.album.id))
    });
    Rediscover {
        old_rotation,
        forgotten_favorites: forgotten
            .into_iter()
            .take(SHELF_LIMIT)
            .map(|candidate| candidate.album.clone())
            .collect(),
    }
}
