//! OpenBikeControl BLE input bridge, independently implemented from the public protocol.
//! Zwift controllers are paired by BikeControl; this crate consumes its normalized bridge.
mod connection;
pub mod protocol;
pub use connection::Controllers;

use bikebridge_core::Result;
use futures_util::{future::BoxFuture, stream::BoxStream};
use uuid::Uuid;

/// OpenBikeControl primary BLE service.
pub const SERVICE: Uuid = Uuid::from_u128(0xd273f680_d548_419d_b9d1_fa0472345229);
/// Button-state characteristic (Notify; Read is optional in BikeControl).
pub const BUTTON_STATE: Uuid = Uuid::from_u128(0xd273f681_d548_419d_b9d1_fa0472345229);
/// Application identity and supported-action characteristic.
pub const APP_INFO: Uuid = Uuid::from_u128(0xd273f683_d548_419d_b9d1_fa0472345229);

/// Injectable transport with message boundaries supplied by BLE notifications.
pub trait ControllerTransport: Send + Sync + 'static {
    /// Connect, verify service/properties, subscribe, and send app information.
    fn open(&self) -> BoxFuture<'_, Result<BoxStream<'static, Vec<u8>>>>;
    /// Check the platform connection state; silence alone is not disconnection.
    fn is_connected(&self) -> BoxFuture<'_, Result<bool>>;
    /// Release the BLE session, also after failed/cancelled setup.
    fn disconnect(&self) -> BoxFuture<'_, Result<()>>;
}
