//! Injectable read-only FTMS transport. No trainer control writes are available here.
use crate::classification::FITNESS_MACHINE;
use bikebridge_core::{BridgeError, ErrorCode, Result};
use btleplug::{
    api::{CharPropFlags, Peripheral as _},
    platform::Peripheral,
};
use futures_util::{StreamExt, future::BoxFuture, stream::BoxStream};
use uuid::Uuid;

/// Fitness Machine Feature characteristic (0x2ACC).
pub const FITNESS_MACHINE_FEATURE: Uuid = Uuid::from_u128(0x00002acc_0000_1000_8000_00805f9b34fb);
/// Indoor Bike Data characteristic (0x2AD2).
pub const INDOOR_BIKE_DATA: Uuid = Uuid::from_u128(0x00002ad2_0000_1000_8000_00805f9b34fb);

/// A subscribed transport, with raw values confined to the BLE layer.
pub struct FtmsSession {
    /// Eight-byte Fitness Machine Feature value.
    pub features: Vec<u8>,
    /// Only Indoor Bike Data notifications; closing means the session was lost.
    pub notifications: BoxStream<'static, Vec<u8>>,
}

/// Per-device I/O interface for native transports and hardware-free fixtures.
/// Callers bound every operation and always disconnect after a partially failed open.
pub trait FtmsTransport: Send + Sync + 'static {
    /// Connect, discover FTMS characteristics, read features, and subscribe to Indoor Bike Data.
    fn open(&self) -> BoxFuture<'_, Result<FtmsSession>>;
    /// Check the OS connection state, even if notifications have stopped.
    fn is_connected(&self) -> BoxFuture<'_, Result<bool>>;
    /// Disconnect and release the subscription, including after a failed open.
    fn disconnect(&self) -> BoxFuture<'_, Result<()>>;
}

pub(crate) struct NativeTransport(pub Peripheral);

fn connection_error(error: btleplug::Error) -> BridgeError {
    tracing::debug!(%error, "BLE session operation failed");
    BridgeError::new(
        ErrorCode::ConnectionFailed,
        "BLE session operation failed. Check that the trainer is awake and available; debug logs contain details.",
    )
}
fn unsupported() -> BridgeError {
    BridgeError::new(
        ErrorCode::UnsupportedOperation,
        "Device must provide readable FTMS features and notifying Indoor Bike Data.",
    )
}

impl FtmsTransport for NativeTransport {
    fn open(&self) -> BoxFuture<'_, Result<FtmsSession>> {
        Box::pin(async move {
            if !self.0.is_connected().await.map_err(connection_error)? {
                self.0.connect().await.map_err(connection_error)?;
            }
            self.0.discover_services().await.map_err(connection_error)?;
            let characteristics = self.0.characteristics();
            let feature = characteristics
                .iter()
                .find(|c| {
                    c.service_uuid == FITNESS_MACHINE
                        && c.uuid == FITNESS_MACHINE_FEATURE
                        && c.properties.contains(CharPropFlags::READ)
                })
                .ok_or_else(unsupported)?;
            let data = characteristics
                .iter()
                .find(|c| {
                    c.service_uuid == FITNESS_MACHINE
                        && c.uuid == INDOOR_BIKE_DATA
                        && c.properties.contains(CharPropFlags::NOTIFY)
                })
                .ok_or_else(unsupported)?;
            let features = self.0.read(feature).await.map_err(connection_error)?;
            crate::ftms::decode_features(&features)?;
            // Install the receiver before subscribing, so the first measurement is not lost.
            let notifications = self.0.notifications().await.map_err(connection_error)?;
            self.0.subscribe(data).await.map_err(connection_error)?;
            let notifications = notifications
                .filter_map(|notification| async move {
                    (notification.uuid == INDOOR_BIKE_DATA).then_some(notification.value)
                })
                .boxed();
            Ok(FtmsSession {
                features,
                notifications,
            })
        })
    }
    fn is_connected(&self) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move { self.0.is_connected().await.map_err(connection_error) })
    }
    fn disconnect(&self) -> BoxFuture<'_, Result<()>> {
        // btleplug tears down the GATT session and its subscription on disconnect.
        // Do not let a failed CCCD unsubscribe prevent the disconnect attempt.
        Box::pin(async move { self.0.disconnect().await.map_err(connection_error) })
    }
}
