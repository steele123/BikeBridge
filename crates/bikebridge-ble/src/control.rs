//! FTMS control codecs and conservative range mapping. No Bluetooth I/O.
//! Control resistance uses SINT16 per SIG ESR11 E8991/E9135.
use bikebridge_core::{
    BridgeError, DeviceCapability, ErrorCode, Result, SafetyLimits, TrainerCommand,
};

/// Validated raw range, in the characteristic's integer units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SupportedRange {
    min: i32,
    max: i32,
    step: i32,
}
impl SupportedRange {
    /// Decode a six-byte signed min/max and unsigned increment, as used by FTMS control ranges.
    /// Three-byte resistance ranges are deliberately not guessed; see docs/ftms-control.md.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let [a, b, c, d, e, f] = *bytes else {
            return Err(invalid("FTMS control range must be six bytes."));
        };
        let range = Self {
            min: i32::from(i16::from_le_bytes([a, b])),
            max: i32::from(i16::from_le_bytes([c, d])),
            step: i32::from(u16::from_le_bytes([e, f])),
        };
        if range.min >= range.max || range.step == 0 || range.step > range.max - range.min {
            return Err(invalid("Invalid FTMS control range or increment."));
        }
        Ok(range)
    }
    fn floor(self, value: f64) -> i32 {
        self.min
            + (((value.clamp(self.min as f64, self.max as f64) - self.min as f64)
                / self.step as f64)
                .floor() as i32)
                * self.step
    }
    fn watts(self, requested: u16, ceiling: u16) -> Result<u16> {
        let lower = self.min + ((0 - self.min).max(0) + self.step - 1) / self.step * self.step;
        let upper_bound = i32::from(ceiling).min(self.max);
        if lower > upper_bound {
            return Err(unsupported(
                "Device power range does not intersect the configured safety limits.",
            ));
        }
        let upper = self.floor(f64::from(upper_bound));
        let raw = self.floor(f64::from(requested)).clamp(lower, upper);
        Ok(raw as u16)
    }
    fn resistance(self, requested: f32) -> Result<(i16, f32)> {
        // Negative resistance can imply motor assistance. It is unsupported in this version.
        if self.min < 0 {
            return Err(unsupported(
                "Negative resistance ranges require a device-specific safety mapping.",
            ));
        }
        let mut raw =
            self.floor(self.min as f64 + f64::from(requested) * f64::from(self.max - self.min));
        // Keep normalization round trips stable at an f32 grid boundary without rounding above the API target.
        let next = raw + self.step;
        if next <= self.max && (next - self.min) as f32 / (self.max - self.min) as f32 <= requested
        {
            raw = next;
        }
        Ok((
            raw as i16,
            (raw - self.min) as f32 / (self.max - self.min) as f32,
        ))
    }
}

/// Verified control modes; absent/invalid ranges disable only the affected mode.
#[derive(Debug, Clone, Default)]
pub struct ControlProfile {
    resistance: Option<SupportedRange>,
    power: Option<SupportedRange>,
    simulation: bool,
}
impl ControlProfile {
    /// Decode target-setting feature bits and ranges. Only call after verifying control/status properties.
    pub fn discover(
        features: &[u8],
        resistance: Option<&[u8]>,
        power: Option<&[u8]>,
    ) -> Result<Self> {
        crate::ftms::decode_features(features)?;
        let targets = u32::from_le_bytes([features[4], features[5], features[6], features[7]]);
        let decode = |bytes: Option<&[u8]>| {
            bytes.and_then(|bytes| match SupportedRange::decode(bytes) {
            Ok(range) => Some(range),
            Err(error) => { tracing::warn!(message = %error.message,"FTMS mode disabled: unsupported range"); None }
        })
        };
        Ok(Self {
            resistance: if targets & 4 != 0 {
                decode(resistance).filter(|range| range.min >= 0)
            } else {
                None
            },
            power: if targets & 8 != 0 {
                decode(power)
            } else {
                None
            },
            simulation: targets & (1 << 13) != 0,
        })
    }
    /// Modes which the profile can encode under the configured safety limits.
    pub fn capabilities(&self, limits: SafetyLimits) -> Vec<DeviceCapability> {
        let mut result = Vec::new();
        if self.resistance.is_some() {
            result.push(DeviceCapability::ResistanceControl);
        }
        if self
            .power
            .is_some_and(|range| range.watts(0, limits.max_erg_watts).is_ok())
        {
            result.push(DeviceCapability::ErgControl);
        }
        if self.simulation {
            result.push(DeviceCapability::SimulationControl);
        }
        result
    }
    /// Clamp, quantize downward to device increments, and encode one command.
    pub fn encode(&self, limits: SafetyLimits, command: TrainerCommand) -> Result<EncodedCommand> {
        let mut applied = limits.clamp(command)?;
        let bytes = match applied {
            TrainerCommand::RequestControl => vec![0x00],
            TrainerCommand::Reset => vec![0x01],
            TrainerCommand::Start => vec![0x07],
            TrainerCommand::Stop => vec![0x08, 0x01],
            TrainerCommand::SetResistance(value) => {
                let (raw, normalized) = self
                    .resistance
                    .ok_or_else(|| {
                        unsupported("Trainer resistance control has no verified safe range.")
                    })?
                    .resistance(value)?;
                applied = TrainerCommand::SetResistance(normalized);
                let mut bytes = vec![0x04];
                bytes.extend(raw.to_le_bytes());
                bytes
            }
            TrainerCommand::SetTargetPower(watts) => {
                let watts = self
                    .power
                    .ok_or_else(|| unsupported("Trainer ERG control has no verified range."))?
                    .watts(watts, limits.max_erg_watts)?;
                applied = TrainerCommand::SetTargetPower(watts);
                let mut bytes = vec![0x05];
                bytes.extend((watts as i16).to_le_bytes());
                bytes
            }
            TrainerCommand::SetSimulation(mut simulation) => {
                if !self.simulation {
                    return Err(unsupported(
                        "Trainer does not support indoor bike simulation.",
                    ));
                }
                // Truncate toward zero so quantization cannot exceed a configured absolute ceiling.
                let wind = (simulation.wind_speed_mps * 1000.0).trunc() as i16;
                let grade = (simulation.grade_percent * 100.0).trunc() as i16;
                let crr = (simulation.crr * 10000.0).trunc() as u8;
                let cw = (simulation.cw * 100.0).trunc() as u8;
                simulation.wind_speed_mps = f32::from(wind) / 1000.0;
                simulation.grade_percent = f32::from(grade) / 100.0;
                simulation.crr = f32::from(crr) / 10000.0;
                simulation.cw = f32::from(cw) / 100.0;
                applied = TrainerCommand::SetSimulation(simulation);
                let mut bytes = vec![0x11];
                bytes.extend(wind.to_le_bytes());
                bytes.extend(grade.to_le_bytes());
                bytes.extend([crr, cw]);
                bytes
            }
        };
        Ok(EncodedCommand { applied, bytes })
    }
}
/// A safety-clamped public command and its private FTMS encoding.
#[derive(Debug)]
pub struct EncodedCommand {
    /// Actual quantized target (resistance may still be ramping toward it).
    pub applied: TrainerCommand,
    /// Control Point value, including opcode.
    pub bytes: Vec<u8>,
}
/// Validate a matching three-byte Control Point indication, preserving all result codes.
pub fn decode_response(bytes: &[u8], opcode: u8) -> Result<()> {
    let [0x80, request, result] = *bytes else {
        return Err(invalid("Malformed FTMS control indication."));
    };
    if request != opcode {
        return Err(invalid(
            "FTMS control indication does not match the pending command.",
        ));
    }
    match result {
        1 => Ok(()),
        2 => Err(unsupported(
            "Trainer rejected an unsupported control opcode.",
        )),
        3 => Err(BridgeError::new(
            ErrorCode::InvalidValue,
            "Trainer rejected the control parameter.",
        )),
        4 => Err(BridgeError::new(
            ErrorCode::TrainerControlFailed,
            "Trainer reported that the control operation failed.",
        )),
        5 => Err(BridgeError::new(
            ErrorCode::TrainerControlDenied,
            "Trainer did not permit control.",
        )),
        _ => Err(invalid("Unknown FTMS control result code.")),
    }
}
pub(crate) fn invalid(message: &str) -> BridgeError {
    BridgeError::new(ErrorCode::InvalidDeviceData, message)
}
pub(crate) fn unsupported(message: &str) -> BridgeError {
    BridgeError::new(ErrorCode::UnsupportedOperation, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bikebridge_core::TrainerSimulation;
    fn profile() -> ControlProfile {
        ControlProfile::discover(
            &[0, 0, 0, 0, 12, 32, 0, 0],
            Some(&[0, 0, 232, 3, 10, 0]),
            Some(&[100, 0, 220, 5, 25, 0]),
        )
        .expect("profile")
    }
    #[test]
    fn verified_opcodes_ranges_and_quantization() {
        let p = profile();
        let limits = SafetyLimits::default();
        for (cmd, bytes) in [
            (TrainerCommand::RequestControl, vec![0]),
            (TrainerCommand::Reset, vec![1]),
            (TrainerCommand::Start, vec![7]),
            (TrainerCommand::Stop, vec![8, 1]),
        ] {
            assert_eq!(p.encode(limits, cmd).expect("encode").bytes, bytes);
        }
        let r = p
            .encode(limits, TrainerCommand::SetResistance(9.0))
            .expect("encode");
        // f32 0.7 is below exact 0.7: conservative downward mapping must never exceed it.
        assert!(
            matches!(r.applied,TrainerCommand::SetResistance(value) if value <= limits.max_resistance)
        );
        assert_eq!(r.bytes[0], 4);
        assert_eq!(r.bytes.len(), 3);
        let pwr = p
            .encode(limits, TrainerCommand::SetTargetPower(999))
            .expect("power");
        assert_eq!(pwr.bytes, vec![5, 32, 3]);
        assert_eq!(pwr.applied, TrainerCommand::SetTargetPower(800));
        assert_eq!(
            p.encode(limits, TrainerCommand::SetTargetPower(112))
                .expect("power")
                .applied,
            TrainerCommand::SetTargetPower(100)
        );
        assert!(
            p.encode(
                SafetyLimits {
                    max_erg_watts: 50,
                    ..limits
                },
                TrainerCommand::SetTargetPower(20)
            )
            .is_err()
        );
        let sim = p
            .encode(
                limits,
                TrainerCommand::SetSimulation(TrainerSimulation {
                    wind_speed_mps: -1.0,
                    grade_percent: 30.0,
                    crr: 0.004,
                    cw: 0.51,
                }),
            )
            .expect("sim");
        assert_eq!(sim.bytes, vec![0x11, 0x18, 0xfc, 0xdc, 5, 40, 51]);
        assert!(
            p.encode(limits, TrainerCommand::SetResistance(f32::NAN))
                .is_err()
        );
    }
    #[test]
    fn invalid_ranges_and_missing_features_disable_modes() {
        for bytes in [
            &[0, 1, 1][..],
            &[1, 0, 0, 0, 1, 0],
            &[0, 0, 10, 0, 0, 0],
            &[0, 0, 10, 0, 11, 0],
        ] {
            assert!(SupportedRange::decode(bytes).is_err());
        }
        let p =
            ControlProfile::discover(&[0; 8], Some(&[0, 0, 100, 0, 1, 0]), None).expect("features");
        assert!(p.capabilities(SafetyLimits::default()).is_empty());
        assert!(
            p.encode(SafetyLimits::default(), TrainerCommand::SetResistance(0.1))
                .is_err()
        );
    }
    #[test]
    fn every_control_response_and_mismatch() {
        assert!(decode_response(&[128, 5, 1], 5).is_ok());
        for (result, code) in [
            (2, ErrorCode::UnsupportedOperation),
            (3, ErrorCode::InvalidValue),
            (4, ErrorCode::TrainerControlFailed),
            (5, ErrorCode::TrainerControlDenied),
        ] {
            assert_eq!(
                decode_response(&[128, 5, result], 5)
                    .expect_err("rejected")
                    .code,
                code
            );
        }
        for bytes in [
            &[][..],
            &[128, 5],
            &[128, 5, 0],
            &[128, 4, 1],
            &[128, 5, 1, 0],
        ] {
            assert!(decode_response(bytes, 5).is_err());
        }
    }
    #[test]
    fn range_mapping_never_crosses_a_safety_ceiling_and_round_trips() {
        for range in [
            SupportedRange {
                min: 100,
                max: 1500,
                step: 25,
            },
            SupportedRange {
                min: -123,
                max: 1000,
                step: 20,
            },
            SupportedRange {
                min: 0,
                max: 2000,
                step: 33,
            },
        ] {
            for ceiling in 1..=2000 {
                for requested in [0, 17, 200, 999, u16::MAX] {
                    if let Ok(watts) = range.watts(requested, ceiling) {
                        assert!(watts <= ceiling);
                        assert!(i32::from(watts) >= range.min && i32::from(watts) <= range.max);
                        assert_eq!((i32::from(watts) - range.min) % range.step, 0);
                    }
                }
            }
        }
        let p = profile();
        for index in 0..=700 {
            let requested = index as f32 / 1000.0;
            let first = p
                .encode(
                    SafetyLimits::default(),
                    TrainerCommand::SetResistance(requested),
                )
                .expect("first");
            let second = p
                .encode(SafetyLimits::default(), first.applied)
                .expect("repeat");
            assert_eq!(first.bytes, second.bytes);
            assert!(
                matches!(first.applied,TrainerCommand::SetResistance(value) if value<=requested)
            );
        }
    }
}
