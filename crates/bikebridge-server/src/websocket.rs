//! Bounded WebSocket sessions, subscriptions, heartbeat, and ownership cleanup.
use crate::{
    AppState,
    protocol::{self, Command, Subscription},
};
use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use bikebridge_core::{BridgeError, ErrorCode, Event};
use futures_util::SinkExt;
use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::json;
use std::time::Duration;
use tokio::{
    sync::broadcast,
    time::{Instant, interval, timeout},
};

pub(crate) async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    // Register before the upgrade is scheduled so shutdown cannot miss a pending session.
    let tracked = state.sessions.token();
    ws.max_message_size(16 * 1024)
        .max_frame_size(16 * 1024)
        .on_upgrade(move |socket| async move {
            let _tracked = tracked;
            session(socket, state).await;
        })
}

async fn send(socket: &mut WebSocket, value: &impl Serialize) -> bool {
    let Ok(text) = serde_json::to_string(value) else {
        return false;
    };
    matches!(
        timeout(
            Duration::from_secs(3),
            socket.send(Message::Text(text.into()))
        )
        .await,
        Ok(Ok(()))
    )
}

async fn session(mut socket: WebSocket, state: AppState) {
    let session = state.open_session();
    tracing::debug!(session, "WebSocket connected");
    run_session(&mut socket, &state, session).await;
    // Every normal exit path (read/write error, timeout, close, shutdown) goes through cleanup.
    state.close_session(session).await;
    // Receiving Close queues an automatic reply. Flush it before attempting our
    // own Close; send(Close) alone can return SendAfterClosing without flushing.
    let _ = timeout(Duration::from_secs(1), socket.flush()).await;
    let _ = timeout(Duration::from_secs(1), socket.send(Message::Close(None))).await;
    tracing::debug!(session, "WebSocket disconnected");
}

async fn run_session(socket: &mut WebSocket, state: &AppState, session: usize) {
    let mut events = state.events.subscribe();
    let mut subscription = Subscription::default();
    if !send(socket, &json!({"type":"hello", "protocolVersion":1, "bikeBridgeVersion":env!("CARGO_PKG_VERSION")})).await { return; }
    let mut heartbeat = interval(Duration::from_secs(10));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_pong = Instant::now();
    // Prevent a local client from flooding commands indefinitely.
    let mut window = Instant::now();
    let mut commands = 0u16;
    let mut pending_connection: Option<BoxFuture<'static, protocol::Response>> = None;
    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            response = async {
                match &mut pending_connection {
                    Some(pending) => pending.await,
                    None => std::future::pending().await,
                }
            } => {
                pending_connection = None;
                if !send(socket,&response).await {break;}
            }
            _ = heartbeat.tick() => {
                if last_pong.elapsed() > Duration::from_secs(30) { break; }
                if !matches!(timeout(Duration::from_secs(3), socket.send(Message::Ping(Vec::new().into()))).await, Ok(Ok(()))) { break; }
            }
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break; };
                match message {
                    Message::Text(text) => {
                        if window.elapsed() >= Duration::from_secs(1) { window = Instant::now(); commands = 0; }
                        commands += 1;
                        if commands > 100 { break; }
                        let (id, decoded) = protocol::decode(&text);
                        if let Ok(Command::Connect {device_id} | Command::Disconnect {device_id}) = &decoded {
                            if pending_connection.is_some() {
                                let error = BridgeError::new(ErrorCode::Busy,"A connection request is already pending on this WebSocket.");
                                if !send(socket,&protocol::Response::new(id,Err(error))).await {break;}
                            } else {
                                let connected = matches!(decoded,Ok(Command::Connect {..}));
                                let device_id = device_id.clone();
                                let state = state.clone();
                                pending_connection = Some(Box::pin(async move {
                                    protocol::Response::new(id,state.set_connection(session,&device_id,connected).await)
                                }));
                            }
                            continue;
                        }
                        let result = match decoded {
                            Ok(command) => state.execute(session, command, &mut subscription).await,
                            Err(error) => Err(error),
                        };
                        if !send(socket, &protocol::Response::new(id, result)).await { break; }
                    }
                    Message::Close(_) => break,
                    Message::Pong(_) => last_pong = Instant::now(),
                    Message::Ping(_) => {}, // Axum/tungstenite queues the required pong automatically.
                    Message::Binary(_) => {
                        let response = protocol::Response::new(None, Err(BridgeError::new(ErrorCode::InvalidCommand, "Use UTF-8 JSON text messages.")));
                        if !send(socket, &response).await { break; }
                    }
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) if subscription.matches(&event) => { if !send(socket, &event).await { break; } },
                    Ok(_) => {},
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let error = Event::Error { data: BridgeError::new(ErrorCode::EventsLost, "Subscriber fell behind; events were dropped. Refresh device state over HTTP.") };
                        if !send(socket, &error).await { break; }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}
