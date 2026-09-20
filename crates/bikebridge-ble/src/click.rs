//! Zwift Click V2 wire facts, independently implemented. See docs/zwift-click-v2.md.
use bikebridge_core::{BikeInput, BridgeError, ErrorCode, InputData, InputState, Result};
use uuid::Uuid;

/// Legacy Zwift controller service; shared with other Zwift products.
pub const LEGACY_SERVICE: Uuid = Uuid::from_u128(0x00000001_19ca_4651_86e5_fa29dcdd09d1);
/// Current Zwift service; service alone does not identify a Click.
pub const SERVICE: Uuid = Uuid::from_u128(0x0000fc82_0000_1000_8000_00805f9b34fb);
pub(crate) const NOTIFY: Uuid = Uuid::from_u128(0x00000002_19ca_4651_86e5_fa29dcdd09d1);
pub(crate) const WRITE: Uuid = Uuid::from_u128(0x00000003_19ca_4651_86e5_fa29dcdd09d1);
pub(crate) const INDICATE: Uuid = Uuid::from_u128(0x00000004_19ca_4651_86e5_fa29dcdd09d1);
pub(crate) const HANDSHAKE: &[u8] = b"RideOn\x02\x03";

/// Physical Click V2 side, identified by Zwift manufacturer data rather than its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// D-pad and minus button.
    Left,
    /// A/B/Y/Z and plus button.
    Right,
}
impl Side {
    /// Decode the model byte from company 0x094a's advertisement payload.
    pub fn from_manufacturer(company: u16, payload: &[u8]) -> Option<Self> {
        if company != 0x094a || payload.len() < 3 {
            return None;
        }
        match payload[0] {
            0x0a => Some(Self::Right),
            0x0b => Some(Self::Left),
            _ => None,
        }
    }
    /// A side label usable when the advertised name is absent or identical on both pucks.
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Zwift Click V2 (left)",
            Self::Right => "Zwift Click V2 (right)",
        }
    }
}

fn invalid() -> BridgeError {
    BridgeError::new(
        ErrorCode::InvalidDeviceData,
        "Malformed Zwift Click V2 button notification.",
    )
}

/// Decode a complete plaintext notification. Only the identified side's buttons are emitted.
/// Idle/battery/handshake packets produce no input; malformed button frames fail atomically.
pub fn decode(side: Side, bytes: &[u8]) -> Result<Vec<InputData>> {
    if bytes.is_empty() || bytes.len() > 512 {
        return Err(invalid());
    }
    if bytes.starts_with(b"RideOn") {
        return Ok(Vec::new());
    }
    if bytes.starts_with(&[0xff, 0x05, 0x00]) {
        return Err(BridgeError::new(
            ErrorCode::UnsupportedOperation,
            "Click V2 stopped input. Enable both controllers in the Zwift game, close Zwift, then reconnect. Firmware compatibility is experimental.",
        ));
    }
    if bytes[0] != 0x23 {
        return Ok(Vec::new());
    }
    let mut rest = &bytes[1..];
    let mut bitmap = None;
    while !rest.is_empty() {
        let tag = varint(&mut rest)?;
        let field = tag >> 3;
        let wire = tag & 7;
        if field == 0 || field > 0x1fff_ffff {
            return Err(invalid());
        }
        if field == 1 {
            if wire != 0 || bitmap.is_some() {
                return Err(invalid());
            }
            bitmap = Some(u32::try_from(varint(&mut rest)?).map_err(|_| invalid())?);
        } else {
            match wire {
                0 => {
                    varint(&mut rest)?;
                }
                1 => skip(&mut rest, 8)?,
                2 => {
                    let count = usize::try_from(varint(&mut rest)?).map_err(|_| invalid())?;
                    skip(&mut rest, count)?;
                }
                5 => skip(&mut rest, 4)?,
                _ => return Err(invalid()),
            }
        }
    }
    let bitmap = bitmap.ok_or_else(invalid)?;
    // Bit positions are the Ride/Click V2 keypad bitmap. Zero means pressed.
    let buttons: &[(u32, BikeInput, Option<u16>)] = match side {
        Side::Left => &[
            (0x1, BikeInput::SteeringLeft, None),
            (0x2, BikeInput::Button, Some(0x2001)),
            (0x4, BikeInput::SteeringRight, None),
            (0x8, BikeInput::Button, Some(0x2002)),
            (0x200, BikeInput::ShiftDown, None),
        ],
        Side::Right => &[
            (0x10, BikeInput::Confirm, None),
            (0x20, BikeInput::Back, None),
            (0x40, BikeInput::Button, Some(0x2003)),
            (0x100, BikeInput::Button, Some(0x2004)),
            (0x2000, BikeInput::ShiftUp, None),
        ],
    };
    Ok(buttons
        .iter()
        .map(|&(mask, input, button)| InputData {
            input,
            button,
            value: None,
            state: if bitmap & mask == 0 {
                InputState::Pressed
            } else {
                InputState::Released
            },
        })
        .collect())
}

fn skip(bytes: &mut &[u8], count: usize) -> Result<()> {
    *bytes = bytes.get(count..).ok_or_else(invalid)?;
    Ok(())
}
fn varint(bytes: &mut &[u8]) -> Result<u64> {
    let mut value = 0;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.first().ok_or_else(invalid)?;
        *bytes = &bytes[1..];
        if shift == 63 && byte > 1 {
            return Err(invalid());
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(bitmap: u32) -> Vec<u8> {
        let mut bytes = vec![0x23, 0x08];
        let mut value = bitmap;
        while value >= 128 {
            bytes.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        bytes.push(value as u8);
        bytes
    }
    #[test]
    fn manufacturer_model_and_side_are_required() {
        assert_eq!(
            Side::from_manufacturer(0x094a, &[0x0a, 1, 2]),
            Some(Side::Right)
        );
        assert_eq!(
            Side::from_manufacturer(0x094a, &[0x0b, 1, 2]),
            Some(Side::Left)
        );
        for (company, bytes) in [
            (0x094a, vec![9, 1, 2]),
            (0x094a, vec![1, 1, 2]),
            (0x1234, vec![0x0b, 1, 2]),
            (0x094a, vec![0x0b]),
        ] {
            assert_eq!(Side::from_manufacturer(company, &bytes), None);
        }
    }
    #[test]
    fn captured_bitmap_and_all_ten_buttons() {
        // Published wire capture: first bit clear means left arrow pressed.
        let captured = [
            0x23, 0x08, 0xfe, 0xff, 0xff, 0xff, 0x0f, 0x12, 0x06, 0x0a, 0x04, 0x08, 0, 0x10, 0,
        ];
        let result = decode(Side::Left, &captured).expect("capture");
        assert_eq!(result[0].input, BikeInput::SteeringLeft);
        assert_eq!(result[0].state, InputState::Pressed);
        assert_eq!(
            result
                .iter()
                .filter(|d| d.state == InputState::Pressed)
                .count(),
            1
        );
        for side in [Side::Left, Side::Right] {
            assert!(
                decode(side, &frame(u32::MAX))
                    .expect("released")
                    .iter()
                    .all(|d| d.state == InputState::Released)
            );
            assert_eq!(
                decode(side, &frame(0))
                    .expect("held")
                    .iter()
                    .filter(|d| d.state == InputState::Pressed)
                    .count(),
                5
            );
        }
        assert!(
            decode(Side::Left, &frame(!0x2000))
                .expect("other side")
                .iter()
                .all(|d| d.state == InputState::Released)
        );
        assert_eq!(
            decode(Side::Right, &frame(!0x2000)).expect("plus")[4].input,
            BikeInput::ShiftUp
        );
        assert_eq!(
            decode(Side::Left, &frame(!0x200)).expect("minus")[4].input,
            BikeInput::ShiftDown
        );
    }
    #[test]
    fn malformed_protobuf_never_changes_held_state() {
        let mut state = bikebridge_openbikecontrol::protocol::Inputs::default();
        state
            .update(decode(Side::Right, &frame(!0x2000)).expect("press"))
            .expect("update");
        for bad in [
            vec![],
            vec![0x23],
            vec![0x23, 8, 0x80],
            vec![0x23, 0],
            vec![0x23, 8, 1, 8, 2],
            vec![0x23, 8, 1, 18, 9],
            vec![0x23, 8, 0xff, 0xff, 0xff, 0xff, 0x7f],
        ] {
            assert!(decode(Side::Right, &bad).is_err());
        }
        assert!(
            state
                .update(decode(Side::Right, &frame(!0x2000)).expect("held"))
                .expect("repeat")
                .is_empty()
        );
        let released = state.release_all();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].input, BikeInput::ShiftUp);
        assert_eq!(released[0].state, InputState::Released);
        assert!(
            decode(Side::Right, &[0x19, 8, 100])
                .expect("battery")
                .is_empty()
        );
        assert!(decode(Side::Left, &[0xff, 5, 0, 0xea, 5]).is_err());
    }
}
