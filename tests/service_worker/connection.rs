use crate::fixture::{Fixture, Library, WAIT, album, event, song, synced, view};
use sonora::service::{Command, Connection, Event};

#[test]
fn reconnect_checks_without_sleep_preserves_cache_and_resyncs() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    synced(&worker);
    fixture.edit_library(|l| l.offline = true);
    worker.service().send(Command::CheckConnection(true));
    event(&worker, |e| {
        matches!(e, Event::Connection(Connection::Offline(_)))
    });
    assert_eq!(view(&worker).track_count, 2);
    worker.service().send(Command::PlayTrack("song".into()));
    loop {
        if let sonora::playback::PlayerCommand::Replace(tracks, _) =
            worker.player_commands().recv_timeout(WAIT).unwrap()
        {
            assert_eq!(tracks[0].id, "song");
            break;
        }
    }
    fixture.edit_library(|l| {
        l.offline = false;
        l.tracks.push(song("new", "new"));
        l.albums.push(album("new"));
    });
    worker.service().send(Command::CheckConnection(true));
    event(&worker, |e| {
        matches!(e, Event::Connection(Connection::Online))
    });
    synced(&worker);
    assert_eq!(view(&worker).track_count, 3);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}
