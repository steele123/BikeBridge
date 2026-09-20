//! Cycling Power measurements (Bluetooth GSS: Cycling Power Feature/Measurement).
//! https://btprodspecificationrefs.blob.core.windows.net/gatt-specification-supplement/GATT_Specification_Supplement.pdf
use bikebridge_core::{BridgeError, DeviceCapability, ErrorCode, Result, TrainerTelemetry};
use std::time::Duration;
use tokio::time::Instant;

fn invalid(message: &str) -> BridgeError {
    BridgeError::new(ErrorCode::InvalidDeviceData, message)
}

/// Verify the four-byte feature value. Cadence requires crank revolution support.
pub fn decode_features(bytes: &[u8]) -> Result<Vec<DeviceCapability>> {
    let bytes: [u8; 4] = bytes
        .try_into()
        .map_err(|_| invalid("Cycling Power Feature must contain four bytes."))?;
    let mut capabilities = vec![DeviceCapability::Power];
    if u32::from_le_bytes(bytes) & (1 << 3) != 0 {
        capabilities.push(DeviceCapability::Cadence);
    }
    Ok(capabilities)
}

struct Cursor<'a>(&'a [u8]);
impl Cursor<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8]> {
        let (head, tail) = self
            .0
            .split_at_checked(len)
            .ok_or_else(|| invalid("Truncated Cycling Power Measurement."))?;
        self.0 = tail;
        Ok(head)
    }
    fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
}

/// Cadence state belongs to a single connection; a reconnect starts a new baseline.
#[derive(Default)]
pub struct Decoder {
    crank: Option<(u16, u16)>,
    last_change: Option<Instant>,
    cadence: Option<f32>,
}

impl Decoder {
    /// Parse power and optional crank data, consuming all other optional fields.
    /// Repeated crank values retain cadence for three seconds, then report zero.
    /// Missing crank data or ambiguous counters omit cadence rather than inventing it.
    pub fn decode(
        &mut self,
        bytes: &[u8],
        now: Instant,
        timestamp_ms: u64,
    ) -> Result<TrainerTelemetry> {
        let mut cursor = Cursor(bytes);
        let flags = cursor.u16()?;
        if flags & 0xe000 != 0 {
            return Err(invalid("Reserved Cycling Power Measurement flags are set."));
        }
        let power = cursor.u16()? as i16;
        let present = |bit: u32| flags & (1u16 << bit) != 0;
        for (bit, len) in [(0, 1), (2, 2), (4, 6)] {
            if present(bit) {
                cursor.take(len)?;
            }
        }
        let crank = if present(5) {
            Some((cursor.u16()?, cursor.u16()?))
        } else {
            None
        };
        for (bit, len) in [(6, 4), (7, 4), (8, 3), (9, 2), (10, 2), (11, 2)] {
            if present(bit) {
                cursor.take(len)?;
            }
        }
        if !cursor.0.is_empty() {
            return Err(invalid(
                "Unexpected trailing Cycling Power Measurement bytes.",
            ));
        }

        let mut cadence = None;
        if let Some((revolutions, ticks)) = crank {
            if let (Some(previous), Some(last_change)) = (self.crank, self.last_change) {
                if previous == (revolutions, ticks) {
                    cadence = if now.duration_since(last_change) >= Duration::from_secs(3) {
                        Some(0.0)
                    } else {
                        self.cadence
                    };
                } else if now.duration_since(last_change) < Duration::from_secs(64) {
                    // Both 16-bit counters wrap. The event clock ticks at 1024 Hz.
                    let delta_revs = revolutions.wrapping_sub(previous.0);
                    let delta_ticks = ticks.wrapping_sub(previous.1);
                    if delta_ticks != 0 {
                        let rpm = f32::from(delta_revs) * 60.0 * 1024.0 / f32::from(delta_ticks);
                        // Reject implausible jumps caused by counter resets.
                        if rpm <= 300.0 {
                            cadence = Some(rpm);
                        }
                    }
                }
            }
            if self.crank != crank {
                self.last_change = Some(now);
            }
        } else {
            self.last_change = None;
        }
        self.crank = crank;
        self.cadence = cadence;
        Ok(TrainerTelemetry {
            power_watts: Some(power),
            cadence_rpm: cadence,
            timestamp_ms,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn measurement(power: i16, revs: u16, ticks: u16) -> Vec<u8> {
        [
            0x20u16.to_le_bytes(),
            power.to_le_bytes(),
            revs.to_le_bytes(),
            ticks.to_le_bytes(),
        ]
        .concat()
    }
    #[test]
    fn xds_packet_cadence_rollover_and_stopping() {
        let mut decoder = Decoder::default();
        let now = Instant::now();
        let sample = decoder
            .decode(&[0x20, 0, 0, 0, 0x98, 0x48, 0xa0, 0x1d], now, 123)
            .expect("observed XDS packet");
        assert_eq!(sample.power_watts, Some(0));
        assert_eq!(sample.cadence_rpm, None);
        assert_eq!(sample.timestamp_ms, 123);
        let mut decoder = Decoder::default();
        decoder
            .decode(&measurement(250, 65535, 65024), now, 0)
            .expect("baseline");
        let moving = now + Duration::from_secs(1);
        assert_eq!(
            decoder
                .decode(&measurement(251, 0, 512), moving, 0)
                .expect("rollover")
                .cadence_rpm,
            Some(60.0)
        );
        assert_eq!(
            decoder
                .decode(&measurement(0, 0, 512), moving + Duration::from_secs(1), 0)
                .expect("repeat")
                .cadence_rpm,
            Some(60.0)
        );
        assert_eq!(
            decoder
                .decode(&measurement(0, 0, 512), moving + Duration::from_secs(3), 0)
                .expect("stopped")
                .cadence_rpm,
            Some(0.0)
        );
        assert_eq!(
            decoder
                .decode(
                    &measurement(250, 100, 1024),
                    moving + Duration::from_secs(65),
                    0
                )
                .expect("ambiguous clock")
                .cadence_rpm,
            None
        );
    }
    #[test]
    fn optional_fields_signed_power_and_malformed_packets() {
        let mut decoder = Decoder::default();
        let now = Instant::now();
        // Every optional payload, including fields before crank data.
        let mut bytes = vec![0xff, 0x1f, 0xfb, 0xff];
        bytes.extend([0; 30]);
        assert_eq!(
            decoder
                .decode(&bytes, now, 0)
                .expect("all fields")
                .power_watts,
            Some(-5)
        );
        for len in 0..bytes.len() {
            assert!(decoder.decode(&bytes[..len], now, 0).is_err());
        }
        bytes.push(0);
        assert!(decoder.decode(&bytes, now, 0).is_err());
        assert!(decoder.decode(&[0, 0x80, 0, 0], now, 0).is_err());
        assert_eq!(
            decode_features(&[8, 0, 0, 0]).expect("features"),
            vec![DeviceCapability::Power, DeviceCapability::Cadence]
        );
        assert!(decode_features(&[8]).is_err());
    }
    #[test]
    fn bad_or_missing_data_cannot_create_a_cadence_spike() {
        let now = Instant::now();
        let mut decoder = Decoder::default();
        decoder
            .decode(&measurement(250, 10, 1000), now, 0)
            .expect("baseline");
        let mut bad = measurement(250, 500, 1100);
        bad.push(1);
        assert!(decoder.decode(&bad, now, 0).is_err());
        assert_eq!(
            decoder
                .decode(&measurement(250, 11, 2024), now, 0)
                .expect("unchanged baseline")
                .cadence_rpm,
            Some(60.0)
        );
        assert_eq!(
            decoder
                .decode(&measurement(250, 0, 100), now, 0)
                .expect("reset")
                .cadence_rpm,
            None
        );
        decoder
            .decode(&[0, 0, 0, 0], now, 0)
            .expect("no crank data");
        assert_eq!(
            decoder
                .decode(&measurement(250, 1, 1124), now, 0)
                .expect("new baseline")
                .cadence_rpm,
            None
        );
    }
}
