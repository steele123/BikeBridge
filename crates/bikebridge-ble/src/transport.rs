//! Injectable FTMS/Cycling Power transport. Raw GATT operations stay inside this crate.
use crate::classification::{CYCLING_POWER, FITNESS_MACHINE};
use crate::control::ControlProfile;
use bikebridge_core::{BridgeError, ErrorCode, Result};
use btleplug::{
    api::{CharPropFlags, Peripheral as _, WriteType},
    platform::Peripheral,
};
use futures_util::{StreamExt, future::BoxFuture, stream::BoxStream};
use uuid::Uuid;

/// Fitness Machine Feature characteristic (0x2ACC).
pub const FITNESS_MACHINE_FEATURE: Uuid = Uuid::from_u128(0x00002acc_0000_1000_8000_00805f9b34fb);
/// Indoor Bike Data characteristic (0x2AD2).
pub const INDOOR_BIKE_DATA: Uuid = Uuid::from_u128(0x00002ad2_0000_1000_8000_00805f9b34fb);
/// Fitness Machine Control Point (0x2AD9).
pub const CONTROL_POINT: Uuid = Uuid::from_u128(0x00002ad9_0000_1000_8000_00805f9b34fb);
/// Fitness Machine Status (0x2ADA).
pub const MACHINE_STATUS: Uuid = Uuid::from_u128(0x00002ada_0000_1000_8000_00805f9b34fb);
/// Supported Resistance Level Range (0x2AD6).
pub const RESISTANCE_RANGE: Uuid = Uuid::from_u128(0x00002ad6_0000_1000_8000_00805f9b34fb);
/// Supported Power Range (0x2AD8).
pub const POWER_RANGE: Uuid = Uuid::from_u128(0x00002ad8_0000_1000_8000_00805f9b34fb);
/// Cycling Power Measurement (0x2A63).
pub const CYCLING_POWER_MEASUREMENT: Uuid = Uuid::from_u128(0x00002a63_0000_1000_8000_00805f9b34fb);
/// Cycling Power Feature (0x2A65).
pub const CYCLING_POWER_FEATURE: Uuid = Uuid::from_u128(0x00002a65_0000_1000_8000_00805f9b34fb);

/// Verified measurement protocol for a connected cycling device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TelemetryFormat {
    /// FTMS Indoor Bike Data and eight-byte Fitness Machine Feature.
    Ftms,
    /// Cycling Power Measurement and four-byte Cycling Power Feature.
    CyclingPower,
}

/// Ordered control indications and machine-status notifications from a single receiver.
pub enum ControlEvent {
    /// Control Point procedure result.
    Indication(Vec<u8>),
    /// Fitness Machine Status update.
    Status(Vec<u8>),
}
/// Optional control connection; telemetry-only trainers need not provide this.
pub struct ControlSession {
    /// Verified target features and ranges.
    pub profile: ControlProfile,
    /// Control results and status updates, in transport order.
    pub events: BoxStream<'static, ControlEvent>,
}

/// A subscribed transport, with raw values confined to the BLE layer.
pub struct FtmsSession {
    /// Selects the decoder for features and notifications; CPS never enables FTMS control.
    pub format: TelemetryFormat,
    /// Available only after subscribing to both control indications and machine status.
    pub control: Option<ControlSession>,
    /// Feature value for the verified measurement protocol.
    pub features: Vec<u8>,
    /// Only the selected measurement notifications; closing means the session was lost.
    pub notifications: BoxStream<'static, Vec<u8>>,
}

/// Per-device I/O interface for native transports and hardware-free fixtures.
/// Callers bound every operation and always disconnect after a partially failed open.
pub trait FtmsTransport: Send + Sync + 'static {
    /// Write a Control Point procedure with GATT response. The matching indication is separate.
    fn write_control<'a>(&'a self, _bytes: &'a [u8]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async {
            Err(crate::control::unsupported(
                "Trainer has no writable FTMS control point.",
            ))
        })
    }
    /// Verify FTMS or Cycling Power features and subscribe to the matching measurements.
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
        "Device must provide readable FTMS or Cycling Power features and matching measurement notifications.",
    )
}

impl FtmsTransport for NativeTransport {
    fn write_control<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let point = self
                .0
                .characteristics()
                .into_iter()
                .find(|c| {
                    c.uuid == CONTROL_POINT
                        && c.service_uuid == FITNESS_MACHINE
                        && c.properties
                            .contains(CharPropFlags::WRITE | CharPropFlags::INDICATE)
                })
                .ok_or_else(unsupported)?;
            self.0
                .write(&point, bytes, WriteType::WithResponse)
                .await
                .map_err(connection_error)
        })
    }
    fn open(&self) -> BoxFuture<'_, Result<FtmsSession>> {
        Box::pin(async move {
            if !self.0.is_connected().await.map_err(connection_error)? {
                self.0.connect().await.map_err(connection_error)?;
            }
            self.0.discover_services().await.map_err(connection_error)?;
            let characteristics = self.0.characteristics();
            let ftms_available = characteristics.iter().any(|c| {
                c.service_uuid == FITNESS_MACHINE
                    && c.uuid == FITNESS_MACHINE_FEATURE
                    && c.properties.contains(CharPropFlags::READ)
            }) && characteristics.iter().any(|c| {
                c.service_uuid == FITNESS_MACHINE
                    && c.uuid == INDOOR_BIKE_DATA
                    && c.properties.contains(CharPropFlags::NOTIFY)
            });
            if !ftms_available {
                let feature = characteristics
                    .iter()
                    .find(|c| {
                        c.service_uuid == CYCLING_POWER
                            && c.uuid == CYCLING_POWER_FEATURE
                            && c.properties.contains(CharPropFlags::READ)
                    })
                    .ok_or_else(unsupported)?;
                let data = characteristics
                    .iter()
                    .find(|c| {
                        c.service_uuid == CYCLING_POWER
                            && c.uuid == CYCLING_POWER_MEASUREMENT
                            && c.properties.contains(CharPropFlags::NOTIFY)
                    })
                    .ok_or_else(unsupported)?;
                let features = self.0.read(feature).await.map_err(connection_error)?;
                crate::cycling_power::decode_features(&features)?;
                let notifications = self.0.notifications().await.map_err(connection_error)?;
                self.0.subscribe(data).await.map_err(connection_error)?;
                return Ok(FtmsSession {
                    format: TelemetryFormat::CyclingPower,
                    control: None,
                    features,
                    notifications: notifications
                        .filter_map(|event| async move {
                            (event.uuid == CYCLING_POWER_MEASUREMENT).then_some(event.value)
                        })
                        .boxed(),
                });
            }
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
            let point = characteristics.iter().find(|c| {
                c.service_uuid == FITNESS_MACHINE
                    && c.uuid == CONTROL_POINT
                    && c.properties
                        .contains(CharPropFlags::WRITE | CharPropFlags::INDICATE)
            });
            let status = characteristics.iter().find(|c| {
                c.service_uuid == FITNESS_MACHINE
                    && c.uuid == MACHINE_STATUS
                    && c.properties.contains(CharPropFlags::NOTIFY)
            });
            let control = if let (Some(point), Some(status)) = (point, status) {
                let mut resistance = None;
                let mut power = None;
                for (uuid, value) in [
                    (RESISTANCE_RANGE, &mut resistance),
                    (POWER_RANGE, &mut power),
                ] {
                    if let Some(c) = characteristics.iter().find(|c| {
                        c.service_uuid == FITNESS_MACHINE
                            && c.uuid == uuid
                            && c.properties.contains(CharPropFlags::READ)
                    }) {
                        match self.0.read(c).await {
                            Ok(bytes) => *value = Some(bytes),
                            Err(error) => {
                                tracing::debug!(%error,"FTMS range read failed");
                                tracing::warn!(
                                    "FTMS range unavailable; corresponding control mode disabled"
                                );
                            }
                        }
                    }
                }
                let profile =
                    ControlProfile::discover(&features, resistance.as_deref(), power.as_deref())?;
                let events = self.0.notifications().await.map_err(connection_error)?;
                self.0.subscribe(status).await.map_err(connection_error)?;
                self.0.subscribe(point).await.map_err(connection_error)?;
                Some(ControlSession {
                    profile,
                    events: events
                        .filter_map(|event| async move {
                            if event.uuid == CONTROL_POINT {
                                Some(ControlEvent::Indication(event.value))
                            } else if event.uuid == MACHINE_STATUS {
                                Some(ControlEvent::Status(event.value))
                            } else {
                                None
                            }
                        })
                        .boxed(),
                })
            } else {
                None
            };
            // Install the receiver before subscribing, so the first measurement is not lost.
            let notifications = self.0.notifications().await.map_err(connection_error)?;
            self.0.subscribe(data).await.map_err(connection_error)?;
            let notifications = notifications
                .filter_map(|notification| async move {
                    (notification.uuid == INDOOR_BIKE_DATA).then_some(notification.value)
                })
                .boxed();
            Ok(FtmsSession {
                format: TelemetryFormat::Ftms,
                control,
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
