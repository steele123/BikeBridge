use crate::{BridgeError, DeviceInfo, InputData, ScanStatus, TrainerCommand, TrainerTelemetry};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// Domain event carried by the asynchronous bus and public WebSocket protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    /// A parsed trainer command about to execute, before clamping or hardware I/O.
    #[serde(rename = "command.started")]
    CommandStarted {
        /// Process-local correlation ID.
        #[serde(rename = "commandId")]
        command_id: u64,
        /// Process-local WebSocket session ID.
        #[serde(rename = "sessionId")]
        session_id: u64,
        /// Target device.
        #[serde(rename = "deviceId")]
        device_id: String,
        /// Requested operation.
        command: TrainerCommand,
    },
    /// Final outcome, including cancellation with an uncertain hardware result.
    #[serde(rename = "command.finished")]
    CommandFinished {
        /// Matching command.started ID.
        #[serde(rename = "commandId")]
        command_id: u64,
        /// Target device.
        #[serde(rename = "deviceId")]
        device_id: String,
        /// Confirmed target, domain error, or cancellation.
        outcome: CommandOutcome,
    },
    /// A live API client left; emitted before its load cleanup.
    #[serde(rename = "session.disconnected")]
    SessionDisconnected {
        /// Process-local WebSocket session ID.
        #[serde(rename = "sessionId")]
        session_id: u64,
    },
    /// Replay was rewound. Clients should replace their device snapshot.
    #[serde(rename = "replay.reset")]
    ReplayReset {
        /// Initial device state from the trace header.
        devices: Vec<DeviceInfo>,
    },
    /// A previously connected device is scheduled for another connection attempt.
    #[serde(rename = "device.reconnecting")]
    DeviceReconnecting {
        /// Opaque BikeBridge device ID.
        #[serde(rename = "deviceId")]
        device_id: String,
        /// One-based retry attempt, bounded to five attempts per link loss.
        attempt: u8,
        /// Seconds until this attempt starts.
        #[serde(rename = "delaySeconds")]
        delay_seconds: u64,
    },
    /// A device became available.
    #[serde(rename = "device.discovered")]
    DeviceDiscovered {
        /// Device snapshot.
        data: DeviceInfo,
    },
    /// Previously discovered device metadata changed.
    #[serde(rename = "device.updated")]
    DeviceUpdated {
        /// Latest device snapshot.
        data: DeviceInfo,
    },
    /// Scanning started, stopped, or failed.
    #[serde(rename = "scan.status")]
    ScanStatus {
        /// Scan state and optional failure.
        data: ScanStatus,
    },
    /// A device connected.
    #[serde(rename = "device.connected")]
    DeviceConnected {
        /// Device snapshot.
        data: DeviceInfo,
    },
    /// A device disconnected.
    #[serde(rename = "device.disconnected")]
    DeviceDisconnected {
        /// Device snapshot.
        data: DeviceInfo,
    },
    /// Per-device measurement.
    #[serde(rename = "telemetry")]
    Telemetry {
        /// Origin identity.
        #[serde(rename = "deviceId")]
        device_id: String,
        /// Measurement sample.
        data: TrainerTelemetry,
    },
    /// Normalized controller input.
    #[serde(rename = "input")]
    Input {
        /// Origin identity.
        #[serde(rename = "deviceId")]
        device_id: String,
        /// Input payload.
        data: InputData,
        /// Unix time in milliseconds.
        #[serde(rename = "timestampMs")]
        timestamp_ms: u64,
    },
    /// Public error.
    #[serde(rename = "error")]
    Error {
        /// Error description.
        data: BridgeError,
    },
}

impl Event {
    /// Subscription category.
    pub fn category(&self) -> &'static str {
        match self {
            Self::DeviceDiscovered { .. }
            | Self::DeviceUpdated { .. }
            | Self::DeviceConnected { .. }
            | Self::DeviceDisconnected { .. } => "device",
            Self::DeviceReconnecting { .. } => "device",
            Self::CommandStarted { .. } | Self::CommandFinished { .. } => "command",
            Self::SessionDisconnected { .. } => "session",
            Self::ReplayReset { .. } => "replay",
            Self::Telemetry { .. } => "telemetry",
            Self::Input { .. } => "input",
            Self::Error { .. } => "error",
            Self::ScanStatus { .. } => "scan",
        }
    }
    /// Origin identity, if the event is device-specific.
    pub fn device_id(&self) -> Option<&str> {
        match self {
            Self::DeviceDiscovered { data }
            | Self::DeviceUpdated { data }
            | Self::DeviceConnected { data }
            | Self::DeviceDisconnected { data } => Some(&data.id),
            Self::Telemetry { device_id, .. }
            | Self::Input { device_id, .. }
            | Self::DeviceReconnecting { device_id, .. }
            | Self::CommandStarted { device_id, .. }
            | Self::CommandFinished { device_id, .. } => Some(device_id),
            Self::Error { .. }
            | Self::ScanStatus { .. }
            | Self::SessionDisconnected { .. }
            | Self::ReplayReset { .. } => None,
        }
    }
}

/// Result of a trainer command at the developer API boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandOutcome {
    /// Accepted/confirmed target (resistance may still be ramping).
    Applied {
        /// Safety-clamped effective command.
        command: TrainerCommand,
    },
    /// Command failed.
    Failed {
        /// Public failure.
        error: BridgeError,
    },
    /// Caller left or shutdown interrupted execution; no success is inferred.
    Cancelled,
}

/// Nonblocking observer invoked in publication order, before WebSocket fan-out.
/// Implementations must not call back into this bus or perform blocking I/O.
pub trait EventObserver: Send + Sync + std::fmt::Debug {
    /// Copy/enqueue an event; overflow must be reported by the observer.
    fn observe(&self, event: &Event);
}

#[derive(Debug)]
struct BusInner {
    sender: broadcast::Sender<Event>,
    observer: Mutex<Option<Arc<dyn EventObserver>>>,
}
/// Bounded fan-out with an optional ordered recording observer.
#[derive(Clone, Debug)]
pub struct EventBus(Arc<BusInner>);

impl Default for EventBus {
    fn default() -> Self {
        Self(Arc::new(BusInner {
            sender: broadcast::channel(256).0,
            observer: Mutex::new(None),
        }))
    }
}

impl EventBus {
    /// Install one recorder; fails rather than replacing an active capture.
    pub fn observe(&self, observer: Arc<dyn EventObserver>) -> crate::Result<()> {
        let mut slot = self.0.observer.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err(BridgeError::new(
                crate::ErrorCode::Busy,
                "An event recorder is already attached.",
            ));
        }
        *slot = Some(observer);
        Ok(())
    }
    /// Remove the observer after all prior publications have been captured.
    pub fn stop_observing(&self) {
        self.0
            .observer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
    /// Subscribe to future events; there is no implicit snapshot or replay.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.sender.subscribe()
    }
    /// Publish in one order to the recorder and subscribers, without waiting for disk I/O.
    pub fn publish(&self, event: Event) {
        let observer = self.0.observer.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(observer) = &*observer {
            observer.observe(&event);
        }
        let _ = self.0.sender.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn telemetry_wire_shape_is_stable() {
        let event = Event::Telemetry {
            device_id: "mock-trainer".into(),
            data: TrainerTelemetry {
                power_watts: Some(243),
                timestamp_ms: 1,
                ..Default::default()
            },
        };
        let json = serde_json::to_value(&event).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({"type":"telemetry","deviceId":"mock-trainer","data":{"powerWatts":243,"timestampMs":1}})
        );
        assert_eq!(
            serde_json::from_value::<Event>(json).expect("deserialize"),
            event
        );
    }
}
