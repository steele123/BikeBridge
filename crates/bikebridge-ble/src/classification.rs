//! Service-only detection using Bluetooth SIG and OpenBikeControl assigned UUIDs.
//! https://www.bluetooth.com/specifications/assigned-numbers/
use bikebridge_core::DeviceKind;
use uuid::Uuid;

/// OpenBikeControl bridge service (distinct from proprietary Zwift services).
pub const OPEN_BIKE_CONTROL: Uuid = bikebridge_openbikecontrol::SERVICE;

/// Standard Fitness Machine Service (not necessarily an indoor bike).
pub const FITNESS_MACHINE: Uuid = bluetooth_uuid(0x1826);
/// Standard Cycling Power Service.
pub const CYCLING_POWER: Uuid = bluetooth_uuid(0x1818);
/// Standard Heart Rate Service.
pub const HEART_RATE: Uuid = bluetooth_uuid(0x180d);
/// Standard Cycling Speed and Cadence Service.
pub const CYCLING_SPEED_CADENCE: Uuid = bluetooth_uuid(0x1816);

const fn bluetooth_uuid(short: u16) -> Uuid {
    Uuid::from_u128(((short as u128) << 96) | 0x00001000800000805f9b34fb)
}

/// Known cycling and Zwift service UUIDs; Zwift model detection also requires manufacturer data.
pub fn cycling_services() -> Vec<Uuid> {
    vec![
        FITNESS_MACHINE,
        CYCLING_POWER,
        HEART_RATE,
        CYCLING_SPEED_CADENCE,
        OPEN_BIKE_CONTROL,
        crate::click::SERVICE,
        crate::click::LEGACY_SERVICE,
    ]
}

/// Primary advertised role, with deterministic precedence for multi-service devices.
/// This is provisional identification, never proof of a controllable trainer.
pub fn classify(services: &[Uuid]) -> Option<DeviceKind> {
    if services.contains(&FITNESS_MACHINE) {
        Some(DeviceKind::Trainer)
    } else if services.contains(&OPEN_BIKE_CONTROL) {
        Some(DeviceKind::BikeController)
    } else if services.contains(&CYCLING_POWER) {
        Some(DeviceKind::PowerMeter)
    } else if services.contains(&CYCLING_SPEED_CADENCE) {
        Some(DeviceKind::CadenceSensor)
    } else if services.contains(&HEART_RATE) {
        Some(DeviceKind::HeartRateMonitor)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sig_ids_and_classification() {
        assert_eq!(
            FITNESS_MACHINE.to_string(),
            "00001826-0000-1000-8000-00805f9b34fb"
        );
        assert_eq!(
            CYCLING_POWER.to_string(),
            "00001818-0000-1000-8000-00805f9b34fb"
        );
        assert_eq!(
            HEART_RATE.to_string(),
            "0000180d-0000-1000-8000-00805f9b34fb"
        );
        assert_eq!(
            CYCLING_SPEED_CADENCE.to_string(),
            "00001816-0000-1000-8000-00805f9b34fb"
        );
        for (service, kind) in [
            (FITNESS_MACHINE, DeviceKind::Trainer),
            (OPEN_BIKE_CONTROL, DeviceKind::BikeController),
            (CYCLING_POWER, DeviceKind::PowerMeter),
            (HEART_RATE, DeviceKind::HeartRateMonitor),
            (CYCLING_SPEED_CADENCE, DeviceKind::CadenceSensor),
        ] {
            assert_eq!(classify(&[service]), Some(kind));
        }
        assert_eq!(classify(&cycling_services()), Some(DeviceKind::Trainer));
        assert_eq!(
            classify(&[HEART_RATE, CYCLING_POWER]),
            Some(DeviceKind::PowerMeter)
        );
        assert_eq!(classify(&[]), None);
        assert_eq!(classify(&[Uuid::nil()]), None);
    }
}
