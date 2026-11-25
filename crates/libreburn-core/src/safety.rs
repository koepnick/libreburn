// SPDX-License-Identifier: AGPL-3.0-or-later
//! Safety rules (requirements FR-15, FR-17, FR-18).
//!
//! Framing never fires the main laser source unless the user has opted in for
//! the current session. This check runs in the service and again in each
//! driver, so a bug in one layer is caught by the other.

use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorCode, Result};
use crate::profile::MachineProfile;

/// What traces the outline during framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FramingSource {
    /// Move the head with the laser off (gantry machines).
    LaserOff,
    /// The galvo's red guide light.
    GuideLight,
    /// The main source at minimum power. Needs a per-session opt-in.
    MainSourceLowPower,
}

/// Per-session safety settings. The default is the safe one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyPolicy {
    /// The user explicitly allowed framing with the main source for this session.
    pub main_source_framing_opt_in: bool,
}

/// Check that framing with `source` is allowed on `profile` under `policy`.
pub fn check_framing(
    profile: &MachineProfile,
    source: FramingSource,
    policy: &SafetyPolicy,
) -> Result<()> {
    let blocked = |why: &str, hint: &str| {
        Err(Error::new(
            ErrorCode::SafetyBlocked,
            "Framing was blocked",
            why,
            hint,
        ))
    };
    match source {
        FramingSource::LaserOff => Ok(()),
        FramingSource::GuideLight => match profile.framing.guide_light_port {
            Some(_) => Ok(()),
            None => blocked(
                "the profile has no framing.guide_light_port",
                "set the guide light's output port in the machine profile",
            ),
        },
        FramingSource::MainSourceLowPower => {
            if !policy.main_source_framing_opt_in {
                return blocked(
                    "framing with the main laser source needs an opt-in for this session",
                    "use the guide light, or enable low-power framing in the device panel",
                );
            }
            if profile.limits.max_power_pct <= 0.0 {
                return blocked(
                    "this profile caps laser power at zero",
                    "use the guide light or laser-off framing",
                );
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(port: Option<u8>, max_power: f64) -> MachineProfile {
        let port = port
            .map(|p| format!("guide_light_port = {p}\n"))
            .unwrap_or_default();
        MachineProfile::from_toml(&format!(
            "[machine]\nname='t'\ndriver='lmc'\nlaser='co2'\n\
             [work_area]\nwidth_mm=1\nheight_mm=1\n\
             [limits]\nmax_power_pct={max_power}\nmax_speed_mm_s=1\n\
             [framing]\ndefault_source='laser_off'\n{port}"
        ))
        .unwrap()
    }

    #[test]
    fn main_source_framing_is_blocked_by_default() {
        let err = check_framing(
            &profile(Some(1), 50.0),
            FramingSource::MainSourceLowPower,
            &SafetyPolicy::default(),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::SafetyBlocked);
    }

    #[test]
    fn main_source_framing_is_allowed_after_opt_in() {
        let policy = SafetyPolicy {
            main_source_framing_opt_in: true,
        };
        assert!(
            check_framing(
                &profile(Some(1), 50.0),
                FramingSource::MainSourceLowPower,
                &policy
            )
            .is_ok()
        );
    }

    #[test]
    fn main_source_framing_is_blocked_when_power_is_capped_at_zero() {
        let policy = SafetyPolicy {
            main_source_framing_opt_in: true,
        };
        assert!(
            check_framing(
                &profile(Some(1), 0.0),
                FramingSource::MainSourceLowPower,
                &policy
            )
            .is_err()
        );
    }

    #[test]
    fn guide_light_needs_a_port() {
        let p = profile(None, 50.0);
        assert!(check_framing(&p, FramingSource::GuideLight, &SafetyPolicy::default()).is_err());
    }
}
