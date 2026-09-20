use crate::click::{self, Side};
use bikebridge_core::{BridgeError, ErrorCode, InputData, Result};
use bikebridge_openbikecontrol::ControllerTransport;
use btleplug::{
    api::{CharPropFlags, Peripheral as _, WriteType},
    platform::Peripheral,
};
use futures_util::{
    StreamExt,
    future::BoxFuture,
    stream::{self, BoxStream},
};

pub(crate) struct NativeClick {
    pub peripheral: Peripheral,
    pub side: Side,
}
fn platform(error: btleplug::Error) -> BridgeError {
    tracing::debug!(%error, "Click V2 Bluetooth operation failed");
    BridgeError::new(
        ErrorCode::ConnectionFailed,
        "Click V2 connection failed. Wake the controller and close other apps connected to it.",
    )
}
fn unsupported() -> BridgeError {
    BridgeError::new(
        ErrorCode::UnsupportedOperation,
        "Click V2 requires the Zwift notification, indication, and writable handshake characteristics.",
    )
}
impl ControllerTransport for NativeClick {
    fn decode(&self, bytes: &[u8]) -> Result<Vec<InputData>> {
        click::decode(self.side, bytes)
    }
    fn open(&self) -> BoxFuture<'_, Result<BoxStream<'static, Vec<u8>>>> {
        Box::pin(async move {
            if !self.peripheral.is_connected().await.map_err(platform)? {
                self.peripheral.connect().await.map_err(platform)?;
            }
            self.peripheral
                .discover_services()
                .await
                .map_err(platform)?;
            let chars = self.peripheral.characteristics();
            let channel = [click::SERVICE, click::LEGACY_SERVICE]
                .into_iter()
                .find_map(|service| {
                    let notify = chars.iter().find(|c| {
                        c.service_uuid == service
                            && c.uuid == click::NOTIFY
                            && c.properties.contains(CharPropFlags::NOTIFY)
                    })?;
                    let indicate = chars.iter().find(|c| {
                        c.service_uuid == service
                            && c.uuid == click::INDICATE
                            && c.properties
                                .intersects(CharPropFlags::INDICATE | CharPropFlags::NOTIFY)
                    })?;
                    let write = chars.iter().find(|c| {
                        c.service_uuid == service
                            && c.uuid == click::WRITE
                            && c.properties.intersects(
                                CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE,
                            )
                    })?;
                    Some((notify, indicate, write))
                })
                .ok_or_else(unsupported)?;
            let mut notifications = self.peripheral.notifications().await.map_err(platform)?;
            self.peripheral
                .subscribe(channel.0)
                .await
                .map_err(platform)?;
            self.peripheral
                .subscribe(channel.1)
                .await
                .map_err(platform)?;
            let mode = if channel
                .2
                .properties
                .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
            {
                WriteType::WithoutResponse
            } else {
                WriteType::WithResponse
            };
            self.peripheral
                .write(channel.2, click::HANDSHAKE, mode)
                .await
                .map_err(platform)?;
            // Do not advertise a successful connection solely because a write succeeded.
            // A controller must acknowledge this protocol or emit a valid button snapshot.
            let first = tokio::time::timeout(std::time::Duration::from_secs(6), async {
                let mut count = 0;
                while let Some(packet) = notifications.next().await {
                    if packet.uuid != click::NOTIFY && packet.uuid != click::INDICATE { continue; }
                    count += 1;
                    if count > 1000 { return Err(BridgeError::new(ErrorCode::InvalidDeviceData, "Click V2 exceeded the handshake packet limit.")); }
                    let decoded = click::decode(self.side, &packet.value)?;
                    if packet.value == click::HANDSHAKE || !decoded.is_empty() { return Ok(packet.value); }
                    if packet.value.starts_with(b"RideOn") { return Err(BridgeError::new(ErrorCode::UnsupportedOperation, "Click V2 returned an unsupported handshake version.")); }
                }
                Err(BridgeError::new(ErrorCode::DeviceDisconnected, "Click V2 disconnected during setup."))
            }).await.map_err(|_| BridgeError::new(ErrorCode::Timeout,
                "Click V2 did not acknowledge setup. Wake both controllers; if necessary enable them in the Zwift game, close Zwift, and reconnect."))??;
            Ok(stream::once(async move { first })
                .chain(notifications.filter_map(|packet| async move {
                    (packet.uuid == click::NOTIFY || packet.uuid == click::INDICATE)
                        .then_some(packet.value)
                }))
                .boxed())
        })
    }
    fn is_connected(&self) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move { self.peripheral.is_connected().await.map_err(platform) })
    }
    fn disconnect(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move { self.peripheral.disconnect().await.map_err(platform) })
    }
}
