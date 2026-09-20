use crate::{BridgeError, ErrorCode, Result};
use serde::{Deserialize, Serialize};

/// Hardware-independent input action; parameterized actions use `value`/`button`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BikeInput {
    /// Shift to the next gear.
    ShiftUp,
    /// Shift to the previous gear.
    ShiftDown,
    /// Left steering action.
    SteeringLeft,
    /// Right steering action.
    SteeringRight,
    /// Absolute steering, with value between -1 and 1.
    Steering,
    /// Gear selection, using an integer value.
    Gear,
    /// Confirm action.
    Confirm,
    /// Back action.
    Back,
    /// Generic button, identified by the `button` field.
    Button,
}

/// Digital edge or analog value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputState {
    /// Button pressed.
    Pressed,
    /// Button released.
    Released,
    /// Analog value changed.
    Value,
}

/// Input payload. The enclosing event supplies its device identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputData {
    /// Normalized action.
    pub input: BikeInput,
    /// Edge or analog state.
    pub state: InputState,
    /// Analog value or gear number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f32>,
    /// Required only for generic buttons.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<u16>,
}

impl InputData {
    /// Reject inconsistent states and malformed analog values.
    pub fn validate(&self) -> Result<()> {
        let analog = matches!(self.input, BikeInput::Steering | BikeInput::Gear);
        let valid = if analog {
            self.state == InputState::Value
                && self.value.is_some_and(|v| {
                    v.is_finite()
                        && match self.input {
                            BikeInput::Steering => (-1.0..=1.0).contains(&v),
                            BikeInput::Gear => (1.0..=100.0).contains(&v) && v.fract() == 0.0,
                            _ => false,
                        }
                })
        } else {
            self.state != InputState::Value && self.value.is_none()
        };
        if !valid || (self.input == BikeInput::Button) != self.button.is_some() {
            return Err(BridgeError::new(
                ErrorCode::InvalidValue,
                "Input state, value, or button is invalid.",
            ));
        }
        Ok(())
    }
}
