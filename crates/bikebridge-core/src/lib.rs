//! Hardware-independent models, commands, safety limits, and events for BikeBridge.
/// Trainer commands and configurable safety limits.
pub mod command;
/// Public device identities and capabilities.
pub mod device;
/// Transport-neutral discovery status and adapter descriptions.
pub mod discovery;
/// Stable public errors.
pub mod error;
/// Asynchronous domain events.
pub mod event;
/// Normalized controller inputs.
pub mod input;
/// Measurement payloads.
pub mod telemetry;
/// Transport-independent trainer interface.
pub mod trainer;

pub use command::*;
pub use device::*;
pub use discovery::*;
pub use error::*;
pub use event::*;
pub use input::*;
pub use telemetry::*;
pub use trainer::*;

/// Unix time in milliseconds; zero if the system clock precedes the Unix epoch.
pub fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}
