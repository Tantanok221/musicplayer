use crate::fixture::{Fixture, Library, Request, WAIT, event, synced};
use sonora::{
    model::Playlist,
    service::{Command, Event, Mutation},
};
use std::sync::Arc;

#[test]
fn removal_uses_entry_index_and_refreshes_persisted_playlist_only_on_success() {
    let mut library = Library::default();
    let first = Arc::new(library.tracks[0].clone());
    library.playlists = vec![Playlist {
        id: "playlist".into(),
        name: "Roadtrip".into(),
        song_count: 3,
        entry: Arc::new(vec![
            first.clone(),
            Arc::new(library.tracks[1].clone()),
            first,
        ]),
        ..Default::default()
    }];
    let fixture = Fixture::new(library);
    fixture.seed();
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::Playlist("playlist".into()));
    event(
        &worker,
        |event| matches!(event, Event::Playlist(playlist) if playlist.entry.len() == 3),
    );
    let request = Request::new(
        "updatePlaylist.view",
        &[("playlistId", "playlist"), ("songIndexToRemove", "2")],
    );
    let mutation = Mutation::RemoveFromPlaylist("playlist".into(), 2);
    fixture.expect_mutation(request, false);
    worker.service().send(Command::Mutate(mutation.clone()));
    event(
        &worker,
        |event| matches!(event, Event::MutationFailed(failed, _) if failed == &mutation),
    );
    assert_eq!(
        fixture
            .catalog()
            .playlist("playlist")
            .unwrap()
            .unwrap()
            .entry
            .len(),
        3
    );
    for index in (0..3).rev() {
        let value = index.to_string();
        fixture.expect_mutation(
            Request::new(
                "updatePlaylist.view",
                &[("playlistId", "playlist"), ("songIndexToRemove", &value)],
            ),
            true,
        );
        worker
            .service()
            .send(Command::Mutate(Mutation::RemoveFromPlaylist(
                "playlist".into(),
                index,
            )));
        event(
            &worker,
            |event| matches!(event, Event::Playlist(playlist) if playlist.entry.len() == index),
        );
        let saved = fixture.catalog().playlist("playlist").unwrap().unwrap();
        assert_eq!(saved.entry.len(), index);
        assert_eq!(saved.song_count as usize, index);
        if index == 2 {
            assert_eq!(
                saved
                    .entry
                    .iter()
                    .map(|track| track.id.as_str())
                    .collect::<Vec<_>>(),
                ["song", "other-song"]
            );
        }
    }
    worker.shutdown(WAIT).unwrap();
    assert!(
        fixture
            .catalog()
            .playlist("playlist")
            .unwrap()
            .unwrap()
            .entry
            .is_empty()
    );
    fixture.finish();
}
