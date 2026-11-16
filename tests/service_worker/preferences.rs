use crate::fixture::{Fixture, Library, WAIT, synced};
use sonora::{
    model::{GainMode, Preferences, RepeatMode},
    playback::PlayerCommand,
    service::Command,
};
use std::time::Instant;

#[test]
fn preferences_save_through_the_worker_and_restore_after_clean_restart() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    let (_, initial) = worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    let preferences = Preferences {
        volume: 0.23,
        gain: GainMode::Album,
        shuffle: true,
        repeat: RepeatMode::All,
        dark: Some(true),
        all_missing_releases: true,
        ..initial
    };
    let expected = serde_json::to_value(&preferences).unwrap();
    worker.service().send(Command::Preferences(preferences));
    let deadline = Instant::now() + WAIT;
    loop {
        let command = worker.player_commands().recv_deadline(deadline).unwrap();
        if let PlayerCommand::Preferences(actual) = command
            && serde_json::to_value(actual).unwrap() == expected
        {
            break;
        }
    }
    assert_eq!(
        serde_json::to_value(
            fixture
                .catalog()
                .state::<Preferences>("preferences")
                .unwrap()
        )
        .unwrap(),
        expected
    );
    worker.shutdown(WAIT).unwrap();
    drop(worker);
    let mut restarted = fixture.start();
    let (account, restored) = restarted.wait_ready(WAIT).unwrap();
    assert_eq!(account.unwrap().namespace(), fixture.account.namespace());
    assert_eq!(serde_json::to_value(restored).unwrap(), expected);
    let PlayerCommand::Preferences(restored) =
        restarted.player_commands().recv_timeout(WAIT).unwrap()
    else {
        panic!("preferences must reach the player first")
    };
    assert_eq!(serde_json::to_value(restored).unwrap(), expected);
    synced(&restarted);
    restarted.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[test]
fn window_frame_saves_outside_the_catalog_and_survives_worker_restart() {
    let fixture = Fixture::new(Library::default());
    let frame = sonora::platform::WindowFrame {
        x: 90.,
        y: 70.,
        width: 1100.,
        height: 720.,
        maximized: false,
        fullscreen: false,
    };
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    synced(&worker);
    worker.service().send(Command::SaveWindow(frame));
    worker.service().send(Command::Initialize);
    crate::fixture::event(&worker, |event| {
        matches!(event, sonora::service::Event::Ready(..))
    });
    assert_eq!(fixture.paths.window_frame(), Some(frame));
    worker.shutdown(WAIT).unwrap();
    let mut restarted = fixture.start();
    restarted.wait_ready(WAIT).unwrap();
    synced(&restarted);
    assert_eq!(fixture.paths.window_frame(), Some(frame));
    restarted.shutdown(WAIT).unwrap();
    fixture.finish();
}
