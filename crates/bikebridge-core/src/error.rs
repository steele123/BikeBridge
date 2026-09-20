use serde::{Deserialize, Serialize};

/// Stable public error codes; implementations may add codes in later versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No usable Bluetooth backend.
    BluetoothUnavailable,
    /// No adapter exists, or the selected adapter is no longer present.
    AdapterNotFound,
    /// The platform could not start, stop, or read a scan.
    ScanFailed,
    /// A bounded command queue is full.
    Busy,
    /// Unknown device identity.
    DeviceNotFound,
    /// Device is not connected.
    DeviceDisconnected,
    /// A BLE connection, discovery, subscription, or disconnect failed.
    ConnectionFailed,
    /// A device returned a malformed or unsupported measurement encoding.
    InvalidDeviceData,
    /// The device cannot perform this operation.
    UnsupportedOperation,
    /// Another client owns control.
    TrainerControlDenied,
    /// A trainer rejected a control transaction or its outcome is uncertain.
    TrainerControlFailed,
    /// Invalid message structure.
    InvalidCommand,
    /// Value is non-finite or out of range for an injection.
    InvalidValue,
    /// A bounded operation timed out.
    Timeout,
    /// Subscriber fell behind the bounded event bus.
    EventsLost,
    /// Unspecified internal failure, without implementation details.
    InternalError,
}

/// Safe-to-publish domain error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct BridgeError {
    /// Machine-readable failure category.
    pub code: ErrorCode,
    /// Human-readable context, without private transport details.
    pub message: String,
}

impl BridgeError {
    /// Create an application error.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// BikeBridge domain result.
pub type Result<T> = std::result::Result<T, BridgeError>;
