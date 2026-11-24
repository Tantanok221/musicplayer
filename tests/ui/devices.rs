use crate::common::ui::start;
use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    playback::PlayerCommand,
    service::{Command, Event},
    sync::DeviceStatus,
};
use sonora_sync_protocol::{Device, PlaybackCommand, PlaybackState};
use sonora_sync_protocol::{QueueChange, queue_fingerprint};
use std::time::{SystemTime, UNIX_EPOCH};

fn kitchen_playing(controlled: Option<&str>) -> DeviceStatus {
    DeviceStatus {
        connected: true,
        local_device_id: "local".into(),
        active_device_id: Some("remote".into()),
        controlled: controlled.map(str::to_owned),
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
    }
}

#[gpui_kit::test]
fn devices_controls_another_device_only_after_it_is_chosen(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _) = start(cx);
    let _ = fixture.backend.player_commands().try_iter().count();

    // Another device playing doesn't take over this device's player.
    fixture
        .backend
        .emit(Event::DeviceSync(kitchen_playing(None)));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("play-pause").label(), Some("Play"));
    })
    .unwrap();
    fixture.click(window, "play-pause", cx);
    assert!(matches!(
        fixture.backend.player_commands().try_recv(),
        Ok(PlayerCommand::Toggle)
    ));

    fixture.click(window, "devices", cx);
    cx.update_window(window, |_, window, _| {
        let menu = window.within("popup-menu");
        assert!(menu.find_by_label("Kitchen · Playing").visible());
        assert!(menu.find_by_label("Laptop · This device").visible());
    })
    .unwrap();
    fixture.expect("control the playing device", |command, _| {
        assert!(matches!(command, Command::ControlDevice(Some(id)) if id == "remote"));
        vec![Event::DeviceSync(kitchen_playing(Some("remote")))]
    });
    fixture.menu_item(window, "Kitchen · Playing", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("popup-menu").is_none());
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

    fixture.expect("pause the controlled device", |command, _| {
        assert!(matches!(
            command,
            Command::RemotePlayback(PlaybackCommand::Pause)
        ));
        vec![]
    });
    fixture.click(window, "play-pause", cx);
    assert!(fixture.backend.player_commands().is_empty());

    fixture.click(window, "devices", cx);
    cx.update_window(window, |_, window, _| {
        let menu = window.within("popup-menu");
        assert!(
            menu.find_by_label("Kitchen · Playing · Controlling")
                .visible()
        );
    })
    .unwrap();
    fixture.expect("back to this device", |command, _| {
        assert!(matches!(command, Command::ControlDevice(None)));
        vec![Event::DeviceSync(kitchen_playing(None))]
    });
    fixture.menu_item(window, "Laptop · This device", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("play-pause").label(), Some("Play"));
    })
    .unwrap();
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn queue_panel_shows_and_edits_the_active_devices_queue(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _) = start(cx);
    let ids: Vec<String> = ["track-1", "track-2", "track-3"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let tracks = fixture.catalog.by_ids(&ids).unwrap();
    let _ = fixture.backend.player_commands().try_iter().count();
    fixture.backend.emit(Event::DeviceSync(DeviceStatus {
        connected: true,
        local_device_id: "local".into(),
        active_device_id: Some("remote".into()),
        controlled: Some("remote".into()),
        devices: vec![Device {
            id: "remote".into(),
            name: "Mac".into(),
            os: "macos".into(),
            online: true,
            last_seen: 1,
        }],
        playback: Some(PlaybackState {
            queue: ids.clone(),
            index: Some(0),
            playing: true,
            seq: 7,
            ..Default::default()
        }),
        track: tracks.first().cloned(),
        queue: tracks.clone().into(),
        ..Default::default()
    }));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        for (index, track) in tracks.iter().enumerate() {
            let row = window.find(format!("queue-entry-{index}").as_str().to_owned());
            assert_eq!(
                row.label(),
                Some(
                    format!(
                        "Queue {}: {} · {}",
                        index + 1,
                        track.title,
                        track.artist_label()
                    )
                    .as_str()
                ),
                "the queue panel shows the playing device's queue"
            );
        }
        assert_eq!(
            window.find("undo-queue-edit").disabled(),
            Some(true),
            "local undo history never applies to another device's queue"
        );
    })
    .unwrap();

    cx.update_window(window, |_, window, cx| {
        window.right_click("queue-entry-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    let fingerprint = queue_fingerprint(&ids);
    fixture.expect(
        "remove from the playing device's queue",
        move |command, _| {
            assert!(matches!(
                command,
                Command::RemotePlayback(PlaybackCommand::EditQueue {
                    queue,
                    change: QueueChange::Remove { indices },
                }) if *queue == fingerprint && *indices == [2]
            ));
            vec![]
        },
    );
    fixture.menu_item(window, "Remove from Queue", cx);
    fixture.assert_script_finished();
    assert!(
        fixture.backend.player_commands().is_empty(),
        "the local queue is not edited"
    );
}
