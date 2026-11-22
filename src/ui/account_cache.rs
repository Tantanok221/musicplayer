use crate::{
    artists::{ArtistProfile, TopSongs},
    backdrop::Backdrop,
    platform::Account,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

#[derive(Default)]
pub(super) struct AccountCache {
    pub(super) artwork: HashMap<String, PathBuf>,
    pub(super) backdrops: HashMap<String, Arc<Backdrop>>,
    pub(super) backdrop_requested: HashSet<String>,
    pub(super) artist_profiles: HashMap<String, Arc<ArtistProfile>>,
    pub(super) profile_requested: HashSet<String>,
    pub(super) top_songs: HashMap<String, Arc<TopSongs>>,
    pub(super) popularity_requested: HashSet<String>,
}

impl AccountCache {
    pub(super) fn reset_for_account(
        &mut self,
        previous: Option<&Account>,
        incoming: Option<&Account>,
    ) -> bool {
        let changed = previous.map(Account::namespace) != incoming.map(Account::namespace);
        if changed {
            *self = Self::default();
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backdrop::Tone;

    #[test]
    fn resets_account_caches_and_requests_only_when_the_namespace_changes() {
        let account = Account {
            url: "https://fixture.invalid".into(),
            username: "first".into(),
        };
        let other_account = Account {
            username: "second".into(),
            ..account.clone()
        };
        let mut cache = AccountCache {
            artwork: HashMap::from([("cover".into(), "cover.png".into())]),
            backdrops: HashMap::from([(
                "cover".into(),
                Arc::new(Backdrop {
                    path: "backdrop.png".into(),
                    palette: Default::default(),
                    tones: [Tone { low: 0., high: 0. }; 2],
                }),
            )]),
            backdrop_requested: HashSet::from(["cover".into()]),
            artist_profiles: HashMap::from([("artist".into(), Arc::default())]),
            profile_requested: HashSet::from(["artist".into()]),
            top_songs: HashMap::from([("artist".into(), Arc::default())]),
            popularity_requested: HashSet::from(["artist".into()]),
        };
        let cache_sizes = |cache: &AccountCache| {
            [
                cache.artwork.len(),
                cache.backdrops.len(),
                cache.backdrop_requested.len(),
                cache.artist_profiles.len(),
                cache.profile_requested.len(),
                cache.top_songs.len(),
                cache.popularity_requested.len(),
            ]
        };
        assert!(!cache.reset_for_account(Some(&account), Some(&account.clone())));
        assert_eq!(cache_sizes(&cache), [1; 7]);
        assert!(cache.reset_for_account(Some(&account), Some(&other_account)));
        assert_eq!(cache_sizes(&cache), [0; 7]);
    }
}
