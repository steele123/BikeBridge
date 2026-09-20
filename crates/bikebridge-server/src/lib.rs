//! BikeBridge HTTP/WebSocket transport for BLE discovery and optional mock devices.
pub mod api;
pub mod protocol;
pub mod state;
mod websocket;

use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
pub use state::AppState;
use std::{net::IpAddr, time::Duration};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

// Reject browser-origin requests in v1, including websocket handshakes, and guard
// the Host header against DNS rebinding. Node/native clients send no Origin.
async fn local_only(State(port): State<u16>, request: Request, next: Next) -> Response {
    let host_ok = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<axum::http::uri::Authority>().ok())
        .is_some_and(|authority| {
            let host = authority.host().trim_matches(['[', ']']);
            (host.eq_ignore_ascii_case("localhost")
                || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()))
                && authority.port_u16().unwrap_or(80) == port
        });
    if request.headers().contains_key(header::ORIGIN) || !host_ok {
        return (StatusCode::FORBIDDEN, axum::Json(serde_json::json!({"type":"error", "data": {
            "code":"access_denied", "message":"Use a native client with a loopback Host and no Origin header."
        }}))).into_response();
    }
    next.run(request).await
}

/// Build the API router for a particular loopback listening port.
pub fn router(state: AppState, port: u16) -> Router {
    Router::new()
        .route("/api/status", get(api::status))
        .route("/api/adapters", get(api::adapters))
        .route("/api/scan/start", post(api::scan_start))
        .route("/api/scan/stop", post(api::scan_stop))
        .route("/api/devices", get(api::devices))
        .route("/api/devices/{id}", get(api::device))
        .route("/api/devices/{id}/connect", post(api::connect))
        .route("/api/devices/{id}/disconnect", post(api::disconnect))
        .route("/ws", get(websocket::upgrade))
        .layer(middleware::from_fn_with_state(port, local_only))
        .with_state(state)
}

/// Serve a loopback listener until cancellation. Cancelling resets trainer load,
/// closes WebSocket sessions, and joins the simulation task.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let address = listener.local_addr()?;
    if !address.ip().is_loopback() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "BikeBridge only supports loopback listeners.",
        ));
    }
    let worker_state = state.clone();
    let ticker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut previous = tokio::time::Instant::now();
        loop {
            tokio::select! {
                _ = worker_state.shutdown.cancelled() => break,
                now = interval.tick() => {
                    let elapsed = now.saturating_duration_since(previous);
                    previous = now;
                    worker_state.tick(elapsed).await;
                }
            }
        }
    });
    let stop_state = state.clone();
    let result = axum::serve(listener, router(state.clone(), address.port()))
        .with_graceful_shutdown(async move {
            shutdown.cancelled().await;
            stop_state.shutdown.cancel();
            stop_state.safe_state().await;
            stop_state.shutdown_discovery().await;
        })
        .await;
    state.shutdown.cancel();
    state.safe_state().await;
    state.shutdown_discovery().await;
    state.sessions.close();
    state.sessions.wait().await;
    let _ = ticker.await;
    result
}
