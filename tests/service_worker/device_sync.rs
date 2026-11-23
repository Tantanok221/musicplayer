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
async fn worker_falls_back_to_paired_lan_and_returns_to_relay() {
    use sonora::sync::lan::LanNode;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let fixture = Fixture::new(Library::default());
    let speaker = "ffffffffffffffffffffffffffffffff";
    std::fs::write(
        fixture.paths.data.join("sync-device-id"),
        "00000000000000000000000000000001",
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
                    if !available {
                        return;
                    }
                    if !matches!(socket.recv().await, Some(Ok(Message::Text(_)))) {
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
    let (account, _) = worker.wait_ready(WAIT).unwrap();
    let device = Device {
        id: speaker.into(),
        name: "LAN speaker".into(),
        os: "linux".into(),
        online: true,
        last_seen: 0,
    };
    let node = LanNode::listen(
        device,
        [7; 32],
        sonora::sync::lan::user_hash(&account.unwrap()),
        0,
    )
    .await
    .unwrap();
    let (mut local, _) = node.connect(node.address()).await.unwrap();
    local
        .send(ClientMessage::StateUpdate {
            state: PlaybackState {
                seq: 25,
                queue: vec!["lan-track".into()],
                index: Some(0),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    local
        .send(ClientMessage::SettingsPut {
            settings: serde_json::json!({"dark": true}),
            base_version: 0,
        })
        .await
        .unwrap();
    event(
        &worker,
        |event| matches!(event, Event::DeviceSync(status) if status.remote_id() == Some(speaker)),
    );
    worker
        .service()
        .send(Command::LanPair(node.address(), node.offer_code()));
    // Let the paired node report the pin before simulating an outage.
    tokio::time::timeout(WAIT, async {
        loop {
            if node.paired_count() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("worker paired to the LAN peer");
    allow_relay.store(false, Ordering::SeqCst);
    disconnect.send(()).unwrap();
    event(
        &worker,
        |event| matches!(event, Event::DeviceSync(status) if status.remote_id() == Some(speaker) && status.playback.as_ref().is_some_and(|state| state.seq == 25)),
    );
    worker
        .service()
        .send(Command::RemotePlayback(PlaybackCommand::Next));
    tokio::time::timeout(WAIT, async {
        loop {
            if matches!(
                local.receive().await.unwrap(),
                ServerMessage::Command {
                    cmd: PlaybackCommand::Next,
                    ..
                }
            ) {
                break;
            }
        }
    })
    .await
    .expect("the LAN command reaches the last active, higher-ID device");
    allow_relay.store(true, Ordering::SeqCst);
    event(
        &worker,
        |event| matches!(event, Event::DeviceSync(status) if status.remote_id() == Some(speaker) && status.playback.is_none()),
    );
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

#[tokio::test]
async fn paired_lan_nodes_transfer_over_manual_loopback_addresses() {
    use sonora::sync::lan::LanNode;

    let device = |id: &str| Device {
        id: id.into(),
        name: id.into(),
        os: "linux".into(),
        online: true,
        last_seen: 0,
    };
    let a = LanNode::listen(device("a"), [1; 32], [3; 32], 0)
        .await
        .unwrap();
    let b = LanNode::listen(device("b"), [2; 32], [3; 32], 0)
        .await
        .unwrap();
    assert!(b.connect(a.address()).await.is_err());
    b.pair(a.address(), &a.offer_code()).await.unwrap();
    let (mut source, _) = a.connect(a.address()).await.unwrap();
    let (mut target, _) = b.connect(a.address()).await.unwrap();
    let state = PlaybackState {
        queue: vec!["track".into()],
        index: Some(0),
        playing: true,
        position_ms: 313,
        seq: 1,
        ..Default::default()
    };
    source
        .send(ClientMessage::StateUpdate {
            state: state.clone(),
        })
        .await
        .unwrap();
    loop {
        let incoming = tokio::time::timeout(WAIT, target.receive())
            .await
            .unwrap()
            .unwrap();
        if matches!(incoming, ServerMessage::State {state: received, ..} if received.seq == 1) {
            break;
        }
    }
    source
        .send(ClientMessage::Transfer {
            target_device_id: "b".into(),
            start_playing: true,
        })
        .await
        .unwrap();
    let id = loop {
        let incoming = tokio::time::timeout(WAIT, target.receive())
            .await
            .unwrap()
            .unwrap();
        if let ServerMessage::TransferRequest {
            transfer_id,
            state: requested,
            start_playing,
        } = incoming
        {
            assert_eq!(requested.queue, state.queue);
            assert!(start_playing);
            break transfer_id;
        }
    };
    target
        .send(ClientMessage::TransferAck {
            transfer_id: id,
            ok: true,
            error: None,
        })
        .await
        .unwrap();
    loop {
        let incoming = tokio::time::timeout(WAIT, source.receive())
            .await
            .unwrap()
            .unwrap();
        if matches!(incoming, ServerMessage::TransferResult { ok: true, .. }) {
            break;
        }
    }
    a.revoke("b").unwrap();
    assert!(b.connect(a.address()).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lan_authority_moves_to_the_higher_id_after_transfer() {
    let source_id = "00000000000000000000000000000001";
    let target_id = "ffffffffffffffffffffffffffffffff";
    let source = Fixture::new(Library::default());
    let mut target = Fixture::new(Library::default());
    target.account = source.account.clone();
    target.paths.save_account(&target.account).unwrap();
    target
        .paths
        .save_credentials(
            &target.account,
            &source.paths.credentials(&source.account).unwrap().unwrap(),
        )
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    for (fixture, id) in [(&source, source_id), (&target, target_id)] {
        std::fs::write(fixture.paths.data.join("sync-device-id"), id).unwrap();
        fixture.seed();
        fixture
            .catalog()
            .set_state(
                "preferences",
                &Preferences {
                    sync_server_url: url.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let (disconnect, down) = tokio::sync::watch::channel(false);
    let relay = Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let mut down = down.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    if *down.borrow() || !matches!(socket.recv().await, Some(Ok(Message::Text(_))))
                    {
                        return;
                    }
                    let welcome = ServerMessage::Welcome {
                        devices: vec![],
                        active_device_id: Some(source_id.into()),
                        state: Some(PlaybackState {
                            queue: vec!["song".into()],
                            index: Some(0),
                            seq: 10,
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
                    let _ = down.wait_for(|&down| down).await;
                    let _ = socket.send(Message::Close(None)).await;
                })
            }
        }),
    );
    let relay = tokio::spawn(async move { axum::serve(listener, relay).await.unwrap() });
    let mut old = source.start();
    let mut new = target.start();
    let Event::LanStatus(status) = event(
        &old,
        |event| matches!(event, Event::LanStatus(status) if status.port != 0),
    ) else {
        unreachable!();
    };
    for worker in [&old, &new] {
        event(
            worker,
            |event| matches!(event, Event::DeviceSync(status) if status.connected && status.active_device_id.as_deref() == Some(source_id)),
        );
    }
    old.service().send(Command::LanShowCode);
    let Event::LanOffer(code) = event(&old, |event| matches!(event, Event::LanOffer(_))) else {
        unreachable!();
    };
    new.service().send(Command::LanPair(
        ([127, 0, 0, 1], status.port).into(),
        code.0,
    ));
    event(
        &new,
        |event| matches!(event, Event::LanStatus(status) if status.peers.iter().any(|peer| peer.id == source_id)),
    );
    disconnect.send(true).unwrap();
    event(
        &old,
        |event| matches!(event, Event::DeviceSync(status) if status.connected && status.devices.iter().any(|device| device.id == target_id && device.online)),
    );

    old.service()
        .send(Command::TransferTo(target_id.into(), false));
    let deadline = std::time::Instant::now() + WAIT;
    let reply = loop {
        let command = new.player_commands().recv_deadline(deadline).unwrap();
        if let PlayerCommand::SaveQueue(reply) = command {
            break reply;
        }
    };
    reply
        .send(SavedQueue {
            ids: vec!["song".into()],
            index: Some(0),
            ..Default::default()
        })
        .unwrap();

    event(
        &old,
        |event| matches!(event, Event::DeviceSync(status) if !status.connected),
    );
    event(&old, |event| {
        matches!(event, Event::DeviceSync(status)
        if status.remote_id() == Some(target_id) && status.playback.as_ref().is_some_and(|state| state.seq == 11))
    });
    old.service()
        .send(Command::RemotePlayback(PlaybackCommand::Next));
    let deadline = std::time::Instant::now() + WAIT;
    while !matches!(
        new.player_commands().recv_deadline(deadline).unwrap(),
        PlayerCommand::Next
    ) {}

    old.shutdown(WAIT).unwrap();
    new.shutdown(WAIT).unwrap();
    relay.abort();
    source.finish();
    target.finish();
}

#[tokio::test]
async fn loopback_lan_pairs_and_authenticates_only_pinned_keys() {
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
    let (left, right) = exchange(Trust::Pair("123456"), Trust::Pair("123456"), &[1; 32]).await;
    let (_, right_key) = left.expect("matching one-time code pairs the initiating device");
    let (_, left_key) = right.expect("matching one-time code pairs the listening device");
    let trusted_left = [left_key];
    let trusted_right = [right_key];
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
        "unpaired devices never receive a session"
    );
    let (left, right) = exchange(Trust::Pair("123456"), Trust::Pair("654321"), &[1; 32]).await;
    assert!(
        left.is_err() && right.is_err(),
        "a wrong code never pairs either device"
    );
    let (left, right) = exchange(Trust::Paired(&trusted_right), Trust::Paired(&[]), &[1; 32]).await;
    assert!(
        left.is_err() && right.is_err(),
        "a revoked pin no longer authenticates"
    );
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
        |event| matches!(event, Event::DeviceSync(status) if status.remote_id() == Some("speaker")),
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
