// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plain-text machine profiles (requirement HW-1).

use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorCode, Result};
use crate::safety::FramingSource;

/// Laser source type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaserType {
    /// CO2 tube (gantry machines such as the K40).
    Co2,
    /// Fiber source on a galvo head.
    Fiber,
    /// Diode module.
    Diode,
}

/// The `[machine]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineInfo {
    /// Display name.
    pub name: String,
    /// Driver to load, such as `ruida` or `lmc`.
    pub driver: String,
    /// Laser source type.
    pub laser: LaserType,
    /// Marks a hardware-in-the-loop test profile. HIL profiles must cap power at zero.
    #[serde(default)]
    pub hil: bool,
}

/// The `[work_area]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkArea {
    /// Width in mm.
    pub width_mm: f64,
    /// Height in mm.
    pub height_mm: f64,
}

/// The `[limits]` table, used by job checks (FR-17, FR-18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Highest power a job may request, in percent.
    pub max_power_pct: f64,
    /// Highest speed in mm/s.
    pub max_speed_mm_s: f64,
    /// Acceleration in mm/s², for time estimates.
    pub accel_mm_s2: Option<f64>,
    /// Longest continuous laser-on time in seconds.
    pub max_continuous_on_s: Option<f64>,
    /// Duty cycle limit in percent.
    pub duty_cycle_pct: Option<f64>,
    /// Limits that block a job instead of warning.
    #[serde(default)]
    pub hard: Vec<String>,
}

/// The `[framing]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Framing {
    /// Default framing source.
    pub default_source: FramingSource,
    /// Output port of the galvo guide (red) light. Required for galvo framing.
    pub guide_light_port: Option<u8>,
}

/// A machine profile, loaded from TOML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineProfile {
    /// The `[machine]` table.
    pub machine: MachineInfo,
    /// The `[work_area]` table.
    pub work_area: WorkArea,
    /// The `[limits]` table.
    pub limits: Limits,
    /// The `[framing]` table.
    pub framing: Framing,
    /// Driver-specific connection settings, passed through untouched.
    #[serde(default)]
    pub connection: toml::Table,
}

impl MachineProfile {
    /// Parse and validate a profile from TOML text.
    pub fn from_toml(text: &str) -> Result<Self> {
        let profile: Self = toml::from_str(text).map_err(|e| {
            Error::new(
                ErrorCode::ProfileInvalid,
                "Could not parse machine profile",
                e.to_string(),
                "fix the line named above; see profiles/ for working examples",
            )
        })?;
        profile.validate()?;
        Ok(profile)
    }

    /// Check rules that TOML types alone cannot express.
    pub fn validate(&self) -> Result<()> {
        let invalid = |why: String, hint: &str| {
            Err(Error::new(
                ErrorCode::ProfileInvalid,
                format!("Profile \"{}\" is invalid", self.machine.name),
                why,
                hint,
            ))
        };
        if !(0.0..=100.0).contains(&self.limits.max_power_pct) {
            return invalid(
                format!("limits.max_power_pct is {}", self.limits.max_power_pct),
                "use a value from 0 to 100",
            );
        }
        if self.machine.hil && self.limits.max_power_pct != 0.0 {
            return invalid(
                format!(
                    "HIL profiles must set limits.max_power_pct = 0, found {}",
                    self.limits.max_power_pct
                ),
                "hardware tests check protocol and motion only; set the power cap to 0",
            );
        }
        if self.machine.laser == LaserType::Fiber {
            if self.framing.guide_light_port.is_none() {
                return invalid(
                    "fiber profiles need framing.guide_light_port".into(),
                    "set the output port the red guide light is wired to",
                );
            }
            if self.framing.default_source == FramingSource::MainSourceLowPower {
                return invalid(
                    "fiber profiles cannot default to framing with the main source".into(),
                    "set framing.default_source = \"guide_light\"; low-power framing is a per-session opt-in",
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GALVO: &str = r#"
[machine]
name = "test galvo"
driver = "lmc"
laser = "fiber"

[work_area]
width_mm = 110
height_mm = 110

[limits]
max_power_pct = 100
max_speed_mm_s = 4000

[framing]
default_source = "guide_light"
guide_light_port = 8
"#;

    #[test]
    fn parses_a_valid_galvo_profile() {
        let p = MachineProfile::from_toml(GALVO).unwrap();
        assert_eq!(p.machine.laser, LaserType::Fiber);
        assert_eq!(p.framing.guide_light_port, Some(8));
    }

    #[test]
    fn fiber_profile_without_guide_light_port_is_rejected() {
        let text = GALVO.replace("guide_light_port = 8\n", "");
        let err = MachineProfile::from_toml(&text).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProfileInvalid);
    }

    #[test]
    fn fiber_profile_cannot_default_to_main_source_framing() {
        let text = GALVO.replace("\"guide_light\"", "\"main_source_low_power\"");
        assert!(MachineProfile::from_toml(&text).is_err());
    }

    #[test]
    fn hil_profile_with_power_is_rejected() {
        let text = GALVO.replace("laser = \"fiber\"", "laser = \"fiber\"\nhil = true");
        let err = MachineProfile::from_toml(&text).unwrap_err();
        assert!(err.why.contains("HIL"));
    }

    #[test]
    fn unknown_keys_are_reported() {
        let text = GALVO.replace("[framing]", "[framing]\ntypo = 1");
        assert!(MachineProfile::from_toml(&text).is_err());
    }
}
