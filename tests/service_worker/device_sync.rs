use super::fixture::{Fixture, Library, Request, WAIT, event, synced};
use axum::{
    Router,
    extract::{WebSocketUpgrade, ws::Message},
    routing::get,
};
use sonora::{
    model::{PlaybackSnapshot, Preferences, SavedQueue},
    playback::PlayerCommand,
    service::{Command, Event, SettingsCredentials},
};
use sonora_sync_protocol::{ClientMessage, Device, PlaybackCommand, PlaybackState, ServerMessage};
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_lan_trust_accepts_known_keys_offline_and_revokes_them() {
    use ed25519_dalek::SigningKey;
    use sonora::{
        platform::{Paths, ServerCredentials},
        server::Server,
        service::test_support::Worker,
        sync::{
            lan::user_hash,
            lan_security::{Link, Trust},
        },
    };

    let fixture = Fixture::new(Library {
        device_playlists: Some(vec![]),
        ..Default::default()
    });
    let first = "00000000000000000000000000000001";
    let second = "00000000000000000000000000000002";
    let second_paths = Paths {
        data: fixture.paths.data.join("speaker"),
        cache: fixture.paths.cache.join("speaker"),
    };
    for (paths, id, key) in [
        (&fixture.paths, first, [1u8; 32]),
        (&second_paths, second, [2u8; 32]),
    ] {
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::create_dir_all(&paths.cache).unwrap();
        paths.save_account(&fixture.account).unwrap();
        paths
            .save_credentials(
                &fixture.account,
                &ServerCredentials::from_password("synthetic-password").unwrap(),
            )
            .unwrap();
        std::fs::write(paths.data.join("sync-device-id"), id).unwrap();
        std::fs::write(paths.data.join("sync-signing-key"), key).unwrap();
    }

    let start = |paths: Paths| {
        let worker = fixture.start_on(paths);
        worker.wait_ready(WAIT).unwrap();
        worker
    };
    let trusted_port = |worker: &Worker, id: &str| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
        loop {
            if let Event::LanStatus(status) = worker
                .events()
                .recv_deadline(deadline)
                .unwrap_or_else(|_| panic!("automatic trust did not converge for {id}"))
                && status.devices.iter().any(|device| device.id == id)
            {
                break status.port;
            }
        }
    };
    let mut left = start(fixture.paths.clone());
    let mut right = start(second_paths);
    let port = trusted_port(&left, second);
    trusted_port(&right, first);
    let group = user_hash(&fixture.account);
    let left_key = [SigningKey::from_bytes(&[1; 32]).verifying_key().to_bytes()];
    let dial = |port, identity| async move {
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let (socket, _) = tokio_tungstenite::client_async(format!("ws://127.0.0.1:{port}/ws"), tcp)
            .await
            .unwrap();
        Link::establish(socket, true, &group, &identity, Trust::Paired(&left_key)).await
    };
    let assert_trusted = |port| async move {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Ok((mut link, _)) = dial(port, [2; 32]).await
                    && let Ok(body) = link.recv().await
                    && serde_json::from_slice::<serde_json::Value>(&body)
                        .is_ok_and(|hello| hello["type"] == "Hello")
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("trusted worker accepts authenticated loopback client");
    };
    assert_trusted(port).await;
    assert!(
        dial(port, [3; 32]).await.is_err(),
        "unknown key authenticated"
    );
    let await_roster = async |second_revoked: bool| {
        tokio::time::timeout(WAIT, async {
            loop {
                let mut matches = false;
                fixture.edit_library(|library| {
                    let lists = library.device_playlists.as_ref().unwrap();
                    let [list] = lists.as_slice() else {
                        return;
                    };
                    if list.public || !list.comment.starts_with("Sonora's device list.") {
                        return;
                    }
                    let (_, payload) = list.comment.split_once("Sonora devices v1:").unwrap();
                    let roster: serde_json::Value = serde_json::from_str(payload).unwrap();
                    matches = roster["devices"][first].is_object()
                        && if second_revoked {
                            roster["devices"][second].is_null()
                                && roster["revoked"] == serde_json::json!([second])
                        } else {
                            roster["devices"][second].is_object()
                        };
                });
                if matches {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("concurrent device publishes converge and revocations are retained");
    };
    await_roster(false).await;
    let server = Server::new(fixture.account.clone(), "synthetic-password".into()).unwrap();
    assert!(
        server.playlists().await.unwrap().is_empty(),
        "the roster is hidden from Sonora playlists"
    );
    right.shutdown(WAIT).unwrap();
    left.shutdown(WAIT).unwrap();

    fixture.edit_library(|library| library.offline = true);
    let mut left = start(fixture.paths.clone());
    let port = trusted_port(&left, second);
    assert_trusted(port).await;
    left.service().send(Command::LanRevoke(second.into()));
    event(
        &left,
        |event| matches!(event, Event::LanStatus(status) if status.devices.is_empty()),
    );
    left.shutdown(WAIT).unwrap();

    fixture.edit_library(|library| library.offline = false);
    let mut left = start(fixture.paths.clone());
    await_roster(true).await;
    left.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_publishes_above_the_last_sequence_even_after_relay_rollback() {
    use sonora::sync::PlaybackObservation;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let fixture = Fixture::new(Library::default());
    let (sequences, published) = flume::unbounded();
    let (disconnect, closed) = flume::bounded(1);
    let connections = Arc::new(AtomicUsize::new(0));
    let relay = Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let attempt = connections.fetch_add(1, Ordering::SeqCst);
            let closed = closed.clone();
            let sequences = sequences.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    let Some(Ok(Message::Text(text))) = socket.recv().await else {
                        return;
                    };
                    let ClientMessage::Hello(hello) = serde_json::from_str(&text).unwrap() else {
                        panic!("relay hello after reconnect");
                    };
                    let active = if attempt == 0 {
                        "speaker".into()
                    } else {
                        hello.device_id
                    };
                    let welcome = ServerMessage::Welcome {
                        devices: vec![],
                        active_device_id: Some(active),
                        state: Some(PlaybackState {
                            seq: if attempt == 0 { 100 } else { 5 },
                            ..Default::default()
                        }),
                        settings: serde_json::json!({}),
                        settings_version: 0,
                    };
                    socket
                        .send(Message::Text(
                            serde_json::to_string(&welcome).unwrap().into(),
                        ))
                        .await
                        .unwrap();
                    if attempt == 0 {
                        closed.recv_async().await.unwrap();
                        socket.send(Message::Close(None)).await.unwrap();
                        return;
                    }
                    while let Some(Ok(Message::Text(text))) = socket.recv().await {
                        if let Ok(ClientMessage::StateUpdate { state }) =
                            serde_json::from_str(&text)
                        {
                            sequences.send(state.seq).unwrap();
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
    let (account, _) = worker.wait_ready(WAIT).unwrap();
    event(
        &worker,
        |event| matches!(event, Event::DeviceSync(status) if status.other_active() == Some("speaker")),
    );
    disconnect.send(()).unwrap();
    event(
        &worker,
        |event| matches!(event, Event::DeviceSync(status) if status.enabled && !status.connected),
    );
    event(
        &worker,
        |event| matches!(event, Event::DeviceSync(status) if status.connected && status.remote_id().is_none()),
    );
    worker.publish_playback(PlaybackSnapshot {
        sync: Some(Arc::new(PlaybackObservation {
            namespace: account.unwrap().namespace(),
            revision: 1,
            volume: 50,
            shuffle: false,
            repeat: Default::default(),
        })),
        ..Default::default()
    });
    assert!(
        published.recv_timeout(WAIT).unwrap() > 100,
        "reconnect discarded the last sequence"
    );
    worker.shutdown(WAIT).unwrap();
    task.abort();
    fixture.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_settings_rebase_local_changes_without_uploading_device_preferences() {
    shared_settings_rebase(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledged_settings_saves_sync_only_shared_preferences() {
    shared_settings_rebase(true).await;
}

async fn shared_settings_rebase(acknowledged: bool) {
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
    if acknowledged {
        assert!(
            worker
                .service()
                .save_settings(prefs, SettingsCredentials::Unchanged)
                .recv_timeout(WAIT)
                .unwrap()
        );
    } else {
        worker.service().send(Command::Preferences(prefs));
    }
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
fn navidrome_queue_is_saved_but_never_offered_for_resume() {
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
        let Event::Connected(account, ..) =
            event(&worker, |event| matches!(event, Event::Connected(..)))
        else {
            unreachable!();
        };
        // Another device's saved queue is never offered or restored on connect.
        synced(&worker);
        assert!(
            !fixture
                .requests()
                .iter()
                .any(|request| request.endpoint.starts_with("getPlayQueue")),
            "the saved server queue is not fetched"
        );
        assert!(
            worker
                .player_commands()
                .try_iter()
                .all(|command| !matches!(command, PlayerCommand::Restore(..))),
            "the local queue is untouched"
        );
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
            && status.remote_id().is_none()
            && status.playback.as_ref().is_some_and(|state| state.seq == 8))
    );
    worker
        .service()
        .send(Command::ControlDevice(Some("speaker".into())));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn navidrome_devices_connect_through_published_lan_addresses() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::SigningKey;
    use sonora::{
        model::Playlist,
        platform::{Paths, ServerCredentials},
        service::test_support::Worker,
        sync::lan::{LanReach, LanStatus},
    };

    let first = "00000000000000000000000000000001";
    let second = "00000000000000000000000000000002";
    let absent = "00000000000000000000000000000003";
    // A device another Sonora published without any LAN address (for example an older version).
    let absent_key =
        URL_SAFE_NO_PAD.encode(SigningKey::from_bytes(&[9; 32]).verifying_key().as_bytes());
    let fixture = Fixture::new(Library {
        device_playlists: Some(vec![Playlist {
            id: "device-list-0".into(),
            name: "Sonora devices (sync keys — do not edit)".into(),
            comment: format!(
                "Sonora's device list. This private playlist stores Sonora device-sync keys. Do not edit.\nSonora devices v1:{}",
                serde_json::json!({
                    "devices": {absent: {"name": "Old laptop", "os": "linux", "key": absent_key}},
                    "revoked": [],
                })
            ),
            owner: "fixture".into(),
            ..Default::default()
        }]),
        ..Default::default()
    });
    let second_paths = Paths {
        data: fixture.paths.data.join("speaker"),
        cache: fixture.paths.cache.join("speaker"),
    };
    for (paths, id, key) in [
        (&fixture.paths, first, [1u8; 32]),
        (&second_paths, second, [2u8; 32]),
    ] {
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::create_dir_all(&paths.cache).unwrap();
        paths.save_account(&fixture.account).unwrap();
        paths
            .save_credentials(
                &fixture.account,
                &ServerCredentials::from_password("synthetic-password").unwrap(),
            )
            .unwrap();
        std::fs::write(paths.data.join("sync-device-id"), id).unwrap();
        std::fs::write(paths.data.join("sync-signing-key"), key).unwrap();
    }
    let start = |paths: Paths| {
        let worker = fixture.start_on(paths);
        worker.wait_ready(WAIT).unwrap();
        worker
    };
    let lan_status = |worker: &Worker, wanted: &dyn Fn(&LanStatus) -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Event::LanStatus(status) = worker
                .events()
                .recv_deadline(deadline)
                .expect("LAN status before deadline")
                && wanted(&status)
            {
                break status;
            }
        }
    };
    let found = |status: &LanStatus, id: &str, port: u16| {
        status.devices.iter().any(|device| {
            device.id == id
                && matches!(device.reach, LanReach::Found(address) | LanReach::Connected(address) if address.port() == port)
        })
    };

    let mut left = start(fixture.paths.clone());
    let left_port = lan_status(&left, &|status| status.port != 0).port;
    let mut right = start(second_paths.clone());
    let right_port = lan_status(&right, &|status| status.port != 0).port;

    // Each device learns the other's address from Navidrome, without network discovery.
    let status = lan_status(&left, &|status| found(status, second, right_port));
    assert!(
        status.devices.iter().any(|device| device.id == absent
            && device.name == "Old laptop"
            && device.reach == LanReach::NotFound),
        "a trusted device without a published address is shown as not found"
    );
    lan_status(&right, &|status| found(status, first, left_port));
    for (worker, other) in [(&left, second), (&right, first)] {
        event(worker, |event| {
            matches!(event, Event::DeviceSync(status)
                if status.connected && status.devices.iter().any(|device| device.id == other && device.online))
        });
    }

    // The listener port is remembered, so published addresses stay valid after a restart.
    right.shutdown(WAIT).unwrap();
    let mut right = start(second_paths);
    assert_eq!(
        lan_status(&right, &|status| status.port != 0).port,
        right_port
    );

    right.shutdown(WAIT).unwrap();
    left.shutdown(WAIT).unwrap();
    fixture.finish();
}

const MAC: &str = "00000000000000000000000000000001";
const LINUX: &str = "00000000000000000000000000000002";

/// Signs each ID in to the fixture's Navidrome account as its own device, seeded
/// with the fixture library. The first device uses the fixture's own paths.
fn sign_in_devices(fixture: &Fixture, ids: &[&str]) -> Vec<sonora::platform::Paths> {
    use sonora::platform::{Paths, ServerCredentials};
    ids.iter()
        .enumerate()
        .map(|(index, id)| {
            let paths = if index == 0 {
                fixture.paths.clone()
            } else {
                Paths {
                    data: fixture.paths.data.join(id),
                    cache: fixture.paths.cache.join(id),
                }
            };
            std::fs::create_dir_all(&paths.data).unwrap();
            std::fs::create_dir_all(&paths.cache).unwrap();
            paths.save_account(&fixture.account).unwrap();
            paths
                .save_credentials(
                    &fixture.account,
                    &ServerCredentials::from_password("synthetic-password").unwrap(),
                )
                .unwrap();
            std::fs::write(paths.data.join("sync-device-id"), id).unwrap();
            std::fs::write(paths.data.join("sync-signing-key"), [index as u8 + 1; 32]).unwrap();
            fixture.seed_on(&paths);
            paths
        })
        .collect()
}

fn playing(fixture: &Fixture, ids: &[&str]) -> PlaybackSnapshot {
    use sonora::sync::PlaybackObservation;
    use std::sync::Arc;
    let ids: Vec<String> = ids.iter().map(|id| (*id).to_owned()).collect();
    PlaybackSnapshot {
        sync: Some(Arc::new(PlaybackObservation {
            namespace: fixture.account.namespace(),
            revision: 1,
            volume: 50,
            shuffle: false,
            repeat: Default::default(),
        })),
        queue: fixture.catalog().by_ids(&ids).unwrap().into(),
        index: Some(0),
        playing: true,
        ..Default::default()
    }
}

/// Waits up to 15 s for a device-sync status, since LAN links redial every few seconds.
fn device_sync(
    worker: &sonora::service::test_support::Worker,
    wanted: impl Fn(&sonora::sync::DeviceStatus) -> bool,
) -> sonora::sync::DeviceStatus {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if let Event::DeviceSync(status) = worker
            .events()
            .recv_deadline(deadline)
            .expect("device sync status before deadline")
            && wanted(&status)
        {
            return status;
        }
    }
}

fn player_command(
    worker: &sonora::service::test_support::Worker,
    wanted: impl Fn(&PlayerCommand) -> bool,
) -> PlayerCommand {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let command = worker
            .player_commands()
            .recv_deadline(deadline)
            .expect("player command before deadline");
        if wanted(&command) {
            return command;
        }
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lan_controller_sees_and_controls_the_playing_device() {
    let fixture = Fixture::new(Library {
        device_playlists: Some(vec![]),
        ..Default::default()
    });
    let mut paths = sign_in_devices(&fixture, &[MAC, LINUX]).into_iter();
    let (mac_paths, linux_paths) = (paths.next().unwrap(), paths.next().unwrap());
    let mut mac = fixture.start_on(mac_paths);
    mac.wait_ready(WAIT).unwrap();
    device_sync(&mac, |status| status.connected);
    mac.publish_playback(playing(&fixture, &["song"]));
    device_sync(&mac, |status| {
        status.active_device_id.as_deref() == Some(MAC)
    });

    // The Linux PC opens later and sees the Mac playing, but keeps its own
    // player until someone chooses the Mac in Devices.
    let mut linux = fixture.start_on(linux_paths);
    linux.wait_ready(WAIT).unwrap();
    device_sync(&linux, |status| {
        status.active_device_id.as_deref() == Some(MAC)
            && status
                .playback
                .as_ref()
                .is_some_and(|state| state.playing && state.queue == ["song"])
            && status
                .devices
                .iter()
                .any(|device| device.id == MAC && device.online)
    });
    linux.service().send(Command::Enqueue(
        vec!["song".into()],
        sonora::playback::Placement::Next,
    ));
    player_command(&linux, |command| {
        matches!(
            command,
            PlayerCommand::Insert(_, sonora::playback::Placement::Next)
        )
    });
    linux
        .service()
        .send(Command::ControlDevice(Some(MAC.into())));
    device_sync(&linux, |status| status.remote_id() == Some(MAC));

    // Transport controls and picking a song both run on the Mac.
    linux
        .service()
        .send(Command::RemotePlayback(PlaybackCommand::Next));
    player_command(&mac, |command| matches!(command, PlayerCommand::Next));
    linux
        .service()
        .send(Command::RemotePlayback(PlaybackCommand::SetQueue {
            track_ids: vec!["other-song".into()],
            index: Some(0),
            position_ms: 0,
        }));
    player_command(&mac, |command| {
        matches!(command, PlayerCommand::Restore(tracks, 0, _, None)
            if tracks.iter().map(|track| track.id.as_str()).eq(["other-song"]))
    });

    // Queue edits made on the Linux PC apply to the Mac's queue.
    linux.service().send(Command::Enqueue(
        vec!["other-song".into()],
        sonora::playback::Placement::Last,
    ));
    player_command(&mac, |command| {
        matches!(command, PlayerCommand::Insert(tracks, sonora::playback::Placement::Last)
            if tracks.iter().map(|track| track.id.as_str()).eq(["other-song"]))
    });
    let edit = |queue: &[&str], index| {
        let queue: Vec<String> = queue.iter().map(|id| (*id).to_owned()).collect();
        Command::RemotePlayback(PlaybackCommand::EditQueue {
            queue: sonora_sync_protocol::queue_fingerprint(&queue),
            change: sonora_sync_protocol::QueueChange::RemoveAt { index },
        })
    };
    // An edit made against a queue the Mac no longer has is ignored.
    linux.service().send(edit(&["song", "other-song"], 1));
    linux.service().send(edit(&["song"], 0));
    assert!(matches!(
        player_command(&mac, |command| matches!(command, PlayerCommand::Remove(_))),
        PlayerCommand::Remove(0)
    ));

    // When the Mac closes, the Linux PC no longer shows it as the active device.
    mac.shutdown(WAIT).unwrap();
    device_sync(&linux, |status| {
        status.connected
            && status.remote_id().is_none()
            && status.devices.iter().all(|device| device.id != MAC)
    });
    linux.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lan_transfer_moves_playback_and_keeps_the_source_on_failure() {
    let fixture = Fixture::new(Library {
        device_playlists: Some(vec![]),
        ..Default::default()
    });
    let mut paths = sign_in_devices(&fixture, &[MAC, LINUX]).into_iter();
    let (mac_paths, linux_paths) = (paths.next().unwrap(), paths.next().unwrap());
    let mut mac = fixture.start_on(mac_paths);
    mac.wait_ready(WAIT).unwrap();
    let mut linux = fixture.start_on(linux_paths);
    linux.wait_ready(WAIT).unwrap();
    device_sync(&mac, |status| {
        status.devices.iter().any(|device| device.id == LINUX)
    });
    mac.publish_playback(playing(&fixture, &["song"]));
    device_sync(&linux, |status| {
        status.other_active() == Some(MAC) && status.remote_id().is_none()
    });

    let prepare = |saved: Vec<&str>| {
        let reply = loop {
            if let PlayerCommand::SaveQueue(reply) = player_command(&linux, |command| {
                matches!(command, PlayerCommand::SaveQueue(_))
            }) {
                break reply;
            }
        };
        reply
            .send(SavedQueue {
                ids: saved.into_iter().map(str::to_owned).collect(),
                index: Some(0),
                ..Default::default()
            })
            .unwrap();
    };

    // A target that cannot prepare the queue leaves the Mac in control.
    mac.service().send(Command::TransferTo(LINUX.into(), false));
    prepare(vec!["other-song"]);
    event(&mac, |event| matches!(event, Event::Error(_)));
    assert_eq!(
        device_sync(&linux, |status| status.active_device_id.is_some())
            .active_device_id
            .as_deref(),
        Some(MAC)
    );

    mac.service().send(Command::TransferTo(LINUX.into(), false));
    prepare(vec!["song"]);
    device_sync(&mac, |status| {
        status.remote_id() == Some(LINUX)
            && status
                .playback
                .as_ref()
                .is_some_and(|state| state.queue == ["song"])
    });
    player_command(&mac, |command| matches!(command, PlayerCommand::Pause));

    // The Mac now controls playback on the Linux PC.
    mac.service()
        .send(Command::RemotePlayback(PlaybackCommand::Next));
    player_command(&linux, |command| matches!(command, PlayerCommand::Next));

    mac.shutdown(WAIT).unwrap();
    linux.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[tokio::test]
async fn lan_nodes_keep_one_link_and_drop_untrusted_or_revoked_devices() {
    use ed25519_dalek::SigningKey;
    use sonora::sync::lan::{LanNode, Trusted};

    let device = |id: &str| Device {
        id: id.into(),
        name: id.into(),
        os: "linux".into(),
        online: true,
        last_seen: 0,
    };
    let trust = |id: &str, seed: u8, port: u16| Trusted {
        key: SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes(),
        device: device(id),
        addresses: vec![([127, 0, 0, 1], port).into()],
    };
    let (a_port, b_port) = (free_port(), free_port());
    let a = LanNode::listen(
        device("a"),
        [1; 32],
        [3; 32],
        a_port,
        vec![trust("b", 2, b_port)],
    )
    .await
    .unwrap();
    let b = LanNode::listen(
        device("b"),
        [2; 32],
        [3; 32],
        b_port,
        vec![trust("a", 1, a_port)],
    )
    .await
    .unwrap();
    // An unknown key that trusts `a` never gets a link.
    let stranger = LanNode::listen(
        device("c"),
        [9; 32],
        [3; 32],
        0,
        vec![trust("a", 1, a_port)],
    )
    .await
    .unwrap();
    let linked = async |wanted: &dyn Fn() -> bool| {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !wanted() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("LAN links settle");
    };
    linked(&|| a.linked() == ["b"] && b.linked() == ["a"]).await;

    let (mut player, _) = a.session();
    let (mut controller, _) = b.session();
    let state = |seq| PlaybackState {
        queue: vec!["song".into()],
        index: Some(0),
        playing: true,
        seq,
        ..Default::default()
    };
    player.send(ClientMessage::StateUpdate { state: state(1) });
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(ServerMessage::State {
                state,
                from_device_id,
            }) = controller.receive().await
                && from_device_id == "a"
                && state.seq == 1
            {
                break;
            }
        }
    })
    .await
    .expect("the controller sees the playing device's state");
    // A paused device cannot publish over the active device.
    controller.send(ClientMessage::StateUpdate {
        state: PlaybackState {
            playing: false,
            ..state(2)
        },
    });
    // Both devices dial each other; a duplicate link must not replace the kept one.
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    assert_eq!(a.linked(), ["b"]);
    assert!(stranger.linked().is_empty(), "an untrusted key never links");
    while let Ok(Some(message)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), player.receive()).await
    {
        assert!(
            !matches!(&message, ServerMessage::State { from_device_id, .. } if from_device_id == "b"),
            "a paused device's playback never replaces the active device's"
        );
        assert!(
            !matches!(&message, ServerMessage::DevicesChanged { devices } if devices.len() != 2),
            "the kept link never dropped"
        );
    }

    a.revoke("b").unwrap();
    linked(&|| a.linked().is_empty() && b.linked().is_empty()).await;
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(ServerMessage::ActiveDeviceChanged { device_id: None }) =
                controller.receive().await
            {
                break;
            }
        }
    })
    .await
    .expect("losing the playing device clears it on the controller");
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    assert!(a.linked().is_empty(), "a revoked device cannot link again");
}

#[tokio::test]
async fn loopback_lan_links_authenticate_only_trusted_keys() {
    use ed25519_dalek::SigningKey;
    use sonora::sync::lan_security::{Link, Trust};
    use tokio::net::TcpStream;
    use tokio_tungstenite::{accept_async, client_async};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let exchange = async |dial_trust, listen_trust, dial_identity| {
        let initiator = async {
            let stream = TcpStream::connect(addr).await.unwrap();
            let (socket, _) = client_async(format!("ws://{addr}/ws"), stream)
                .await
                .unwrap();
            Link::establish(socket, true, &[3; 32], dial_identity, dial_trust).await
        };
        let responder = async {
            let (stream, _) = listener.accept().await.unwrap();
            let socket = accept_async(stream).await.unwrap();
            Link::establish(socket, false, &[3; 32], &[2; 32], listen_trust).await
        };
        tokio::time::timeout(WAIT, async { tokio::join!(initiator, responder) })
            .await
            .unwrap()
    };
    let key = |seed: u8| {
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes()
    };
    let trusted_left = [key(1)];
    let trusted_right = [key(2)];
    let (left, right) = exchange(
        Trust::Paired(&trusted_right),
        Trust::Paired(&trusted_left),
        &[1; 32],
    )
    .await;
    let (mut left, _) = left.unwrap();
    let (mut right, _) = right.unwrap();
    let queue_payload = vec![42; 128_000];
    let (sent, received) = tokio::join!(left.send(&queue_payload), right.recv());
    sent.unwrap();
    assert_eq!(received.unwrap(), queue_payload);

    let (left, right) = exchange(
        Trust::Paired(&trusted_right),
        Trust::Paired(&trusted_left),
        &[4; 32],
    )
    .await;
    assert!(
        left.is_err() && right.is_err(),
        "untrusted devices never receive a session"
    );
    let (left, right) = exchange(Trust::Paired(&trusted_right), Trust::Paired(&[]), &[1; 32]).await;
    assert!(
        left.is_err() && right.is_err(),
        "a revoked key no longer authenticates"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_falls_back_to_lan_and_returns_to_relay() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::SigningKey;
    use sonora::sync::lan::{LanNode, Trusted, user_hash};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let speaker = "ffffffffffffffffffffffffffffffff";
    let fixture = Fixture::new(Library::default());
    std::fs::write(fixture.paths.data.join("sync-device-id"), MAC).unwrap();
    std::fs::write(fixture.paths.data.join("sync-signing-key"), [1; 32]).unwrap();
    let key = |seed: u8| {
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes()
    };
    let speaker_port = free_port();
    // The worker trusts the speaker through its cached Navidrome device list.
    std::fs::write(
        fixture
            .paths
            .data
            .join(format!("sync-devices-{}.json", fixture.account.namespace())),
        format!(
            "Sonora's device list. This private playlist stores Sonora device-sync keys. Do not edit.\nSonora devices v1:{}",
            serde_json::json!({
                "devices": {speaker: {
                    "name": "LAN speaker",
                    "os": "linux",
                    "key": URL_SAFE_NO_PAD.encode(key(7)),
                    "addresses": [format!("127.0.0.1:{speaker_port}")],
                }},
                "revoked": [],
            })
        ),
    )
    .unwrap();

    let allow_relay = Arc::new(AtomicBool::new(true));
    let (disconnect, closed) = flume::bounded::<()>(1);
    let (sent, reconciled) = flume::unbounded();
    let relay_available = allow_relay.clone();
    let relay = Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let available = relay_available.load(Ordering::SeqCst);
            let closed = closed.clone();
            let sent = sent.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    if !available || !matches!(socket.recv().await, Some(Ok(Message::Text(_)))) {
                        return;
                    }
                    let msg = ServerMessage::Welcome {
                        devices: vec![],
                        active_device_id: Some(speaker.into()),
                        state: None,
                        settings: serde_json::json!({}),
                        settings_version: 0,
                    };
                    socket
                        .send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
                        .await
                        .unwrap();
                    loop {
                        tokio::select! {
                            _ = closed.recv_async() => {
                                let _ = socket.send(Message::Close(None)).await;
                                break;
                            }
                            incoming = socket.recv() => {
                                let Some(Ok(Message::Text(text))) = incoming else { break };
                                if let Ok(ClientMessage::Reconcile { playback, settings }) = serde_json::from_str(&text) {
                                    sent.send((playback, settings)).unwrap();
                                }
                            }
                        }
                    }
                })
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    fixture
        .catalog()
        .set_state(
            "preferences",
            &Preferences {
                sync_server_url: format!("ws://{}", listener.local_addr().unwrap()),
                ..Default::default()
            },
        )
        .unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, relay).await.unwrap() });
    let mut worker = fixture.start();
    worker.wait_ready(WAIT).unwrap();
    let node = LanNode::listen(
        Device {
            id: speaker.into(),
            name: "LAN speaker".into(),
            os: "linux".into(),
            online: true,
            last_seen: 0,
        },
        [7; 32],
        user_hash(&fixture.account),
        speaker_port,
        vec![Trusted {
            key: key(1),
            device: Device {
                id: MAC.into(),
                name: "Mac".into(),
                os: "macos".into(),
                online: true,
                last_seen: 0,
            },
            addresses: vec![],
        }],
    )
    .await
    .unwrap();
    let (mut local, _) = node.session();
    local.send(ClientMessage::StateUpdate {
        state: PlaybackState {
            seq: 25,
            queue: vec!["lan-track".into()],
            index: Some(0),
            ..Default::default()
        },
    });
    local.send(ClientMessage::SettingsPut {
        settings: serde_json::json!({"dark": true}),
        base_version: 0,
    });
    device_sync(&worker, |status| status.other_active() == Some(speaker));
    worker
        .service()
        .send(Command::ControlDevice(Some(speaker.into())));
    device_sync(&worker, |status| status.remote_id() == Some(speaker));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while node.linked() != [MAC] {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the worker links to the trusted speaker");

    // The relay goes away; the worker follows the speaker over the LAN.
    allow_relay.store(false, Ordering::SeqCst);
    disconnect.send(()).unwrap();
    device_sync(&worker, |status| {
        status.remote_id() == Some(speaker)
            && status
                .playback
                .as_ref()
                .is_some_and(|state| state.seq == 25)
    });
    worker
        .service()
        .send(Command::RemotePlayback(PlaybackCommand::Next));
    tokio::time::timeout(WAIT, async {
        loop {
            if matches!(
                local.receive().await,
                Some(ServerMessage::Command {
                    cmd: PlaybackCommand::Next,
                    ..
                })
            ) {
                break;
            }
        }
    })
    .await
    .expect("the LAN command reaches the playing speaker");

    // The relay returns; the worker hands it the playback and settings it learned on the LAN.
    allow_relay.store(true, Ordering::SeqCst);
    device_sync(&worker, |status| {
        status.remote_id() == Some(speaker) && status.playback.is_none()
    });
    let mut playback_seen = false;
    let mut settings_seen = false;
    while !playback_seen || !settings_seen {
        let (playback, settings) = reconciled.recv_timeout(WAIT).unwrap();
        playback_seen |= playback
            .is_some_and(|snapshot| snapshot.device_id == speaker && snapshot.state.seq == 25);
        settings_seen |= settings.is_some_and(|snapshot| {
            snapshot.version == 1 && snapshot.settings.get("dark") == Some(&serde_json::json!(true))
        });
    }
    worker.shutdown(WAIT).unwrap();
    task.abort();
    fixture.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lan_playing_locally_takes_over_without_other_devices_following() {
    let fixture = Fixture::new(Library {
        device_playlists: Some(vec![]),
        ..Default::default()
    });
    let mut paths = sign_in_devices(&fixture, &[MAC, LINUX]).into_iter();
    let (mac_paths, linux_paths) = (paths.next().unwrap(), paths.next().unwrap());
    let mut mac = fixture.start_on(mac_paths);
    mac.wait_ready(WAIT).unwrap();
    device_sync(&mac, |status| status.connected);
    mac.publish_playback(playing(&fixture, &["song"]));
    device_sync(&mac, |status| {
        status.active_device_id.as_deref() == Some(MAC)
    });
    let mut linux = fixture.start_on(linux_paths);
    linux.wait_ready(WAIT).unwrap();
    device_sync(&linux, |status| {
        status.active_device_id.as_deref() == Some(MAC) && status.remote_id().is_none()
    });

    // Playing on the Linux PC makes it the active device and pauses the Mac,
    // and the Mac keeps showing its own player.
    linux.publish_playback(playing(&fixture, &["other-song"]));
    player_command(&mac, |command| matches!(command, PlayerCommand::Pause));
    device_sync(&mac, |status| {
        status.active_device_id.as_deref() == Some(LINUX)
            && status.remote_id().is_none()
            && status
                .playback
                .as_ref()
                .is_some_and(|state| state.queue == ["other-song"])
    });
    device_sync(&linux, |status| {
        status.active_device_id.as_deref() == Some(LINUX)
    });

    // Choosing the Linux PC on the Mac controls it; choosing this device again stops.
    mac.service()
        .send(Command::ControlDevice(Some(LINUX.into())));
    device_sync(&mac, |status| status.remote_id() == Some(LINUX));
    mac.service()
        .send(Command::RemotePlayback(PlaybackCommand::Pause));
    player_command(&linux, |command| matches!(command, PlayerCommand::Pause));
    mac.service().send(Command::ControlDevice(None));
    device_sync(&mac, |status| {
        status.remote_id().is_none() && status.active_device_id.as_deref() == Some(LINUX)
    });

    mac.shutdown(WAIT).unwrap();
    linux.shutdown(WAIT).unwrap();
    fixture.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lan_device_that_restarts_and_resumes_its_saved_queue_shows_as_playing() {
    let fixture = Fixture::new(Library {
        device_playlists: Some(vec![]),
        ..Default::default()
    });
    let mut paths = sign_in_devices(&fixture, &[MAC, LINUX]).into_iter();
    let (mac_paths, linux_paths) = (paths.next().unwrap(), paths.next().unwrap());
    let linux_again = linux_paths.clone();
    let mut mac = fixture.start_on(mac_paths);
    mac.wait_ready(WAIT).unwrap();
    let mut linux = fixture.start_on(linux_paths);
    linux.wait_ready(WAIT).unwrap();
    device_sync(&mac, |status| {
        status.devices.iter().any(|device| device.id == LINUX)
    });

    // The Linux PC plays for a while, then the Mac takes over and pauses.
    linux.publish_playback(playing(&fixture, &["song"]));
    device_sync(&mac, |status| {
        status.active_device_id.as_deref() == Some(LINUX)
            && status.playback.as_ref().is_some_and(|state| state.playing)
    });
    mac.publish_playback(playing(&fixture, &["other-song"]));
    device_sync(&linux, |status| {
        status.active_device_id.as_deref() == Some(MAC)
    });
    mac.publish_playback(PlaybackSnapshot {
        playing: false,
        ..playing(&fixture, &["other-song"])
    });
    linux.shutdown(WAIT).unwrap();
    device_sync(&mac, |status| {
        status.devices.iter().all(|device| device.id != LINUX)
    });

    // The Linux PC opens again and plays the queue it restored, straight away.
    let mut linux = fixture.start_on(linux_again);
    linux.wait_ready(WAIT).unwrap();
    linux.publish_playback(playing(&fixture, &["song"]));
    device_sync(&mac, |status| {
        status.active_device_id.as_deref() == Some(LINUX)
            && status
                .playback
                .as_ref()
                .is_some_and(|state| state.playing && state.queue == ["song"])
    });
    device_sync(&linux, |status| {
        status.active_device_id.as_deref() == Some(LINUX)
    });

    mac.shutdown(WAIT).unwrap();
    linux.shutdown(WAIT).unwrap();
    fixture.finish();
}
