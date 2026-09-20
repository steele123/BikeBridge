//! OpenBikeControl v1 button packets and edge normalization.
//! Reference: OpenBikeControl/openbikecontrol-protocol, c057e1d7ad05ceb1a6d59a0ca95e57394b7f9b48.
use bikebridge_core::{BikeInput, BridgeError, ErrorCode, InputData, InputState, Result};

/// Explicit actions advertised to BikeControl (its current implementation needs a nonempty list).
pub const SUPPORTED_BUTTONS: &[u8] = &[
    0x01, 0x02, 0x03, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x20, 0x21,
    0x24, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x40, 0x44,
    0x45, 0x46, 0x50, 0x51, 0x52,
];

/// App information, sent after subscribing. BikeControl reassembles 20-byte BLE chunks.
pub fn app_info() -> Vec<u8> {
    let app = b"bikebridge";
    let version = env!("CARGO_PKG_VERSION").as_bytes();
    let mut bytes = vec![4, 1, app.len() as u8];
    bytes.extend(app);
    bytes.push(version.len() as u8);
    bytes.extend(version);
    bytes.push(SUPPORTED_BUTTONS.len() as u8);
    bytes.extend(SUPPORTED_BUTTONS);
    bytes
}

/// Maintains partial button updates, suppresses duplicate states, and clears held inputs on link loss.
#[derive(Default)]
pub struct Inputs {
    states: Vec<InputData>,
}
impl Inputs {
    /// Parse one complete notification atomically. Preserve multiple transitions in wire order.
    pub fn packet(&mut self, bytes: &[u8]) -> Result<Vec<InputData>> {
        self.update(decode_packet(bytes)?)
    }
    /// Apply validated partial updates from any controller transport, suppressing duplicates.
    pub fn update(&mut self, decoded: Vec<InputData>) -> Result<Vec<InputData>> {
        for data in &decoded {
            data.validate()?;
        }
        let mut output = Vec::new();
        for data in decoded {
            let current = self
                .states
                .iter_mut()
                .find(|old| old.input == data.input && old.button == data.button);
            if let Some(current) = current {
                if *current == data {
                    continue;
                }
                *current = data.clone();
            } else {
                self.states.push(data.clone());
            }
            output.push(data);
        }
        Ok(output)
    }
    /// Synthetic releases prevent stuck buttons after disconnect; gear selections need no release.
    pub fn release_all(&mut self) -> Vec<InputData> {
        std::mem::take(&mut self.states)
            .into_iter()
            .filter_map(|mut data| {
                if data.input == BikeInput::Gear || data.state == InputState::Released {
                    return None;
                }
                if data.input == BikeInput::Brake {
                    if data.value == Some(0.0) {
                        return None;
                    }
                    data.value = Some(0.0);
                } else {
                    data.state = InputState::Released;
                    data.value = None;
                }
                Some(data)
            })
            .collect()
    }
}
/// Decode one complete OpenBikeControl notification without changing held state.
pub fn decode_packet(bytes: &[u8]) -> Result<Vec<InputData>> {
    if bytes.first() != Some(&1)
        || bytes.len() < 3
        || bytes.len() > 511
        || bytes.len().is_multiple_of(2)
    {
        return Err(BridgeError::new(
            ErrorCode::InvalidDeviceData,
            "Malformed OpenBikeControl button notification.",
        ));
    }
    let mut decoded = Vec::new();
    for pair in bytes[1..].as_chunks::<2>().0 {
        if !SUPPORTED_BUTTONS.contains(&pair[0]) {
            continue;
        }
        if let Some(data) = decode(pair[0], pair[1])? {
            decoded.push(data);
        }
    }
    Ok(decoded)
}
fn decode(id: u8, state: u8) -> Result<Option<InputData>> {
    let input = match id {
        1 => BikeInput::ShiftUp,
        2 => BikeInput::ShiftDown,
        3 => BikeInput::Gear,
        0x14 => BikeInput::Confirm,
        0x15 => BikeInput::Back,
        0x18 => BikeInput::SteeringLeft,
        0x19 => BikeInput::SteeringRight,
        0x1a => BikeInput::Brake,
        _ => BikeInput::Button,
    };
    let mut data = InputData {
        input,
        state: InputState::Released,
        value: None,
        button: (input == BikeInput::Button).then_some(u16::from(id)),
    };
    match input {
        BikeInput::Gear => {
            if state < 2 {
                return Ok(None);
            }
            data.state = InputState::Value;
            data.value = Some(f32::from(state - 1).min(100.0));
        }
        BikeInput::Brake => {
            data.state = InputState::Value;
            data.value = Some(match state {
                0 => 0.0,
                1 => 1.0,
                _ => f32::from(state - 1).min(200.0) / 100.0,
            });
        }
        BikeInput::Button if state > 1 => {
            // Preserve extension-specific values (emotes, cameras, workout targets) without guessing semantics.
            data.state = InputState::Value;
            data.value = Some(f32::from(state));
        }
        _ => {
            if state > 1 {
                return Err(BridgeError::new(
                    ErrorCode::InvalidDeviceData,
                    "Unsupported analog value for a digital OpenBikeControl action.",
                ));
            }
            data.state = if state == 0 {
                InputState::Released
            } else {
                InputState::Pressed
            };
        }
    }
    data.validate()?;
    Ok(Some(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapping_partial_updates_rapid_presses_and_disconnect_release() {
        let mut inputs = Inputs::default();
        let events = inputs
            .packet(&[1, 1, 1, 2, 1, 0x14, 1, 0x18, 1])
            .expect("packet");
        assert_eq!(
            events.iter().map(|d| d.input).collect::<Vec<_>>(),
            [
                BikeInput::ShiftUp,
                BikeInput::ShiftDown,
                BikeInput::Confirm,
                BikeInput::SteeringLeft
            ]
        );
        assert!(inputs.packet(&[1, 1, 1]).expect("repeat").is_empty());
        assert_eq!(
            inputs.packet(&[1, 1, 0, 1, 1, 1, 0]).expect("rapid").len(),
            3
        );
        let releases = inputs.release_all();
        assert_eq!(releases.len(), 3);
        assert!(releases.iter().all(|d| d.state == InputState::Released));
        assert!(inputs.release_all().is_empty());
    }
    #[test]
    fn gear_brakes_and_generic_analog_values_are_explicit() {
        let mut inputs = Inputs::default();
        assert!(inputs.packet(&[1, 3, 0, 3, 1]).expect("noop").is_empty());
        let data = inputs
            .packet(&[1, 3, 2, 0x1a, 101, 0x40, 4])
            .expect("analog");
        assert_eq!(data[0].value, Some(1.0));
        assert_eq!(data[1].value, Some(1.0));
        assert_eq!(data[2].button, Some(0x40));
        assert_eq!(data[2].value, Some(4.0));
        let data = inputs.packet(&[1, 3, 255, 0x1a, 255]).expect("clamp");
        assert_eq!(data[0].value, Some(100.0));
        assert_eq!(data[1].value, Some(2.0));
        assert!(
            inputs
                .release_all()
                .iter()
                .all(|d| d.input != BikeInput::Gear)
        );
    }
    #[test]
    fn malformed_packets_do_not_partially_mutate_state() {
        let mut inputs = Inputs::default();
        for bytes in [
            vec![],
            vec![1],
            vec![1, 1],
            vec![2, 1, 1],
            vec![1, 1, 1, 2, 2],
            vec![1; 513],
        ] {
            assert!(inputs.packet(&bytes).is_err());
            assert!(inputs.release_all().is_empty());
        }
        assert!(inputs.packet(&[1, 0x7f, 1]).expect("unknown").is_empty());
        let info = app_info();
        assert_eq!(&info[..3], &[4, 1, 10]);
        assert_eq!(&info[3..13], b"bikebridge");
        assert_eq!(&info[20..], SUPPORTED_BUTTONS);
        assert_eq!(info[19] as usize, SUPPORTED_BUTTONS.len());
    }
}
