//! Public discovery models without platform handles, addresses, or GATT UUIDs.
use crate::BridgeError;
use serde::{Deserialize, Serialize};

/// A Bluetooth scan result, including devices whose cycling support is unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyDevice {
    /// Opaque selection identity, scoped to this daemon session.
    pub id: String,
    /// Bluetooth display name, when supplied by the device or operating system.
    pub name: Option<String>,
    /// Cached received signal strength in dBm.
    pub signal_strength: Option<i16>,
    /// Corresponding cycling-device identity, after discovery or explicit selection.
    pub device_id: Option<String>,
}

/// Platform-reported radio power state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterState {
    /// Radio is enabled.
    PoweredOn,
    /// Radio is disabled.
    PoweredOff,
    /// Platform cannot determine radio state.
    Unknown,
}

/// Public adapter metadata; IDs are opaque and scoped to the daemon session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterInfo {
    /// BikeBridge identity; never an OS address or path.
    pub id: String,
    /// Generic display label, avoiding platform identifiers.
    pub name: String,
    /// Last observed radio state.
    pub state: AdapterState,
    /// Whether this adapter will be used for the next scan.
    pub is_default: bool,
}

/// Current discovery state. Scan failure is distinct from an empty device list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    /// True while scanning, or if stopping a scan failed and activity is uncertain.
    pub scanning: bool,
    /// Selected adapter identity, when one was found.
    pub adapter_id: Option<String>,
    /// Last discovery failure; cleared by a successful start or stopping an active scan.
    pub last_error: Option<BridgeError>,
}
