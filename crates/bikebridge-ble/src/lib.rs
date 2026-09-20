//! Cross-platform discovery, FTMS trainer sessions, and OpenBikeControl controller input.
pub mod backend;
pub mod classification;
pub mod click;
mod click_transport;
mod connection;
pub mod control;
mod controller_transport;
pub mod cycling_power;
pub mod ftms;
mod registry;
mod scanner;
mod session;
pub mod transport;

pub use backend::{Advertisement, BackendAdapter, DiscoveryBackend, NativeBackend};
pub use scanner::{DiscoverySnapshot, Scanner};
