use crate::{BridgeError, DeviceInfo, InputData, ScanStatus, TrainerTelemetry};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Domain event carried by the asynchronous bus and public WebSocket protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
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
            Self::Telemetry { device_id, .. } | Self::Input { device_id, .. } => Some(device_id),
            Self::Error { .. } | Self::ScanStatus { .. } => None,
        }
    }
}

/// Bounded, nonblocking fan-out. Slow consumers receive an explicit lag error.
#[derive(Clone, Debug)]
pub struct EventBus(broadcast::Sender<Event>);

impl Default for EventBus {
    fn default() -> Self {
        Self(broadcast::channel(256).0)
    }
}

impl EventBus {
    /// Subscribe to future events; there is no implicit snapshot or replay.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.subscribe()
    }
    /// Publish without waiting for consumers. No listeners is a normal condition.
    pub fn publish(&self, event: Event) {
        let _ = self.0.send(event);
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
