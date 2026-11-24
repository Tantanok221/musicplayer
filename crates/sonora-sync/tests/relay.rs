use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use sonora_sync_protocol::{ClientMessage, Hello, PlaybackCommand, PlaybackState, ServerMessage};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use url::Url;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
#[path = "relay/settings.rs"]
mod settings;
#[path = "relay/transfer.rs"]
mod transfer;
const TOKEN: &str = "0123456789abcdef0123456789abcdef";

struct Running {
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Running {
    async fn start(router: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { addr, task }
    }

    async fn connect(&self) -> Socket {
        connect_async(format!("ws://{}/ws", self.addr))
            .await
            .expect("relay should accept the WebSocket upgrade")
            .0
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn fake_navidrome() -> (Running, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let server = Running::start(Router::new().route(
        "/music/rest/ping.view",
        get(|State(calls): State<Arc<AtomicUsize>>, Query(params): Query<HashMap<String, String>>| async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let valid = matches!(params.get("u").map(String::as_str), Some("alice" | "bob"))
                && params.get("t").is_some_and(|value| value == TOKEN)
                && params.get("s").is_some_and(|value| value == "synthetic-salt");
            Json(json!({"subsonic-response": {"status": if valid { "ok" } else { "failed" }}}))
        }),
    ).with_state(calls.clone())).await;
    (server, calls)
}

async fn start(navidrome: &Running, dir: &TempDir) -> Running {
    let url = Url::parse(&format!("http://{}/music", navidrome.addr)).unwrap();
    Running::start(sonora_sync::router(url, &dir.path().join("relay.sqlite")).unwrap()).await
}

fn hello(username: &str, id: &str, token: &str) -> ClientMessage {
    ClientMessage::Hello(Hello {
        username: username.into(),
        subsonic_token: token.into(),
        subsonic_salt: "synthetic-salt".into(),
        device_id: id.into(),
        device_name: format!("Device {id}"),
        os: "linux".into(),
        client_version: "0.1.0".into(),
    })
}

async fn send(socket: &mut Socket, message: ClientMessage) {
    socket
        .send(Message::Text(
            serde_json::to_string(&message).unwrap().into(),
        ))
        .await
        .unwrap();
}

async fn next(socket: &mut Socket) -> ServerMessage {
    let result = tokio::time::timeout(Duration::from_secs(3), socket.next()).await;
    let message = result
        .expect("relay message deadline")
        .expect("socket closed")
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).expect("valid relay message")
}

async fn login(relay: &Running, username: &str, id: &str) -> (Socket, ServerMessage) {
    let mut socket = relay.connect().await;
    send(&mut socket, hello(username, id, TOKEN)).await;
    let welcome = next(&mut socket).await;
    (socket, welcome)
}

#[tokio::test]
async fn relay_authenticates_and_isolates_accounts_then_relays_presence_state_and_commands() {
    let dir = tempfile::tempdir().unwrap();
    let (navidrome, calls) = fake_navidrome().await;
    let relay = start(&navidrome, &dir).await;
    assert_eq!(
        reqwest::get(format!("http://{}/health", relay.addr))
            .await
            .unwrap()
            .status(),
        200
    );
    let (mut a, welcome) = login(&relay, "alice", "a").await;
    assert!(
        matches!(welcome, ServerMessage::Welcome { active_device_id: Some(id), devices, .. } if id == "a" && devices.len() == 1)
    );
    let (mut b, welcome) = login(&relay, "alice", "b").await;
    assert!(
        matches!(welcome, ServerMessage::Welcome { active_device_id: Some(id), devices, .. } if id == "a" && devices.len() == 2 && devices.iter().all(|d| d.online))
    );
    assert!(
        matches!(next(&mut a).await, ServerMessage::DevicesChanged { devices } if devices.len() == 2)
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "valid credentials are cached"
    );

    let mut rejected = relay.connect().await;
    send(
        &mut rejected,
        hello("alice", "b", "ffffffffffffffffffffffffffffffff"),
    )
    .await;
    assert!(
        matches!(next(&mut rejected).await, ServerMessage::Error { code, .. } if code == "auth_failed")
    );
    let (mut other, welcome) = login(&relay, "bob", "a").await;
    assert!(matches!(welcome, ServerMessage::Welcome { devices, .. } if devices.len() == 1));

    let state = PlaybackState {
        queue: vec!["song".into(), "song".into()],
        index: Some(1),
        position_ms: 100,
        captured_at_ms: -1,
        playing: true,
        seq: 4,
        ..Default::default()
    };
    send(
        &mut a,
        ClientMessage::StateUpdate {
            state: state.clone(),
        },
    )
    .await;
    assert!(
        matches!(next(&mut b).await, ServerMessage::State { from_device_id, state: got } if from_device_id == "a" && got.seq == 4 && got.index == Some(1) && got.captured_at_ms > 0)
    );
    send(
        &mut a,
        ClientMessage::StateUpdate {
            state: PlaybackState {
                seq: 3,
                ..state.clone()
            },
        },
    )
    .await;
    send(
        &mut b,
        ClientMessage::StateUpdate {
            state: PlaybackState {
                seq: 10,
                ..state.clone()
            },
        },
    )
    .await;
    send(&mut b, ClientMessage::Ping).await;
    assert!(matches!(next(&mut b).await, ServerMessage::Pong));
    send(
        &mut a,
        ClientMessage::StateUpdate {
            state: PlaybackState { seq: 5, ..state },
        },
    )
    .await;
    assert!(matches!(next(&mut b).await, ServerMessage::State { state, .. } if state.seq == 5));
    send(&mut other, ClientMessage::Ping).await;
    assert!(matches!(next(&mut other).await, ServerMessage::Pong));

    send(
        &mut b,
        ClientMessage::Command {
            target_device_id: "a".into(),
            cmd: PlaybackCommand::Pause,
        },
    )
    .await;
    assert!(
        matches!(next(&mut a).await, ServerMessage::Command { cmd: PlaybackCommand::Pause, from_device_id } if from_device_id == "b")
    );
    b.close(None).await.unwrap();
    assert!(
        matches!(next(&mut a).await, ServerMessage::DevicesChanged { devices } if devices.iter().any(|d| d.id == "b" && !d.online))
    );
}

#[tokio::test]
async fn retains_devices_and_latest_sequence_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let (navidrome, _) = fake_navidrome().await;
    let relay = start(&navidrome, &dir).await;
    let (mut a, _) = login(&relay, "alice", "a").await;
    let (mut b, _) = login(&relay, "alice", "b").await;
    let _ = next(&mut a).await;
    send(
        &mut a,
        ClientMessage::StateUpdate {
            state: PlaybackState {
                seq: 12,
                ..Default::default()
            },
        },
    )
    .await;
    assert!(matches!(next(&mut b).await, ServerMessage::State { state, .. } if state.seq == 12));
    drop(a);
    drop(b);
    drop(relay);

    let relay = start(&navidrome, &dir).await;
    let (mut a, welcome) = login(&relay, "alice", "a").await;
    assert!(
        matches!(welcome, ServerMessage::Welcome { devices, state: Some(state), .. } if state.seq == 12 && devices.iter().any(|d| d.id == "b" && !d.online))
    );
    let (mut b, _) = login(&relay, "alice", "b").await;
    let _ = next(&mut a).await;
    send(
        &mut a,
        ClientMessage::StateUpdate {
            state: PlaybackState {
                seq: 11,
                ..Default::default()
            },
        },
    )
    .await;
    send(
        &mut a,
        ClientMessage::StateUpdate {
            state: PlaybackState {
                seq: 13,
                ..Default::default()
            },
        },
    )
    .await;
    assert!(matches!(next(&mut b).await, ServerMessage::State { state, .. } if state.seq == 13));
}
