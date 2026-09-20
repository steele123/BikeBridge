//! Cross-platform BLE discovery and read-only FTMS trainer sessions.
pub mod backend;
pub mod classification;
mod connection;
pub mod ftms;
mod registry;
mod scanner;
pub mod transport;

pub use backend::{Advertisement, BackendAdapter, DiscoveryBackend, NativeBackend};
pub use scanner::{DiscoverySnapshot, Scanner};
