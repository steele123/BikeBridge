//! Bluetooth Heart Rate Service 1.0, Heart Rate Measurement (0x2A37).
//! https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/HRS_v1.0/out/en/index-en.html
use bikebridge_core::{BridgeError, ErrorCode, Result, TrainerTelemetry};

fn invalid(message: &str) -> BridgeError {
    BridgeError::new(ErrorCode::InvalidDeviceData, message)
}

/// Decode a complete notification, validating optional energy and RR-interval fields.
/// No/poor skin contact explicitly clears BPM; unsupported contact detection preserves it.
/// Energy and RR intervals are consumed but not exposed by the current telemetry model.
pub fn decode(bytes: &[u8], timestamp_ms: u64) -> Result<TrainerTelemetry> {
    let (&flags, values) = bytes
        .split_first()
        .ok_or_else(|| invalid("Empty Heart Rate Measurement."))?;
    if flags & 0xe0 != 0 {
        return Err(invalid("Reserved Heart Rate Measurement flags are set."));
    }
    let width = if flags & 1 == 0 { 1 } else { 2 };
    let required = width + if flags & 8 != 0 { 2 } else { 0 };
    let (fields, rr) = values
        .split_at_checked(required)
        .ok_or_else(|| invalid("Truncated Heart Rate Measurement."))?;
    if flags & 0x10 != 0 {
        if rr.is_empty() || !rr.len().is_multiple_of(2) {
            return Err(invalid(
                "Heart Rate RR intervals must contain one or more complete 16-bit values.",
            ));
        }
    } else if !rr.is_empty() {
        return Err(invalid("Unexpected trailing Heart Rate Measurement bytes."));
    }
    let bpm = if width == 1 {
        u16::from(fields[0])
    } else {
        u16::from_le_bytes([fields[0], fields[1]])
    };
    let contact_lost = flags & 4 != 0 && flags & 2 == 0;
    Ok(TrainerTelemetry {
        heart_rate_bpm: (!contact_lost).then_some(bpm),
        timestamp_ms,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_formats_zero_and_contact_detection() {
        for (bytes, expected) in [
            (vec![0, 142], Some(142)),
            (vec![1, 44, 1], Some(300)),
            (vec![1, 255, 255], Some(65535)),
            (vec![0, 0], Some(0)),
            (vec![6, 142], Some(142)),
            (vec![4, 142], None),
            (vec![2, 142], Some(142)), // Status bit has no meaning without support bit.
        ] {
            let data = decode(&bytes, 123).expect("valid measurement");
            assert_eq!(data.heart_rate_bpm, expected);
            assert_eq!(data.timestamp_ms, 123);
            assert_eq!(data.power_watts, None);
            assert_eq!(data.cadence_rpm, None);
            assert_eq!(data.speed_kph, None);
        }
    }
    #[test]
    fn all_flag_combinations_and_optional_payloads() {
        for flags in 0u8..32 {
            let mut bytes = vec![flags, 142];
            if flags & 1 != 0 {
                bytes.push(0);
            }
            if flags & 8 != 0 {
                bytes.extend([3, 0]);
            }
            if flags & 16 != 0 {
                bytes.extend([0, 4, 128, 3]);
            }
            assert!(decode(&bytes, 0).is_ok(), "flags {flags}");
            // One complete RR interval remains valid; all other truncations are invalid.
            for length in 0..bytes.len() {
                let one_rr = flags & 16 != 0 && length == bytes.len() - 2;
                assert_eq!(
                    decode(&bytes[..length], 0).is_ok(),
                    one_rr,
                    "flags {flags}, length {length}"
                );
            }
            bytes.push(0);
            assert!(decode(&bytes, 0).is_err());
        }
        for flags in [32, 64, 128, 255] {
            assert!(decode(&[flags, 142, 0], 0).is_err());
        }
    }
    #[test]
    fn malformed_optional_data_is_rejected_even_without_contact() {
        assert!(decode(&[0x1c, 142, 1, 0, 1], 0).is_err());
        assert!(decode(&[0x14, 142], 0).is_err());
    }
}
