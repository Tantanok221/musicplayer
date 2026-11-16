use super::Fixture;
use sonora::playback::Placement;

#[derive(Debug, PartialEq)]
pub(crate) enum PlaybackIntent {
    PlayAlbum(String),
    EnqueueAlbum(String, Placement),
    PlayIds(Vec<String>, usize),
}

impl Fixture {
    pub(crate) fn capture_playback(&mut self) {
        self.playback_intents = Some(vec![]);
    }

    pub(crate) fn playback_intents(&self) -> &[PlaybackIntent] {
        self.playback_intents.as_deref().unwrap_or_default()
    }

    pub(super) fn capturing_playback(&self) -> bool {
        self.playback_intents.is_some()
    }

    pub(super) fn record_playback(&mut self, intent: PlaybackIntent) {
        self.trace.push(match &intent {
            PlaybackIntent::PlayAlbum(id) => format!("PlayAlbum({id})"),
            PlaybackIntent::EnqueueAlbum(id, placement) => {
                format!("EnqueueAlbum({id}, {placement:?})")
            }
            PlaybackIntent::PlayIds(ids, at) => format!("PlayIds({ids:?}, {at})"),
        });
        self.playback_intents.as_mut().unwrap().push(intent);
    }
}
