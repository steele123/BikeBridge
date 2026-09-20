//! BikeBridge HTTP/WebSocket transport for BLE discovery and optional mock devices.
pub mod api;
pub mod protocol;
mod replay;
pub mod state;
mod ui;
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

// Only the dashboard's exact HTTP origin may use the browser API, including WS.
// Validate Host independently to prevent DNS rebinding. Native clients omit Origin.
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
    let origin_ok = match request.headers().get(header::ORIGIN) {
        None => true,
        Some(origin) => origin
            .to_str()
            .ok()
            .and_then(|v| v.parse::<axum::http::Uri>().ok())
            .is_some_and(|origin| {
                origin.scheme_str() == Some("http")
                    && origin
                        .path_and_query()
                        .is_none_or(|path| path.as_str() == "/")
                    && origin.authority().is_some_and(|authority| {
                        request
                            .headers()
                            .get(header::HOST)
                            .and_then(|v| v.to_str().ok())
                            .is_some_and(|host| authority.as_str().eq_ignore_ascii_case(host))
                    })
            }),
    };
    let cross_site = request
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site");
    if !origin_ok || !host_ok || cross_site {
        return (StatusCode::FORBIDDEN, axum::Json(serde_json::json!({"type":"error", "data": {
            "code":"access_denied", "message":"Use the dashboard's exact loopback origin or a native client."
        }}))).into_response();
    }
    next.run(request).await
}

/// Build the API router for a particular loopback listening port.
pub fn router(state: AppState, port: u16) -> Router {
    Router::new()
        .route("/", get(ui::index))
        .route("/app.js", get(ui::javascript))
        .route("/app.css", get(ui::stylesheet))
        .route("/overlay", get(ui::overlay_index))
        .route("/overlay/", get(ui::overlay_index))
        .route("/overlay/app.js", get(ui::overlay_javascript))
        .route("/overlay/app.css", get(ui::overlay_stylesheet))
        .route("/api/status", get(api::status))
        .route("/api/replay", get(api::replay_status))
        .route("/api/replay/{action}", post(api::replay_action))
        .route("/api/adapters", get(api::adapters))
        .route("/api/scan/start", post(api::scan_start))
        .route("/api/scan/stop", post(api::scan_stop))
        .route("/api/scan/select", post(api::select_name))
        .route("/api/scan/nearby", get(api::nearby))
        .route("/api/scan/nearby/{id}/select", post(api::select_nearby))
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
    let replay_state = state.clone();
    let replay_worker = tokio::spawn(async move {
        if replay_state.replay_status().await.is_none() {
            return;
        }
        let mut interval = tokio::time::interval(Duration::from_millis(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = replay_state.shutdown.cancelled() => break,
                _ = interval.tick() => replay_state.replay_tick().await,
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
    let _ = replay_worker.await;
    result
}
