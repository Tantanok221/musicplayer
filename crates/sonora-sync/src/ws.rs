use crate::{
    auth::{AuthError, NavidromeAuthenticator},
    session::{Peer, SessionHub, error},
};
use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use sonora_sync_protocol::{ClientMessage, Hello, ServerMessage};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, watch};

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MESSAGE: usize = 512 * 1024;
const OUTBOX: usize = 16;

#[derive(Clone)]
pub(crate) struct Relay {
    pub auth: NavidromeAuthenticator,
    pub hub: SessionHub,
    pub generations: Arc<AtomicU64>,
}

pub(crate) async fn upgrade(ws: WebSocketUpgrade, State(relay): State<Relay>) -> Response {
    ws.max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| connection(socket, relay))
}

fn valid_hello(hello: &Hello) -> bool {
    fn valid(field: &str, max: usize) -> bool {
        !field.trim().is_empty() && field.len() <= max && !field.chars().any(char::is_control)
    }
    valid(&hello.username, 256)
        && hello.subsonic_token.len() == 32
        && hello.subsonic_token.bytes().all(|b| b.is_ascii_hexdigit())
        && valid(&hello.subsonic_salt, 128)
        && valid(&hello.device_id, 128)
        && valid(&hello.device_name, 128)
        && valid(&hello.os, 64)
        && valid(&hello.client_version, 64)
}

async fn write(socket: &mut WebSocket, msg: ServerMessage) -> bool {
    let Ok(text) = serde_json::to_string(&msg) else {
        return false;
    };
    matches!(
        tokio::time::timeout(WRITE_TIMEOUT, socket.send(Message::Text(text.into()))).await,
        Ok(Ok(()))
    )
}

async fn authenticate(socket: &mut WebSocket, auth: &NavidromeAuthenticator) -> Option<Hello> {
    let Ok(Some(Ok(Message::Text(text)))) =
        tokio::time::timeout(HELLO_TIMEOUT, socket.recv()).await
    else {
        return None;
    };
    let Ok(ClientMessage::Hello(hello)) = serde_json::from_str(text.as_str()) else {
        write(
            socket,
            error("hello_required", "Send Hello before using the relay"),
        )
        .await;
        return None;
    };
    if !valid_hello(&hello) {
        write(
            socket,
            error("invalid_hello", "Invalid login or device fields"),
        )
        .await;
        return None;
    }
    if let Err(err) = auth.check(&hello).await {
        let code = match err {
            AuthError::Rejected => "auth_failed",
            AuthError::Unavailable => "auth_unavailable",
        };
        write(socket, error(code, &err.to_string())).await;
        return None;
    }
    Some(hello)
}

async fn connection(mut socket: WebSocket, relay: Relay) {
    let Some(hello) = authenticate(&mut socket, &relay.auth).await else {
        return;
    };
    let user = hello.username.clone();
    let id = hello.device_id.clone();
    let generation = relay.generations.fetch_add(1, Ordering::Relaxed);
    let (out, mut incoming) = mpsc::channel(OUTBOX);
    let (cancel, mut cancelled) = watch::channel(false);
    let welcome = match relay
        .hub
        .join(
            hello,
            Peer {
                generation,
                out,
                cancel,
            },
        )
        .await
    {
        Ok(welcome) => welcome,
        Err(_) => {
            tracing::warn!("Could not open relay session");
            write(&mut socket, error("unavailable", "Relay is unavailable")).await;
            return;
        }
    };
    if write(&mut socket, welcome).await {
        loop {
            tokio::select! {
                _ = cancelled.changed() => break,
                msg = incoming.recv() => {
                    let Some(msg) = msg else { break };
                    if !write(&mut socket, msg).await { break; }
                }
                frame = socket.recv() => {
                    match frame {
                        Some(Ok(Message::Text(text))) => {
                            let Ok(msg) = serde_json::from_str(text.as_str()) else {
                                if !write(&mut socket, error("invalid_message", "Invalid protocol message")).await {
                                    break;
                                }
                                continue;
                            };
                            if relay.hub.receive(user.clone(), id.clone(), generation, msg).await.is_err() {
                                tracing::warn!("Could not process relay message");
                                if !write(&mut socket, error("unavailable", "Relay is unavailable")).await {
                                    break;
                                }
                            }
                        }
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                        _ => break,
                    }
                }
            }
        }
    }
    if relay.hub.disconnect(user, id, generation).await.is_err() {
        tracing::warn!("Could not close relay session");
    }
}
