use super::fixture::{Fixture, Library, WAIT, event};
use axum::{
    Router,
    extract::{WebSocketUpgrade, ws::Message},
    routing::get,
};
use sonora::{
    model::Preferences,
    playback::PlayerCommand,
    service::{Command, Event},
};
use sonora_sync_protocol::{ClientMessage, Device, PlaybackCommand, PlaybackState, ServerMessage};
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_worker_uses_saved_login_and_bridges_relay_controls() {
    let fixture = Fixture::new(Library::default());
    let (out, received) = flume::unbounded();
    let relay = Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let out = out.clone();
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
    worker.shutdown(WAIT).unwrap();
    task.abort();
    fixture.finish();
}
