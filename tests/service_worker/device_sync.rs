use super::fixture::{Fixture, Library, Request, WAIT, event};
use axum::{
    Router,
    extract::{WebSocketUpgrade, ws::Message},
    routing::get,
};
use sonora::{
    model::{PlaybackSnapshot, Preferences, SavedQueue},
    playback::PlayerCommand,
    service::{Command, Event},
};
use sonora_sync_protocol::{ClientMessage, Device, PlaybackCommand, PlaybackState, ServerMessage};
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_settings_rebase_local_changes_without_uploading_device_preferences() {
    let fixture = Fixture::new(Library::default());
    let (writes, written) = flume::unbounded();
    let relay = Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let writes = writes.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    assert!(matches!(socket.recv().await, Some(Ok(Message::Text(_)))));
                    let send = |msg| Message::Text(serde_json::to_string(&msg).unwrap().into());
                    socket
                        .send(send(ServerMessage::Welcome {
                            devices: vec![],
                            active_device_id: None,
                            state: None,
                            settings: serde_json::json!({"dark": true, "gain": "Track"}),
                            settings_version: 5,
                        }))
                        .await
                        .unwrap();
                    while let Some(Ok(Message::Text(text))) = socket.recv().await {
                        if let Ok(ClientMessage::SettingsPut {
                            settings,
                            base_version,
                        }) = serde_json::from_str(&text)
                        {
                            writes.send((settings.clone(), base_version)).unwrap();
                            let answer = if base_version == 5 {
                                ServerMessage::SettingsConflict {
                                    settings: serde_json::json!({"dark": true, "gain": "Album"}),
                                    version: 6,
                                }
                            } else {
                                ServerMessage::Settings {
                                    settings,
                                    version: base_version + 1,
                                }
                            };
                            socket.send(send(answer)).await.unwrap();
                        }
                    }
                })
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, relay).await.unwrap() });
    fixture
        .catalog()
        .set_state(
            "preferences",
            &Preferences {
                lastfm_user: "local history".into(),
                volume: 0.35,
                sync_server_url: url.clone(),
                ..Default::default()
            },
        )
        .unwrap();
    let mut worker = fixture.start();
    event(
        &worker,
        |event| matches!(event, Event::DeviceSync(status) if status.connected),
    );
    let mut prefs: Preferences = fixture.catalog().state("preferences").unwrap();
    assert_eq!(prefs.dark, Some(true));
    assert_eq!(prefs.gain, sonora::model::GainMode::Track);
    assert_eq!(prefs.lastfm_user, "local history");
    assert_eq!(prefs.volume, 0.35);
    prefs.dark = Some(false);
    prefs.lastfm_user = "new local history".into();
    worker.service().send(Command::Preferences(prefs));
    assert_eq!(
        written.recv_timeout(WAIT).unwrap(),
        (serde_json::json!({"dark": false, "gain": "Track"}), 5)
    );
    assert_eq!(
        written.recv_timeout(WAIT).unwrap(),
        (serde_json::json!({"dark": false, "gain": "Album"}), 6)
    );
    let prefs: Preferences = fixture.catalog().state("preferences").unwrap();
    assert_eq!(prefs.dark, Some(false));
    assert_eq!(prefs.gain, sonora::model::GainMode::Album);
    assert_eq!(prefs.lastfm_user, "new local history");
    assert_eq!(prefs.sync_server_url, url);
    worker.shutdown(WAIT).unwrap();
    task.abort();
    fixture.finish();
}

#[test]
fn navidrome_queue_preserves_duplicate_occurrences_and_falls_back_without_extension() {
    for indexed in [true, false] {
        let fixture = Fixture::new(Library {
            indexed_play_queue: indexed.then(|| {
                serde_json::json!({
                    "changedBy": "Kitchen",
                    "changed": "2025-01-01T01:00:00Z",
                    "currentIndex": 1,
                    "position": 1500,
                    "entry": [{"id": "other-song"}, {"id": "song"}]
                })
            }),
            ..Library::default()
        });
        fixture.seed();
        let mut worker = fixture.start();
        let Event::Connected(account, _) =
            event(&worker, |event| matches!(event, Event::Connected(..)))
        else {
            unreachable!();
        };
        if indexed {
            fixture.wait_request(&Request::new("getPlayQueueByIndex.view", &[]));
            assert!(matches!(
                event(&worker, |event| matches!(event, Event::ResumeQueue(Some(_)))),
                Event::ResumeQueue(Some(device)) if device == "Kitchen"
            ));
            worker.service().send(Command::ResumeServerQueue);
            let deadline = std::time::Instant::now() + WAIT;
            loop {
                if let PlayerCommand::Restore(tracks, 1, 1.5, None) =
                    worker.player_commands().recv_deadline(deadline).unwrap()
                {
                    assert!(
                        tracks
                            .iter()
                            .map(|track| track.id.as_str())
                            .eq(["other-song", "song"])
                    );
                    break;
                }
            }
            assert!(matches!(
                worker.player_commands().recv_deadline(deadline).unwrap(),
                PlayerCommand::Play
            ));
        }
        worker.service().send(Command::Persist(
            account.namespace(),
            SavedQueue {
                ids: vec!["song".into(), "song".into()],
                index: Some(1),
                position: 1.25,
                ..Default::default()
            },
        ));
        let (endpoint, current) = if indexed {
            ("savePlayQueueByIndex.view", ("currentIndex", "1"))
        } else {
            ("savePlayQueue.view", ("current", "song"))
        };
        fixture.wait_request(&Request::new(
            endpoint,
            &[
                ("queueId", "song"),
                ("queueId", "song"),
                current,
                ("position", "1250"),
            ],
        ));
        worker.shutdown(WAIT).unwrap();
        fixture.finish();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_worker_uses_saved_login_and_bridges_relay_controls() {
    let fixture = Fixture::new(Library::default());
    fixture.seed();
    let (out, received) = flume::unbounded();
    let (acks, acknowledgements) = flume::unbounded();
    let relay = Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let out = out.clone();
            let acks = acks.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    let Some(Ok(Message::Text(text))) = socket.recv().await else {
                        return;
                    };
                    let Ok(ClientMessage::Hello(hello)) = serde_json::from_str(&text) else {
                        return;
                    };
                    assert_eq!(hello.username, "fixture");
                    let expected = format!(
                        "{:x}",
                        md5::compute(format!("synthetic-password{}", hello.subsonic_salt))
                    );
                    assert!(
                        hello.subsonic_token == expected,
                        "relay uses the saved Subsonic login"
                    );
                    let remote = Device {
                        id: "speaker".into(),
                        name: "Speaker".into(),
                        os: "linux".into(),
                        online: true,
                        last_seen: 1,
                    };
                    let send = |msg| Message::Text(serde_json::to_string(&msg).unwrap().into());
                    socket
                        .send(send(ServerMessage::Welcome {
                            devices: vec![remote],
                            active_device_id: Some("speaker".into()),
                            state: Some(PlaybackState {
                                playing: true,
                                seq: 8,
                                ..Default::default()
                            }),
                            settings: serde_json::json!({}),
                            settings_version: 0,
                        }))
                        .await
                        .unwrap();
                    while let Some(Ok(Message::Text(text))) = socket.recv().await {
                        match serde_json::from_str::<ClientMessage>(&text).unwrap() {
                            ClientMessage::Ping => {
                                socket.send(send(ServerMessage::Pong)).await.unwrap()
                            }
                            ClientMessage::Command {
                                target_device_id,
                                cmd,
                            } => {
                                out.send((target_device_id, cmd)).unwrap();
                                socket
                                    .send(send(ServerMessage::Command {
                                        cmd: PlaybackCommand::Pause,
                                        from_device_id: "speaker".into(),
                                    }))
                                    .await
                                    .unwrap();
                                for (transfer_id, track_id) in
                                    [("missing", "not-in-catalog"), ("valid", "song")]
                                {
                                    socket
                                        .send(send(ServerMessage::TransferRequest {
                                            transfer_id: transfer_id.into(),
                                            state: PlaybackState {
                                                queue: vec![track_id.into(), track_id.into()],
                                                index: Some(1),
                                                position_ms: 1250,
                                                seq: 8,
                                                ..Default::default()
                                            },
                                            start_playing: true,
                                        }))
                                        .await
                                        .unwrap();
                                }
                            }
                            ClientMessage::TransferAck {
                                transfer_id, ok, ..
                            } => {
                                if transfer_id == "valid" {
                                    socket
                                        .send(send(ServerMessage::TransferResult {
                                            transfer_id: transfer_id.clone(),
                                            ok: false,
                                            error: Some("test rollback".into()),
                                        }))
                                        .await
                                        .unwrap();
                                }
                                acks.send((transfer_id, ok)).unwrap();
                            }
                            _ => {}
                        }
                    }
                })
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, relay).await.unwrap() });
    fixture
        .catalog()
        .set_state(
            "preferences",
            &Preferences {
                sync_server_url: url,
                ..Default::default()
            },
        )
        .unwrap();
    let mut worker = fixture.start();
    assert!(
        matches!(event(&worker, |event| matches!(event, Event::DeviceSync(status) if status.connected)),
        Event::DeviceSync(status) if status.active_device_id.as_deref() == Some("speaker")
            && status.playback.as_ref().is_some_and(|state| state.seq == 8))
    );
    worker
        .service()
        .send(Command::RemotePlayback(PlaybackCommand::Next));
    assert_eq!(
        received.recv_timeout(WAIT).unwrap(),
        ("speaker".into(), PlaybackCommand::Next)
    );
    let deadline = std::time::Instant::now() + WAIT;
    while !matches!(
        worker.player_commands().recv_deadline(deadline).unwrap(),
        PlayerCommand::Pause
    ) {}
    let next = || loop {
        let command = worker
            .player_commands()
            .recv_deadline(deadline)
            .expect("transfer must load the target queue");
        if !matches!(command, PlayerCommand::Server(_)) {
            break command;
        }
    };
    assert!(
        matches!(next(), PlayerCommand::Restore(tracks, 1, 1.25, None)
        if tracks.iter().map(|track| track.id.as_str()).eq(["song", "song"]))
    );
    assert!(matches!(next(), PlayerCommand::Play));
    let PlayerCommand::SaveQueue(reply) = next() else {
        panic!("target must verify the player prepared the queue before acknowledging");
    };
    reply
        .send(SavedQueue {
            ids: vec!["song".into(), "song".into()],
            index: Some(1),
            position: 1.25,
            ..Default::default()
        })
        .unwrap();
    let tracks = fixture
        .catalog()
        .by_ids(&["song".into(), "song".into()])
        .unwrap();
    worker.publish_playback(PlaybackSnapshot {
        queue: tracks.into(),
        index: Some(1),
        playing: true,
        ..Default::default()
    });
    assert_eq!(
        acknowledgements.recv_timeout(WAIT).unwrap(),
        ("missing".into(), false)
    );
    assert_eq!(
        acknowledgements.recv_timeout(WAIT).unwrap(),
        ("valid".into(), true)
    );
    assert!(matches!(next(), PlayerCommand::Restore(tracks, 0, 0., None) if tracks.is_empty()));
    worker.shutdown(WAIT).unwrap();
    task.abort();
    fixture.finish();
}
