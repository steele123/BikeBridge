use serde::{Deserialize, Serialize};

/// Partial measurement sample. Missing fields mean unavailable, never zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrainerTelemetry {
    /// Instantaneous power; signed to preserve standard sensor measurements.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_watts: Option<i16>,
    /// Average power over the device's measurement period, in watts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_power_watts: Option<i16>,
    /// Crank speed in RPM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cadence_rpm: Option<f32>,
    /// Speed in km/h.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_kph: Option<f32>,
    /// Heart rate in BPM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heart_rate_bpm: Option<u16>,
    /// Accumulated distance in meters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance_meters: Option<f64>,
    /// Elapsed workout time reported by the device, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_time_seconds: Option<u16>,
    /// Applied normalized resistance, from 0 to 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resistance_level: Option<f32>,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: u64,
}
