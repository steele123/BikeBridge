//! Deterministic, hardware-free device implementations for development and CI.
use bikebridge_core::*;
use serde::Deserialize;
use std::time::Duration;

/// Stable development trainer identity.
pub const TRAINER_ID: &str = "mock-trainer";
/// Stable development controller identity.
pub const CONTROLLER_ID: &str = "mock-controller";

/// Optional overrides for mock measurements; omitted fields retain prior values.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MockTelemetry {
    /// Power in watts (0–2000).
    pub power_watts: Option<i16>,
    /// Cadence in RPM (0–250).
    pub cadence_rpm: Option<f32>,
    /// Speed in km/h (0–150).
    pub speed_kph: Option<f32>,
    /// Heart rate in BPM (30–240).
    pub heart_rate_bpm: Option<u16>,
}

impl MockTelemetry {
    /// Validate the entire patch before mutating a device.
    pub fn validate(&self) -> Result<()> {
        let invalid = self.power_watts.is_some_and(|v| !(0..=2000).contains(&v))
            || self
                .cadence_rpm
                .is_some_and(|v| !v.is_finite() || !(0.0..=250.0).contains(&v))
            || self
                .speed_kph
                .is_some_and(|v| !v.is_finite() || !(0.0..=150.0).contains(&v))
            || self
                .heart_rate_bpm
                .is_some_and(|v| !(30..=240).contains(&v));
        if invalid {
            return Err(BridgeError::new(
                ErrorCode::InvalidValue,
                "Mock measurement is outside its documented range.",
            ));
        }
        Ok(())
    }
}

/// Simulated trainer with repeatable telemetry and configurable resistance smoothing.
pub struct MockTrainer {
    connected: bool,
    running: bool,
    limits: SafetyLimits,
    mode: TrainerCommand,
    resistance: f32,
    target_resistance: f32,
    elapsed: f32,
    distance: f64,
    overrides: MockTelemetry,
}

impl MockTrainer {
    /// Construct an already-connected mock trainer after validating its limits.
    pub fn new(limits: SafetyLimits) -> Result<Self> {
        limits.validate()?;
        Ok(Self {
            connected: true,
            running: true,
            limits,
            mode: TrainerCommand::Reset,
            resistance: 0.0,
            target_resistance: 0.0,
            elapsed: 0.0,
            distance: 0.0,
            overrides: MockTelemetry::default(),
        })
    }

    /// Update mock measurements atomically. Overrides persist until reset or a control command.
    pub fn set_telemetry(&mut self, patch: MockTelemetry) -> Result<()> {
        self.ensure_connected()?;
        patch.validate()?;
        if patch.power_watts.is_some() {
            self.overrides.power_watts = patch.power_watts;
        }
        if patch.cadence_rpm.is_some() {
            self.overrides.cadence_rpm = patch.cadence_rpm;
        }
        if patch.speed_kph.is_some() {
            self.overrides.speed_kph = patch.speed_kph;
        }
        if patch.heart_rate_bpm.is_some() {
            self.overrides.heart_rate_bpm = patch.heart_rate_bpm;
        }
        Ok(())
    }

    /// Advance the simulation. No packet is produced while disconnected.
    pub fn tick(&mut self, delta: Duration) -> Option<TrainerTelemetry> {
        if !self.connected {
            return None;
        }
        let dt = delta.as_secs_f32().min(1.0);
        self.elapsed += dt;
        let step = self.limits.max_resistance_change_per_second * dt;
        if self.limits.smooth_resistance {
            self.resistance += (self.target_resistance - self.resistance).clamp(-step, step);
        } else {
            self.resistance = self.target_resistance;
        }
        let wave = (self.elapsed * 0.7).sin();
        let power = match self.mode {
            TrainerCommand::SetTargetPower(watts) => watts as f32,
            TrainerCommand::SetResistance(_) => 100.0 + self.resistance * 500.0,
            TrainerCommand::SetSimulation(s) => 180.0 + s.grade_percent * 10.0,
            _ => 180.0,
        };
        let speed = if self.running {
            self.overrides.speed_kph.unwrap_or(30.0 + wave * 0.8)
        } else {
            0.0
        };
        self.distance += speed as f64 / 3.6 * dt as f64;
        Some(TrainerTelemetry {
            power_watts: Some(if self.running {
                self.overrides
                    .power_watts
                    .unwrap_or((power + wave * 3.0).max(0.0) as i16)
            } else {
                0
            }),
            cadence_rpm: Some(if self.running {
                self.overrides.cadence_rpm.unwrap_or(88.0 + wave * 1.5)
            } else {
                0.0
            }),
            speed_kph: Some(speed),
            heart_rate_bpm: Some(self.overrides.heart_rate_bpm.unwrap_or(142)),
            distance_meters: Some(self.distance),
            resistance_level: Some(self.resistance),
            timestamp_ms: timestamp_ms(),
            ..Default::default()
        })
    }

    fn ensure_connected(&self) -> Result<()> {
        if self.connected {
            Ok(())
        } else {
            Err(BridgeError::new(
                ErrorCode::DeviceDisconnected,
                "Trainer is disconnected.",
            ))
        }
    }
}

impl Trainer for MockTrainer {
    fn info(&self) -> DeviceInfo {
        DeviceInfo {
            id: TRAINER_ID.into(),
            name: "BikeBridge Mock Trainer".into(),
            kind: DeviceKind::Trainer,
            transport: "mock".into(),
            connected: self.connected,
            signal_strength: None,
            capabilities: vec![
                DeviceCapability::Power,
                DeviceCapability::Cadence,
                DeviceCapability::Speed,
                DeviceCapability::HeartRate,
                DeviceCapability::ResistanceControl,
                DeviceCapability::ErgControl,
                DeviceCapability::SimulationControl,
            ],
        }
    }
    async fn connect(&mut self) -> Result<()> {
        self.connected = true;
        self.running = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<()> {
        self.safe_state();
        self.connected = false;
        Ok(())
    }
    async fn execute(&mut self, command: TrainerCommand) -> Result<TrainerCommand> {
        self.ensure_connected()?;
        let command = self.limits.clamp(command)?;
        match command {
            TrainerCommand::RequestControl => {}
            TrainerCommand::Reset => self.safe_state(),
            TrainerCommand::Start => self.running = true,
            TrainerCommand::Stop => {
                self.safe_state();
                self.running = false;
            }
            TrainerCommand::SetResistance(value) => {
                self.mode = command;
                self.target_resistance = value;
                self.overrides.power_watts = None;
            }
            TrainerCommand::SetTargetPower(_) | TrainerCommand::SetSimulation(_) => {
                self.mode = command;
                self.target_resistance = 0.0;
                self.overrides.power_watts = None;
            }
        }
        Ok(command)
    }
    fn safe_state(&mut self) {
        self.mode = TrainerCommand::Reset;
        self.resistance = 0.0;
        self.target_resistance = 0.0;
        self.overrides = MockTelemetry::default();
    }
}

/// Explicitly injected controller; no synthetic unsolicited button presses.
pub struct MockController {
    connected: bool,
}

impl Default for MockController {
    fn default() -> Self {
        Self { connected: true }
    }
}

impl MockController {
    /// Current controller metadata.
    pub fn info(&self) -> DeviceInfo {
        DeviceInfo {
            id: CONTROLLER_ID.into(),
            name: "BikeBridge Mock Controller".into(),
            kind: DeviceKind::BikeController,
            transport: "mock".into(),
            connected: self.connected,
            signal_strength: None,
            capabilities: vec![DeviceCapability::ShiftButtons, DeviceCapability::Steering],
        }
    }
    /// Change the mock connection state.
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }
    /// Validate and normalize an input into a timestamped event.
    pub fn input(&self, data: InputData) -> Result<Event> {
        if !self.connected {
            return Err(BridgeError::new(
                ErrorCode::DeviceDisconnected,
                "Controller is disconnected.",
            ));
        }
        data.validate()?;
        Ok(Event::Input {
            device_id: CONTROLLER_ID.into(),
            data,
            timestamp_ms: timestamp_ms(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn trainer_obeys_limits_slew_and_disconnect() {
        let mut trainer = MockTrainer::new(SafetyLimits::default()).expect("trainer");
        assert_eq!(
            trainer
                .execute(TrainerCommand::SetTargetPower(900))
                .await
                .expect("ERG"),
            TrainerCommand::SetTargetPower(800)
        );
        assert!(
            (797..=803).contains(
                &trainer
                    .tick(Duration::from_millis(250))
                    .expect("sample")
                    .power_watts
                    .expect("power")
            )
        );
        trainer
            .execute(TrainerCommand::SetResistance(1.0))
            .await
            .expect("resistance");
        assert_eq!(
            trainer
                .tick(Duration::from_millis(250))
                .expect("sample")
                .resistance_level,
            Some(0.05)
        );
        trainer.safe_state();
        assert_eq!(
            trainer
                .tick(Duration::from_millis(250))
                .expect("sample")
                .resistance_level,
            Some(0.0)
        );
        trainer.disconnect().await.expect("disconnect");
        assert!(trainer.tick(Duration::from_secs(1)).is_none());
        assert_eq!(
            trainer
                .execute(TrainerCommand::Start)
                .await
                .expect_err("disconnected")
                .code,
            ErrorCode::DeviceDisconnected
        );
    }
    #[test]
    fn mock_injection_is_atomic() {
        let mut trainer = MockTrainer::new(SafetyLimits::default()).expect("trainer");
        assert!(
            trainer
                .set_telemetry(MockTelemetry {
                    power_watts: Some(250),
                    cadence_rpm: Some(f32::NAN),
                    ..Default::default()
                })
                .is_err()
        );
        assert_ne!(
            trainer.tick(Duration::ZERO).expect("sample").power_watts,
            Some(250)
        );
        trainer
            .set_telemetry(MockTelemetry {
                power_watts: Some(250),
                ..Default::default()
            })
            .expect("override");
        assert_eq!(
            trainer.tick(Duration::ZERO).expect("sample").power_watts,
            Some(250)
        );
    }
    #[test]
    fn controller_validates_input() {
        let mut controller = MockController::default();
        let input = InputData {
            input: BikeInput::ShiftUp,
            state: InputState::Pressed,
            value: None,
            button: None,
        };
        assert!(matches!(
            controller.input(input.clone()),
            Ok(Event::Input { .. })
        ));
        assert!(
            controller
                .input(InputData {
                    state: InputState::Value,
                    ..input.clone()
                })
                .is_err()
        );
        controller.set_connected(false);
        assert_eq!(
            controller.input(input).expect_err("disconnected").code,
            ErrorCode::DeviceDisconnected
        );
    }
}
