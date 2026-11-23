use crate::common::ui::start;
use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    playback::PlayerCommand,
    service::{Command, Event},
    sync::DeviceStatus,
};
use sonora_sync_protocol::{Device, PlaybackCommand, PlaybackState};
use std::time::{SystemTime, UNIX_EPOCH};

#[gpui_kit::test]
fn navidrome_resume_offer_plays_only_when_selected(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _) = start(cx);
    fixture
        .backend
        .emit(Event::ResumeQueue(Some("Kitchen".into())));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("resume-server-queue").label(),
            Some("Resume from Kitchen")
        );
    })
    .unwrap();
    fixture.expect("accept saved queue", |command, _| {
        assert!(matches!(command, Command::ResumeServerQueue));
        vec![Event::ResumeQueue(None)]
    });
    fixture.click(window, "resume-server-queue", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("resume-server-queue").is_none());
    })
    .unwrap();
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn devices_shows_remote_playback_and_controls_only_the_active_peer(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _) = start(cx);
    let _ = fixture.backend.player_commands().try_iter().count();
    fixture.backend.emit(Event::DeviceSync(DeviceStatus {
        connected: true,
        local_device_id: "local".into(),
        active_device_id: Some("remote".into()),
        devices: vec![
            Device {
                id: "local".into(),
                name: "Laptop".into(),
                os: "linux".into(),
                online: true,
                last_seen: 1,
            },
            Device {
                id: "remote".into(),
                name: "Kitchen".into(),
                os: "macos".into(),
                online: true,
                last_seen: 1,
            },
        ],
        playback: Some(PlaybackState {
            queue: vec!["remote-track".into()],
            index: Some(0),
            position_ms: 30_000,
            playing: true,
            volume: 80,
            seq: 5,
            captured_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64
                - 60_000,
            ..Default::default()
        }),
        ..Default::default()
    }));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("play-pause").label(), Some("Pause"));
        assert!(
            window
                .find("playback-elapsed")
                .label()
                .unwrap()
                .starts_with("1:")
        );
    })
    .unwrap();
    fixture.click(window, "devices", cx);
    cx.update_window(window, |_, window, _| {
        let menu = window.within("popup-menu");
        assert!(menu.find_by_label("Kitchen · Active").visible());
        assert!(menu.find_by_label("Laptop · This device").visible());
    })
    .unwrap();
    fixture.expect("transfer to this device", |command, _| {
        assert!(matches!(command, Command::TransferTo(id, true) if id == "local"));
        vec![]
    });
    fixture.menu_item(window, "Laptop · This device", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("popup-menu").is_none());
    })
    .unwrap();

    fixture.expect("pause active remote device", |command, _| {
        assert!(matches!(
            command,
            Command::RemotePlayback(PlaybackCommand::Pause)
        ));
        vec![]
    });
    fixture.click(window, "play-pause", cx);
    assert!(fixture.backend.player_commands().is_empty());
    fixture.assert_script_finished();

    fixture
        .backend
        .emit(Event::DeviceSync(DeviceStatus::default()));
    fixture.settle(window, cx);
    fixture.click(window, "play-pause", cx);
    assert!(matches!(
        fixture.backend.player_commands().try_recv(),
        Ok(PlayerCommand::Toggle)
    ));
}
