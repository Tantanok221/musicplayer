use crate::{
    model::{Album, Track},
    service::Mutation,
};
use std::collections::HashSet;

#[derive(Debug, Default)]
pub struct Feedback {
    latest: Option<Mutation>,
    albums_in_flight: HashSet<String>,
}

impl Feedback {
    pub fn begin(&mut self, mutation: &Mutation) -> bool {
        if let Some(id) = mutation.album_feedback_id()
            && !self.albums_in_flight.insert(id.into())
        {
            return false;
        }
        self.latest = Some(mutation.clone());
        true
    }

    pub fn finished(&mut self, mutation: &Mutation) -> bool {
        self.release(mutation);
        if self.latest.as_ref() == Some(mutation) {
            self.latest = None;
            true
        } else {
            false
        }
    }

    pub fn failed(&mut self, mutation: Mutation) {
        self.release(&mutation);
        self.latest = Some(mutation);
    }

    pub fn retryable(&self) -> Option<&Mutation> {
        self.latest.as_ref()
    }

    pub fn dismiss(&mut self) {
        self.latest = None;
    }

    pub fn release_albums(&mut self) {
        self.albums_in_flight.clear();
    }

    pub fn album_busy(&self, id: &str) -> bool {
        self.albums_in_flight.contains(id)
    }

    fn release(&mut self, mutation: &Mutation) {
        if let Some(id) = mutation.album_feedback_id() {
            self.albums_in_flight.remove(id);
        }
    }
}

pub fn optimistic_album(album: &Album, mutation: &Mutation, now: &str) -> Option<Album> {
    let mut edited = album.clone();
    match mutation {
        Mutation::AlbumFavorite(_, favorite) => edited.starred = favorite.then(|| now.into()),
        Mutation::AlbumRating(_, rating) => edited.user_rating = Some(*rating),
        _ => return None,
    }
    Some(edited)
}

pub fn optimistic_track(track: &Track, mutation: &Mutation, now: &str) -> Option<Track> {
    let mut edited = track.clone();
    match mutation {
        Mutation::Favorite(_, favorite) => edited.starred = favorite.then(|| now.into()),
        Mutation::Rating(_, rating) => edited.user_rating = Some(*rating),
        _ => return None,
    }
    Some(edited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{Album, Track},
        service::Mutation,
    };

    const NOW: &str = "2026-11-16T00:00:00Z";

    #[test]
    fn a_second_change_to_an_album_in_flight_is_refused_until_it_settles() {
        let mut feedback = Feedback::default();
        let favorite = Mutation::AlbumFavorite("a".into(), true);
        assert!(feedback.begin(&favorite));
        assert!(feedback.album_busy("a"));
        assert!(!feedback.begin(&Mutation::AlbumRating("a".into(), 3)));
        assert!(feedback.begin(&Mutation::AlbumRating("b".into(), 3)));
        assert!(!feedback.finished(&favorite));
        assert!(!feedback.album_busy("a"));
        assert!(feedback.begin(&Mutation::AlbumRating("a".into(), 3)));
    }

    #[test]
    fn a_failed_change_is_kept_for_retry_until_it_succeeds_or_is_dismissed() {
        let mut feedback = Feedback::default();
        let rating = Mutation::Rating("song".into(), 4);
        assert!(feedback.begin(&rating));
        feedback.failed(rating.clone());
        assert_eq!(feedback.retryable(), Some(&rating));
        assert!(feedback.begin(&rating));
        assert!(!feedback.finished(&Mutation::Rating("other".into(), 1)));
        assert_eq!(feedback.retryable(), Some(&rating));
        assert!(feedback.finished(&rating));
        assert_eq!(feedback.retryable(), None);

        let favorite = Mutation::AlbumFavorite("a".into(), true);
        assert!(feedback.begin(&favorite));
        feedback.failed(favorite.clone());
        assert!(!feedback.album_busy("a"));
        feedback.dismiss();
        assert_eq!(feedback.retryable(), None);
    }

    #[test]
    fn reloading_preferences_releases_albums_but_keeps_the_retryable_change() {
        let mut feedback = Feedback::default();
        let favorite = Mutation::AlbumFavorite("a".into(), true);
        assert!(feedback.begin(&favorite));
        feedback.release_albums();
        assert!(!feedback.album_busy("a"));
        assert_eq!(feedback.retryable(), Some(&favorite));
    }

    #[test]
    fn optimistic_edits_change_only_the_targeted_field() {
        let album = Album {
            id: "a".into(),
            user_rating: Some(2),
            ..Default::default()
        };
        let favorited =
            optimistic_album(&album, &Mutation::AlbumFavorite("a".into(), true), NOW).unwrap();
        assert_eq!(favorited.starred.as_deref(), Some(NOW));
        assert_eq!(favorited.user_rating, Some(2));
        let cleared = optimistic_album(&album, &Mutation::AlbumRating("a".into(), 0), NOW).unwrap();
        assert_eq!(cleared.user_rating, Some(0));
        assert!(cleared.starred.is_none());
        assert!(optimistic_album(&album, &Mutation::Rating("a".into(), 1), NOW).is_none());

        let track = Track {
            id: "t".into(),
            starred: Some(NOW.into()),
            user_rating: Some(5),
            ..Default::default()
        };
        let unstarred =
            optimistic_track(&track, &Mutation::Favorite("t".into(), false), NOW).unwrap();
        assert!(unstarred.starred.is_none());
        assert_eq!(unstarred.user_rating, Some(5));
        let rated = optimistic_track(&track, &Mutation::Rating("t".into(), 1), NOW).unwrap();
        assert_eq!(rated.user_rating, Some(1));
        assert_eq!(rated.starred.as_deref(), Some(NOW));
        assert!(
            optimistic_track(&track, &Mutation::AlbumFavorite("t".into(), true), NOW).is_none()
        );
    }
}
