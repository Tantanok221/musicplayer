use crate::fixture::{Fixture, Library, WAIT, album, event, song, synced, view};
use sonora::service::{Command, Connection, Event};

#[test]
fn slskd_check_uses_saved_auth_even_when_download_offers_are_disabled() {
    use sonora::{model::Preferences, service::SettingsCredentials, slskd::ConnectionStatus};
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    let mut prefs = Preferences::default();
    prefs.downloads.url = fixture.account.url.clone();
    assert!(!prefs.downloads.enabled);
    let save = |key: &str| {
        assert_eq!(
            worker
                .service()
                .save_settings(
                    prefs.clone(),
                    SettingsCredentials::Slskd(key.into(), String::new(), String::new())
                )
                .recv_timeout(WAIT),
            Ok(true)
        );
    };
    save("synthetic-invalid-key");
    worker.service().send(Command::SlskdTest(7));
    event(&worker, |event| {
        matches!(
            event,
            Event::SlskdTested(7, ConnectionStatus::AuthenticationFailed(_))
        )
    });
    save("synthetic-slskd-key");
    worker.service().send(Command::SlskdTest(8));
    event(&worker, |event| {
        matches!(
            event,
            Event::SlskdTested(
                8,
                ConnectionStatus::Connected {
                    logged_in: Some(false),
                    ..
                }
            )
        )
    });
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn checking_the_saved_server_reports_each_result_and_updates_reachability() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    event(&worker, |e| matches!(e, Event::LibrarySyncing(true)));
    event(&worker, |e| matches!(e, Event::LibrarySyncing(false)));
    synced(&worker);

    worker.service().send(Command::TestSavedConnection(1));
    event(&worker, |e| {
        matches!(e, Event::SavedConnectionTested(1, Ok(())))
    });
    fixture.edit_library(|l| l.offline = true);
    worker.service().send(Command::TestSavedConnection(2));
    event(&worker, |e| {
        matches!(e, Event::Connection(Connection::Offline(_)))
    });
    event(&worker, |e| {
        matches!(e, Event::SavedConnectionTested(2, Err(_)))
    });
    assert_eq!(view(&worker).track_count, 2);
    worker.shutdown(WAIT).unwrap();
    fixture.finish();
}

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
