//! Fitness Machine Service decoding, independent of BLE I/O.
//! Layout: Bluetooth SIG GSS Indoor Bike Data; record semantics: FTMS 1.0.1 §4.9/4.19.
use bikebridge_core::{BridgeError, DeviceCapability, ErrorCode, Result, TrainerTelemetry};
use std::time::Duration;
use tokio::time::Instant;

/// A decoded Indoor Bike Data notification, which may be a record fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct IndoorBikeData {
    /// More notifications belong to this measurement record when true.
    pub more_data: bool,
    /// Available normalized measurements from this notification only.
    pub telemetry: TrainerTelemetry,
    flags: u16,
}

fn invalid(message: &str) -> BridgeError {
    BridgeError::new(ErrorCode::InvalidDeviceData, message)
}

struct Cursor<'a>(&'a [u8]);
impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let (head, tail) = self
            .0
            .split_at_checked(N)
            .ok_or_else(|| invalid("Truncated FTMS Indoor Bike Data."))?;
        let mut bytes = [0; N];
        bytes.copy_from_slice(head);
        self.0 = tail;
        Ok(bytes)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take()?))
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take::<1>()?[0])
    }
}

/// Decode one notification. Reject reserved flags, truncation, and trailing bytes.
/// Optional unsupported metrics are consumed so later fields retain their offsets.
/// Resistance follows the GSS uint8 layout; its raw scale is not the API's 0..1 scale.
pub fn decode_indoor_bike_data(bytes: &[u8]) -> Result<IndoorBikeData> {
    let mut cursor = Cursor(bytes);
    let flags = cursor.u16()?;
    if flags & 0xe000 != 0 {
        return Err(invalid("Reserved FTMS Indoor Bike Data flags are set."));
    }
    let present = |bit: u32| flags & (1u16 << bit) != 0;
    let mut telemetry = TrainerTelemetry::default();
    if !present(0) {
        telemetry.speed_kph = Some(f32::from(cursor.u16()?) / 100.0);
    }
    if present(1) {
        cursor.u16()?;
    } // Average speed
    if present(2) {
        telemetry.cadence_rpm = Some(f32::from(cursor.u16()?) / 2.0);
    }
    if present(3) {
        cursor.u16()?;
    } // Average cadence
    if present(4) {
        let [a, b, c] = cursor.take()?;
        telemetry.distance_meters = Some(f64::from(u32::from_le_bytes([a, b, c, 0])));
    }
    if present(5) {
        cursor.u8()?;
    } // GSS Resistance Level (not normalized)
    if present(6) {
        telemetry.power_watts = Some(cursor.i16()?);
    }
    if present(7) {
        telemetry.average_power_watts = Some(cursor.i16()?);
    }
    if present(8) {
        cursor.take::<5>()?;
    } // Total, hourly, per-minute energy
    if present(9) {
        telemetry.heart_rate_bpm = Some(u16::from(cursor.u8()?));
    }
    if present(10) {
        cursor.u8()?;
    } // MET
    if present(11) {
        telemetry.elapsed_time_seconds = Some(cursor.u16()?);
    }
    if present(12) {
        cursor.u16()?;
    } // Remaining time
    if !cursor.0.is_empty() {
        return Err(invalid("Unexpected trailing FTMS Indoor Bike Data bytes."));
    }
    Ok(IndoorBikeData {
        more_data: present(0),
        telemetry,
        flags,
    })
}

/// Validate the eight-byte Fitness Machine Feature value and select read-only capabilities.
pub fn decode_features(bytes: &[u8]) -> Result<Vec<DeviceCapability>> {
    if bytes.len() != 8 {
        return Err(invalid(
            "Fitness Machine Feature must contain exactly eight bytes.",
        ));
    }
    let mut cursor = Cursor(bytes);
    let features = u32::from_le_bytes(cursor.take()?);
    cursor.take::<4>()?; // Target settings do not authorize control in Phase 3.
    let mut capabilities = vec![DeviceCapability::Speed];
    for (bit, capability) in [
        (1, DeviceCapability::Cadence),
        (10, DeviceCapability::HeartRate),
        (14, DeviceCapability::Power),
    ] {
        if features & (1 << bit) != 0 {
            capabilities.push(capability);
        }
    }
    Ok(capabilities)
}

/// Bounded record assembly; never carries absent values into a later record.
#[derive(Default)]
pub(crate) struct RecordAssembler {
    pending: TrainerTelemetry,
    flags: u16,
    fragments: u8,
    started: Option<Instant>,
    discarding: bool,
}
impl RecordAssembler {
    pub fn push(
        &mut self,
        bytes: &[u8],
        now: Instant,
        timestamp_ms: u64,
    ) -> Result<Option<TrainerTelemetry>> {
        let data = match decode_indoor_bike_data(bytes) {
            Ok(data) => data,
            Err(error) => {
                // An invalid fragment invalidates its entire record. The next final fragment resynchronizes it.
                *self = Self {
                    discarding: bytes.len() < 2 || bytes[0] & 1 != 0,
                    ..Self::default()
                };
                return Err(error);
            }
        };
        if self.discarding {
            if !data.more_data {
                *self = Self::default();
            }
            return Ok(None);
        }
        if self.fragments >= 16
            || self
                .started
                .is_some_and(|start| now.duration_since(start) > Duration::from_secs(5))
            || self.flags & (data.flags & !1) != 0
        {
            *self = Self {
                discarding: data.more_data,
                ..Self::default()
            };
            return Err(invalid(
                "Incomplete or overlapping FTMS measurement record discarded.",
            ));
        }
        self.started.get_or_insert(now);
        self.fragments += 1;
        self.flags |= data.flags & !1;
        macro_rules! merge { ($($field:ident),*) => { $(if data.telemetry.$field.is_some() { self.pending.$field = data.telemetry.$field; })* }; }
        merge!(
            power_watts,
            average_power_watts,
            cadence_rpm,
            speed_kph,
            heart_rate_bpm,
            distance_meters,
            elapsed_time_seconds
        );
        if data.more_data {
            return Ok(None);
        }
        let mut telemetry = std::mem::take(&mut self.pending);
        telemetry.timestamp_ms = timestamp_ms;
        *self = Self::default();
        Ok(Some(telemetry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_packet_and_signed_power() {
        let data =
            decode_indoor_bike_data(&[0x44, 0, 0xb8, 0xb, 181, 0, 0xec, 0xff]).expect("decode");
        assert_eq!(data.telemetry.speed_kph, Some(30.0));
        assert_eq!(data.telemetry.cadence_rpm, Some(90.5));
        assert_eq!(data.telemetry.power_watts, Some(-20));
        assert_eq!(data.telemetry.heart_rate_bpm, None);
    }
    #[test]
    fn every_optional_field_and_all_truncations() {
        let packet = [
            0xfe, 0x1f, 0xb8, 0x0b, 0, 0, 181, 0, 0, 0, 0x56, 0x34, 0x12, 7, 0xec, 0xff, 200, 0,
            0xff, 0xff, 0xff, 0xff, 0xff, 150, 20, 0x10, 0x0e, 0, 0,
        ];
        let data = decode_indoor_bike_data(&packet)
            .expect("full record")
            .telemetry;
        assert_eq!(data.distance_meters, Some(1_193_046.0));
        assert_eq!(data.power_watts, Some(-20));
        assert_eq!(data.average_power_watts, Some(200));
        assert_eq!(data.heart_rate_bpm, Some(150));
        assert_eq!(data.elapsed_time_seconds, Some(3600));
        assert_eq!(data.resistance_level, None);
        for end in 0..packet.len() {
            assert!(
                decode_indoor_bike_data(&packet[..end]).is_err(),
                "length {end}"
            );
        }
        let mut trailing = packet.to_vec();
        trailing.push(0);
        assert!(decode_indoor_bike_data(&trailing).is_err());
        assert!(decode_indoor_bike_data(&[0, 0x20, 0, 0]).is_err());
    }
    #[test]
    fn all_flag_combinations_have_exact_lengths() {
        let sizes = [0, 2, 2, 2, 3, 1, 2, 2, 5, 1, 1, 2, 2];
        for flags in 0u16..0x2000 {
            let len = 2
                + if flags & 1 == 0 { 2 } else { 0 }
                + (1..13)
                    .filter(|bit| flags & (1 << bit) != 0)
                    .map(|bit| sizes[bit])
                    .sum::<usize>();
            let mut bytes = vec![0; len];
            bytes[..2].copy_from_slice(&flags.to_le_bytes());
            assert!(decode_indoor_bike_data(&bytes).is_ok(), "flags {flags:x}");
            bytes.push(0);
            assert!(decode_indoor_bike_data(&bytes).is_err());
        }
    }
    #[test]
    fn records_merge_then_forget_and_resynchronize() {
        let mut assembler = RecordAssembler::default();
        let now = Instant::now();
        assert!(
            assembler
                .push(&[0x41, 0, 250, 0], now, 1)
                .expect("fragment")
                .is_none()
        );
        let data = assembler
            .push(&[4, 0, 0xb8, 0xb, 180, 0], now, 2)
            .expect("final")
            .expect("sample");
        assert_eq!(data.power_watts, Some(250));
        assert_eq!(data.timestamp_ms, 2);
        assert_eq!(
            assembler
                .push(&[0, 0, 0, 0], now, 3)
                .expect("new")
                .expect("sample")
                .power_watts,
            None
        );
        assert!(assembler.push(&[0x41, 0, 0], now, 4).is_err());
        assert!(
            assembler
                .push(&[0, 0, 0, 0], now, 5)
                .expect("resync")
                .is_none()
        );
        assert!(
            assembler
                .push(&[0, 0, 0, 0], now, 6)
                .expect("next")
                .is_some()
        );
        assembler.push(&[0x41, 0, 1, 0], now, 7).expect("fragment");
        assert!(
            assembler
                .push(&[0, 0, 0, 0], now + Duration::from_secs(6), 8)
                .is_err()
        );
    }
    #[test]
    fn features_never_advertise_control() {
        assert_eq!(
            decode_features(&[2, 0x44, 0, 0, 255, 255, 255, 255]).expect("features"),
            vec![
                DeviceCapability::Speed,
                DeviceCapability::Cadence,
                DeviceCapability::HeartRate,
                DeviceCapability::Power
            ]
        );
        for len in [0, 7, 9] {
            assert!(decode_features(&vec![0; len]).is_err());
        }
    }
}
