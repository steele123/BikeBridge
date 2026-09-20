use crate::{
    ftms::{RecordAssembler, decode_features},
    transport::{FtmsSession, FtmsTransport},
};
use bikebridge_core::{BridgeError, DeviceInfo, ErrorCode, Event, EventBus, Result, timestamp_ms};
use futures_util::StreamExt;
use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;

#[derive(Clone)]
pub(crate) struct Connections {
    entries: Arc<RwLock<HashMap<String, Entry>>>,
    bus: EventBus,
    stop: CancellationToken,
    tasks: TaskTracker,
}
struct Entry {
    info: Arc<RwLock<DeviceInfo>>,
    transport: Arc<dyn FtmsTransport>,
    commands: Option<mpsc::Sender<Request>>,
}
struct Request {
    connected: bool,
    reply: oneshot::Sender<Result<DeviceInfo>>,
}

impl Connections {
    pub fn new(bus: EventBus) -> Self {
        Self {
            entries: Arc::default(),
            bus,
            stop: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }
    }
    pub async fn register(&self, info: &DeviceInfo, transport: Arc<dyn FtmsTransport>) {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get(&info.id) {
            let mut current = entry.info.write().await;
            current.name.clone_from(&info.name);
            current.signal_strength = info.signal_strength;
        } else {
            entries.insert(
                info.id.clone(),
                Entry {
                    info: Arc::new(RwLock::new(info.clone())),
                    transport,
                    commands: None,
                },
            );
        }
    }
    pub async fn overlay(&self, info: &mut DeviceInfo) {
        if let Some(entry) = self.entries.read().await.get(&info.id) {
            let current = entry.info.read().await;
            info.connected = current.connected;
            info.capabilities.clone_from(&current.capabilities);
        }
    }
    pub async fn publish_discovery(&self, mut event: Event) {
        if let Event::DeviceDiscovered { data } | Event::DeviceUpdated { data } = &mut event
            && let Some(entry) = self.entries.read().await.get(&data.id)
        {
            // Keep publication under the same metadata lock as connection events:
            // a stale advertisement must never follow a newer connection transition.
            let current = entry.info.read().await;
            data.connected = current.connected;
            data.capabilities.clone_from(&current.capabilities);
            self.bus.publish(event);
        } else {
            self.bus.publish(event);
        }
    }
    pub async fn set_connection(&self, id: &str, connected: bool) -> Result<DeviceInfo> {
        let (reply, response) = oneshot::channel();
        {
            let mut entries = self.entries.write().await;
            if self.stop.is_cancelled() {
                return Err(stopped());
            }
            let entry = entries.get_mut(id).ok_or_else(|| {
                BridgeError::new(
                    ErrorCode::UnsupportedOperation,
                    "Only discovered FTMS indoor bikes can connect in Phase 3.",
                )
            })?;
            if entry.commands.is_none() {
                if !connected {
                    return Ok(entry.info.read().await.clone());
                }
                let (commands, requests) = mpsc::channel(4);
                self.tasks.spawn(run(
                    entry.transport.clone(),
                    entry.info.clone(),
                    requests,
                    self.bus.clone(),
                    self.stop.clone(),
                ));
                entry.commands = Some(commands);
            }
            entry
                .commands
                .as_ref()
                .ok_or_else(stopped)?
                .try_send(Request { connected, reply })
                .map_err(|_| {
                    BridgeError::new(
                        ErrorCode::Busy,
                        "Device command queue is unavailable. Retry later.",
                    )
                })?;
        }
        tokio::time::timeout(Duration::from_secs(18), response)
            .await
            .map_err(|_| BridgeError::new(ErrorCode::Timeout, "Device request timed out."))?
            .map_err(|_| stopped())?
    }
    pub async fn shutdown(&self) {
        // Serialize cancellation with session creation before closing the tracker.
        let entries = self.entries.write().await;
        self.stop.cancel();
        self.tasks.close();
        drop(entries);
        self.tasks.wait().await;
    }
}
fn stopped() -> BridgeError {
    BridgeError::new(
        ErrorCode::DeviceDisconnected,
        "BLE sessions are shutting down.",
    )
}
async fn bounded<T>(duration: u64, future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(duration), future)
        .await
        .map_err(|_| BridgeError::new(ErrorCode::Timeout, "BLE session operation timed out."))?
}
async fn mark(info: &RwLock<DeviceInfo>, connected: bool, bus: &EventBus) -> DeviceInfo {
    let mut info = info.write().await;
    let changed = info.connected != connected;
    info.connected = connected;
    let data = info.clone();
    if changed {
        tracing::info!(device_id = %data.id, connected, "BLE connection changed");
        bus.publish(if connected {
            Event::DeviceConnected { data: data.clone() }
        } else {
            Event::DeviceDisconnected { data: data.clone() }
        });
    }
    data
}
async fn cleanup(
    transport: &dyn FtmsTransport,
    info: &RwLock<DeviceInfo>,
    bus: &EventBus,
) -> Result<DeviceInfo> {
    let result = bounded(3, transport.disconnect()).await;
    // connected means usable telemetry session; clear it even when OS teardown fails.
    let data = mark(info, false, bus).await;
    if let Err(error) = &result {
        bus.publish(Event::Error {
            data: error.clone(),
        });
    }
    result.map(|()| data)
}
async fn run(
    transport: Arc<dyn FtmsTransport>,
    info: Arc<RwLock<DeviceInfo>>,
    mut requests: mpsc::Receiver<Request>,
    bus: EventBus,
    stop: CancellationToken,
) {
    let id = info.read().await.id.clone();
    let mut session: Option<FtmsSession> = None;
    let mut assembler = RecordAssembler::default();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_parse_error = None;
    // Failed teardown must be retried before reopening the same peripheral.
    let mut needs_cleanup = false;
    loop {
        if stop.is_cancelled() {
            break;
        }
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            request = requests.recv() => {
                let Some(mut request) = request else {break};
                if request.reply.is_closed() {continue;}
                let result = if request.connected {
                    if session.is_some() { Ok(info.read().await.clone()) }
                    else {
                        let opened = tokio::select! {
                            _ = stop.cancelled() => Err(stopped()),
                            _ = request.reply.closed() => Err(BridgeError::new(ErrorCode::DeviceDisconnected,"Connection requester left before setup completed.")),
                            result = bounded(10,async {
                                if needs_cleanup { transport.disconnect().await?; }
                                let opened = transport.open().await?;
                                let capabilities = decode_features(&opened.features)?;
                                Ok((opened,capabilities))
                            }) => result,
                        };
                        match opened {
                            Ok((opened,capabilities)) => {
                                info.write().await.capabilities = capabilities;
                                session = Some(opened); needs_cleanup = true;
                                assembler = RecordAssembler::default(); last_parse_error = None;
                                Ok(mark(&info,true,&bus).await)
                            }
                            Err(error) => {
                                needs_cleanup = cleanup(transport.as_ref(),&info,&bus).await.is_err();
                                bus.publish(Event::Error {data:error.clone()});
                                Err(error)
                            }
                        }
                    }
                } else if session.take().is_some() || needs_cleanup {
                    assembler = RecordAssembler::default();
                    let result = cleanup(transport.as_ref(),&info,&bus).await;
                    needs_cleanup = result.is_err(); result
                } else { Ok(info.read().await.clone()) };
                let _ = request.reply.send(result);
            }
            notification = async { match &mut session { Some(active) => active.notifications.next().await, None => std::future::pending().await } } => {
                match notification {
                    Some(bytes) => match assembler.push(&bytes,tokio::time::Instant::now(),timestamp_ms()) {
                        Ok(Some(data)) => { bus.publish(Event::Telemetry { device_id:id.clone(),data }); }
                        Ok(None) => {},
                        Err(error) => {
                            let now = tokio::time::Instant::now();
                            if last_parse_error.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(5)) {
                                tracing::warn!(device_id = %id, message = %error.message, "Invalid FTMS measurement dropped");
                                bus.publish(Event::Error {data:error}); last_parse_error = Some(now);
                            }
                        }
                    },
                    None => {
                        session = None; assembler = RecordAssembler::default();
                        bus.publish(Event::Error { data:BridgeError::new(ErrorCode::DeviceDisconnected,"FTMS notification stream ended. Reconnect the trainer explicitly.") });
                        needs_cleanup = cleanup(transport.as_ref(),&info,&bus).await.is_err();
                    }
                }
            }
            _ = interval.tick(), if session.is_some() => {
                let state = tokio::select! {
                    _ = stop.cancelled() => break,
                    state = bounded(3,transport.is_connected()) => state,
                };
                if !matches!(state,Ok(true)) {
                    session = None; assembler = RecordAssembler::default();
                    let error = state.err().unwrap_or_else(|| BridgeError::new(ErrorCode::DeviceDisconnected,"Trainer disconnected. Reconnect it explicitly."));
                    bus.publish(Event::Error {data:error});
                    needs_cleanup = cleanup(transport.as_ref(),&info,&bus).await.is_err();
                }
            }
        }
    }
    drop(session);
    if needs_cleanup {
        let _ = cleanup(transport.as_ref(), &info, &bus).await;
    }
}
