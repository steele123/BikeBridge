use serde::{Deserialize, Serialize};

/// Broad hardware role, independent of its transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// Controllable indoor trainer.
    Trainer,
    /// Heart rate sensor.
    HeartRateMonitor,
    /// Crank or wheel revolution sensor.
    CadenceSensor,
    /// Power meter.
    PowerMeter,
    /// Buttons or steering controls.
    BikeController,
    /// Unclassified device.
    Unknown,
}

/// Features actually supported by a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceCapability {
    /// Power measurements in watts.
    Power,
    /// Cadence measurements in RPM.
    Cadence,
    /// Speed measurements in km/h.
    Speed,
    /// Heart rate measurements in BPM.
    HeartRate,
    /// Normalized resistance control.
    ResistanceControl,
    /// Target power control.
    ErgControl,
    /// Simulation parameters.
    SimulationControl,
    /// Shift button inputs.
    ShiftButtons,
    /// Steering inputs.
    Steering,
}

/// Public device record. IDs are opaque BikeBridge identities, not BLE addresses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// Opaque identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Primary device role.
    pub kind: DeviceKind,
    /// Transport label (currently `mock`).
    pub transport: String,
    /// Whether telemetry and commands are available.
    pub connected: bool,
    /// RSSI in dBm, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_strength: Option<i16>,
    /// Supported measurements and controls.
    pub capabilities: Vec<DeviceCapability>,
}
