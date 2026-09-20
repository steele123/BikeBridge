use bikebridge_core::{BridgeError, ErrorCode, Result};
use bikebridge_openbikecontrol::{APP_INFO, BUTTON_STATE, ControllerTransport, SERVICE, protocol};
use btleplug::{
    api::{CharPropFlags, Peripheral as _, WriteType},
    platform::Peripheral,
};
use futures_util::{StreamExt, future::BoxFuture, stream::BoxStream};

pub(crate) struct NativeController(pub Peripheral);
fn transport_error(error: btleplug::Error) -> BridgeError {
    tracing::debug!(%error,"OpenBikeControl BLE operation failed");
    BridgeError::new(
        ErrorCode::ConnectionFailed,
        "OpenBikeControl bridge connection failed. Check BikeControl's Bluetooth bridge and OS permissions.",
    )
}
fn unsupported() -> BridgeError {
    BridgeError::new(
        ErrorCode::UnsupportedOperation,
        "Controller must provide OpenBikeControl button notifications and writable app information.",
    )
}
impl ControllerTransport for NativeController {
    fn open(&self) -> BoxFuture<'_, Result<BoxStream<'static, Vec<u8>>>> {
        Box::pin(async move {
            if !self.0.is_connected().await.map_err(transport_error)? {
                self.0.connect().await.map_err(transport_error)?;
            }
            self.0.discover_services().await.map_err(transport_error)?;
            let characteristics = self.0.characteristics();
            let buttons = characteristics
                .iter()
                .find(|c| {
                    c.service_uuid == SERVICE
                        && c.uuid == BUTTON_STATE
                        && c.properties.contains(CharPropFlags::NOTIFY)
                })
                .ok_or_else(unsupported)?;
            let app = characteristics
                .iter()
                .find(|c| {
                    c.service_uuid == SERVICE
                        && c.uuid == APP_INFO
                        && c.properties.intersects(
                            CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE,
                        )
                })
                .ok_or_else(unsupported)?;
            let notifications = self.0.notifications().await.map_err(transport_error)?;
            self.0.subscribe(buttons).await.map_err(transport_error)?;
            let mode = if app.properties.contains(CharPropFlags::WRITE) {
                WriteType::WithResponse
            } else {
                WriteType::WithoutResponse
            };
            // BikeControl's AppInfoReassembler accepts consecutive 20-byte chunks. Do not rely on an oversized ATT write.
            // Subscribe first: forwarding is enabled as soon as BikeControl has received the complete app-info value.
            for chunk in protocol::app_info().chunks(20) {
                self.0
                    .write(app, chunk, mode)
                    .await
                    .map_err(transport_error)?;
            }
            Ok(notifications
                .filter_map(|packet| async move {
                    (packet.uuid == BUTTON_STATE).then_some(packet.value)
                })
                .boxed())
        })
    }
    fn is_connected(&self) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move { self.0.is_connected().await.map_err(transport_error) })
    }
    fn disconnect(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move { self.0.disconnect().await.map_err(transport_error) })
    }
}
