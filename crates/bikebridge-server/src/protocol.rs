//! Version-one client messages and response envelopes.
use bikebridge_core::{
    BridgeError, ErrorCode, Event, InputData, Result, TrainerCommand, TrainerSimulation,
};
use bikebridge_mock::MockTelemetry;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Supported event families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    /// Requested trainer commands and their outcomes.
    Command,
    /// Client session lifecycle.
    Session,
    /// Replay lifecycle.
    Replay,
    /// Scan lifecycle and failures.
    Scan,
    /// Trainer measurements.
    Telemetry,
    /// Controller inputs.
    Input,
    /// Discovery and connection state.
    Device,
    /// Broadcast failures.
    Error,
}

/// Session-local event filter; starts empty and is replaced by each subscription.
#[derive(Debug, Default)]
pub struct Subscription {
    /// Enabled event categories.
    pub events: Vec<EventCategory>,
    /// Empty means all devices.
    pub device_ids: Vec<String>,
}

impl Subscription {
    /// Test an event against this session's subscription.
    pub fn matches(&self, event: &Event) -> bool {
        let category = match event.category() {
            "scan" => EventCategory::Scan,
            "command" => EventCategory::Command,
            "session" => EventCategory::Session,
            "replay" => EventCategory::Replay,
            "telemetry" => EventCategory::Telemetry,
            "input" => EventCategory::Input,
            "device" => EventCategory::Device,
            _ => EventCategory::Error,
        };
        self.events.contains(&category)
            && (self.device_ids.is_empty()
                || event
                    .device_id()
                    .is_some_and(|id| self.device_ids.iter().any(|filter| filter == id)))
    }
}

/// Strict command schema. Unknown fields and variants are rejected.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Command {
    /// Replace the subscription; empty events unsubscribe from everything.
    #[serde(rename = "subscribe")]
    Subscribe {
        /// Requested event families.
        events: Vec<EventCategory>,
        /// Optional device filter.
        #[serde(rename = "deviceIds", default)]
        device_ids: Vec<String>,
    },
    /// Connect a discovered device.
    #[serde(rename = "device.connect")]
    Connect {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
    },
    /// Disconnect a device.
    #[serde(rename = "device.disconnect")]
    Disconnect {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
    },
    /// Acquire exclusive trainer control.
    #[serde(rename = "trainer.requestControl")]
    RequestControl {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
    },
    /// Reset the trainer and release ownership.
    #[serde(rename = "trainer.reset")]
    Reset {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
    },
    /// Start/resume the trainer.
    #[serde(rename = "trainer.start")]
    Start {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
    },
    /// Stop/pause the trainer.
    #[serde(rename = "trainer.stop")]
    Stop {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
    },
    /// Set normalized resistance.
    #[serde(rename = "trainer.setResistance")]
    SetResistance {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
        /// Requested load.
        data: ResistanceData,
    },
    /// Set ERG power.
    #[serde(rename = "trainer.setTargetPower")]
    SetTargetPower {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
        /// Requested watts.
        data: PowerData,
    },
    /// Set simulation parameters.
    #[serde(rename = "trainer.setSimulation")]
    SetSimulation {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
        /// Simulation parameters.
        data: TrainerSimulation,
    },
    /// Inject mock measurements.
    #[serde(rename = "mock.setTelemetry")]
    MockTelemetry {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
        /// Partial measurement override.
        data: MockTelemetry,
    },
    /// Inject a controller event.
    #[serde(rename = "mock.input")]
    MockInput {
        /// Device identity.
        #[serde(rename = "deviceId")]
        device_id: String,
        /// Normalized input.
        data: InputData,
    },
}

/// Resistance command data.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResistanceData {
    /// Normalized resistance, clamped to configured limits.
    pub resistance: f32,
}
/// ERG command data.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerData {
    /// Target watts, clamped to configured limits.
    pub watts: u16,
}

impl Command {
    /// Map API syntax to hardware-independent trainer operations.
    pub fn trainer_command(&self) -> Option<(&str, TrainerCommand)> {
        Some(match self {
            Self::RequestControl { device_id } => (device_id, TrainerCommand::RequestControl),
            Self::Reset { device_id } => (device_id, TrainerCommand::Reset),
            Self::Start { device_id } => (device_id, TrainerCommand::Start),
            Self::Stop { device_id } => (device_id, TrainerCommand::Stop),
            Self::SetResistance { device_id, data } => {
                (device_id, TrainerCommand::SetResistance(data.resistance))
            }
            Self::SetTargetPower { device_id, data } => {
                (device_id, TrainerCommand::SetTargetPower(data.watts))
            }
            Self::SetSimulation { device_id, data } => {
                (device_id, TrainerCommand::SetSimulation(*data))
            }
            _ => return None,
        })
    }
}

/// Decode one strict JSON command while preserving valid correlation IDs on errors.
pub fn decode(text: &str) -> (Option<String>, Result<Command>) {
    let invalid = || {
        BridgeError::new(
            ErrorCode::InvalidCommand,
            "Invalid command. See docs/protocol.md for the version 1 schema.",
        )
    };
    let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(text) else {
        return (None, Err(invalid()));
    };
    let request_id = match fields.remove("requestId") {
        None => None,
        Some(Value::String(id)) if !id.is_empty() && id.len() <= 128 => Some(id),
        _ => return (None, Err(invalid())),
    };
    let command = serde_json::from_value(Value::Object(fields)).map_err(|_| invalid());
    (request_id, command)
}

/// Every command receives a response, even without a correlation ID.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// Always `response`.
    pub r#type: &'static str,
    /// Echoed when supplied and valid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Whether the operation succeeded.
    pub success: bool,
    /// Result, including the effective command when clamped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Stable public failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BridgeError>,
}

impl Response {
    /// Construct a response from a domain result.
    pub fn new(request_id: Option<String>, result: Result<Value>) -> Self {
        match result {
            Ok(data) => Self {
                r#type: "response",
                request_id,
                success: true,
                data: Some(data),
                error: None,
            },
            Err(error) => Self {
                r#type: "response",
                request_id,
                success: false,
                data: None,
                error: Some(error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_commands_retain_correlation_and_reject_unknown_fields() {
        for body in [
            r#"{"type":"trainer.setTargetPower","requestId":"test","deviceId":"mock-trainer","data":{"watts":-1}}"#,
            r#"{"type":"trainer.setResistance","requestId":"test","deviceId":"mock-trainer","data":{"resistance":0.2,"bypass":true}}"#,
            r#"{"type":"trainer.reset","requestId":"test","deviceId":"mock-trainer","extra":true}"#,
            r#"{"type":"subscribe","requestId":"test","events":["unknown"]}"#,
        ] {
            let (id, result) = decode(body);
            assert_eq!(id.as_deref(), Some("test"));
            assert_eq!(result.expect_err("invalid").code, ErrorCode::InvalidCommand);
        }
        for body in [
            "null",
            "[]",
            "{",
            r#"{"type":"subscribe","events":[],"requestId":1}"#,
        ] {
            assert!(decode(body).1.is_err());
        }
    }
    #[test]
    fn valid_command_and_response_shape() {
        let (id, command) = decode(
            r#"{"type":"trainer.setTargetPower","requestId":"one","deviceId":"mock-trainer","data":{"watts":250}}"#,
        );
        assert!(matches!(
            command.expect("valid"),
            Command::SetTargetPower { .. }
        ));
        let response =
            serde_json::to_value(Response::new(id, Ok(serde_json::json!({})))).expect("serialize");
        assert_eq!(response["type"], "response");
        assert_eq!(response["requestId"], "one");
        assert_eq!(response["success"], true);
        assert!(response.get("error").is_none());
    }
}
