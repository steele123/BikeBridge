//! Local HTTP discovery and connection API; trainer control remains session-bound over WebSocket.
use crate::{AppState, state::Status};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use bikebridge_core::{AdapterInfo, BridgeError, DeviceInfo, ErrorCode, ScanStatus};
use serde_json::{Value, json};

pub(crate) async fn status(State(state): State<AppState>) -> Json<Status> {
    Json(state.status().await)
}
pub(crate) async fn devices(State(state): State<AppState>) -> Json<Vec<DeviceInfo>> {
    Json(state.devices().await)
}
pub(crate) async fn device(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> std::result::Result<Json<DeviceInfo>, (StatusCode, Json<Value>)> {
    state
        .devices()
        .await
        .into_iter()
        .find(|d| d.id == id)
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"type":"error", "data": crate::state::not_found()})),
            )
        })
}
pub(crate) async fn adapters(State(state): State<AppState>) -> Json<Vec<AdapterInfo>> {
    Json(state.adapters().await)
}

pub(crate) async fn scan_start(
    State(state): State<AppState>,
) -> Result<Json<ScanStatus>, (StatusCode, Json<Value>)> {
    state.scan(true).await.map(Json).map_err(scan_error)
}
pub(crate) async fn scan_stop(
    State(state): State<AppState>,
) -> Result<Json<ScanStatus>, (StatusCode, Json<Value>)> {
    state.scan(false).await.map(Json).map_err(scan_error)
}
fn scan_error(error: BridgeError) -> (StatusCode, Json<Value>) {
    let status = match error.code {
        ErrorCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
        ErrorCode::Busy => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::DeviceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::UnsupportedOperation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::TrainerControlDenied => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, Json(json!({"type":"error", "data":error})))
}

pub(crate) async fn connect(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    state
        .set_connection(0, &id, true)
        .await
        .map(Json)
        .map_err(scan_error)
}
pub(crate) async fn disconnect(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    state
        .set_connection(0, &id, false)
        .await
        .map(Json)
        .map_err(scan_error)
}

pub(crate) async fn replay_status(
    State(state): State<AppState>,
) -> Json<Option<bikebridge_trace::PlaybackStatus>> {
    Json(state.replay_status().await)
}
pub(crate) async fn replay_action(
    State(state): State<AppState>,
    Path(action): Path<String>,
) -> Result<Json<bikebridge_trace::PlaybackStatus>, (StatusCode, Json<Value>)> {
    state
        .replay_action(&action)
        .await
        .map(Json)
        .map_err(scan_error)
}
