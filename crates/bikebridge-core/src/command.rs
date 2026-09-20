use crate::{BridgeError, ErrorCode, Result};
use serde::{Deserialize, Serialize};

/// Physical simulation parameters, expressed in SI units except grade (percent).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrainerSimulation {
    /// Road grade in percent.
    pub grade_percent: f32,
    /// Wind speed in meters per second.
    pub wind_speed_mps: f32,
    /// Rolling resistance coefficient.
    pub crr: f32,
    /// Wind resistance coefficient in kg/m.
    pub cw: f32,
}

/// Transport-neutral trainer command.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "operation", content = "value", rename_all = "snake_case")]
pub enum TrainerCommand {
    /// Acquire device control.
    RequestControl,
    /// Return to the safe default.
    Reset,
    /// Resume telemetry simulation.
    Start,
    /// Stop and remove active load.
    Stop,
    /// Set normalized resistance.
    SetResistance(f32),
    /// Set target watts.
    SetTargetPower(u16),
    /// Set simulation parameters.
    SetSimulation(TrainerSimulation),
}

/// Configurable software ceilings. Hardware backends must also enforce device limits.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SafetyLimits {
    /// Maximum ERG target in watts.
    pub max_erg_watts: u16,
    /// Maximum absolute simulated grade in percent.
    pub max_grade_percent: f32,
    /// Maximum normalized resistance.
    pub max_resistance: f32,
    /// Maximum normalized resistance increase per second when smoothing is enabled.
    pub max_resistance_change_per_second: f32,
    /// Smooth resistance changes; emergency reset always bypasses smoothing.
    pub smooth_resistance: bool,
}

impl Default for SafetyLimits {
    fn default() -> Self {
        Self {
            max_erg_watts: 800,
            max_grade_percent: 15.0,
            max_resistance: 0.7,
            max_resistance_change_per_second: 0.2,
            smooth_resistance: true,
        }
    }
}

impl SafetyLimits {
    /// Validate configuration before accepting commands.
    pub fn validate(&self) -> Result<()> {
        if self.max_erg_watts == 0
            || self.max_erg_watts > 2000
            || !self.max_grade_percent.is_finite()
            || !(0.0..=25.0).contains(&self.max_grade_percent)
            || !self.max_resistance.is_finite()
            || !(0.0..=1.0).contains(&self.max_resistance)
            || !self.max_resistance_change_per_second.is_finite()
            || !(0.001..=1.0).contains(&self.max_resistance_change_per_second)
        {
            return Err(BridgeError::new(
                ErrorCode::InvalidValue,
                "Invalid trainer safety limits.",
            ));
        }
        Ok(())
    }

    /// Reject non-finite numbers and clamp finite commands to configured ceilings.
    pub fn clamp(&self, command: TrainerCommand) -> Result<TrainerCommand> {
        self.validate()?;
        Ok(match command {
            TrainerCommand::SetResistance(value) => {
                finite(value)?;
                TrainerCommand::SetResistance(value.clamp(0.0, self.max_resistance))
            }
            TrainerCommand::SetTargetPower(watts) => {
                TrainerCommand::SetTargetPower(watts.min(self.max_erg_watts))
            }
            TrainerCommand::SetSimulation(mut simulation) => {
                for v in [
                    simulation.grade_percent,
                    simulation.wind_speed_mps,
                    simulation.crr,
                    simulation.cw,
                ] {
                    finite(v)?;
                }
                simulation.grade_percent = simulation
                    .grade_percent
                    .clamp(-self.max_grade_percent, self.max_grade_percent);
                simulation.wind_speed_mps = simulation.wind_speed_mps.clamp(-20.0, 20.0);
                simulation.crr = simulation.crr.clamp(0.0, 0.02);
                simulation.cw = simulation.cw.clamp(0.0, 1.0);
                TrainerCommand::SetSimulation(simulation)
            }
            other => other,
        })
    }
}

fn finite(value: f32) -> Result<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(BridgeError::new(
            ErrorCode::InvalidValue,
            "Values must be finite.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clamps_resistance_erg_and_simulation() {
        let limits = SafetyLimits::default();
        assert_eq!(
            limits
                .clamp(TrainerCommand::SetResistance(99.0))
                .expect("valid"),
            TrainerCommand::SetResistance(0.7)
        );
        assert_eq!(
            limits
                .clamp(TrainerCommand::SetResistance(-1.0))
                .expect("valid"),
            TrainerCommand::SetResistance(0.0)
        );
        assert_eq!(
            limits
                .clamp(TrainerCommand::SetTargetPower(900))
                .expect("valid"),
            TrainerCommand::SetTargetPower(800)
        );
        let TrainerCommand::SetSimulation(s) = limits
            .clamp(TrainerCommand::SetSimulation(TrainerSimulation {
                grade_percent: -90.0,
                wind_speed_mps: 90.0,
                crr: -1.0,
                cw: 2.0,
            }))
            .expect("valid")
        else {
            panic!("simulation")
        };
        assert_eq!(
            (s.grade_percent, s.wind_speed_mps, s.crr, s.cw),
            (-15.0, 20.0, 0.0, 1.0)
        );
    }
    #[test]
    fn rejects_non_finite_values_and_invalid_limits() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                SafetyLimits::default()
                    .clamp(TrainerCommand::SetResistance(value))
                    .is_err()
            );
            assert!(
                SafetyLimits::default()
                    .clamp(TrainerCommand::SetSimulation(TrainerSimulation {
                        grade_percent: value,
                        wind_speed_mps: 0.0,
                        crr: 0.004,
                        cw: 0.51,
                    }))
                    .is_err()
            );
        }
        assert!(
            SafetyLimits {
                max_resistance: 2.0,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}
