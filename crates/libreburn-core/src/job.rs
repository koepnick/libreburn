// SPDX-License-Identifier: AGPL-3.0-or-later
//! Device-neutral job model (requirement HW-3).
//!
//! Moves already carry Z, tool and auxiliary-output fields so generic CNC
//! drivers can be added later without reshaping the model.
//!
//! # Coordinates
//!
//! All positions are millimetres in job space: the origin is the top-left
//! corner of the work area, X grows to the right and Y grows down (the SVG
//! convention). Drivers map job space to their machine's own axes.
//!
//! # Stability
//!
//! Drivers depend on these types. New fields are only ever added as optional
//! fields with a serde default, so build values with [`Move::travel`],
//! [`Move::burn`] and `Layer { .., ..Default::default() }` rather than full
//! struct literals.

use serde::{Deserialize, Serialize};

use crate::profile::MachineProfile;

/// Clamp a requested power to what `profile` allows: 0 to `limits.max_power_pct`.
///
/// Drivers call this (usually through [`Job::power_for`]) right before
/// encoding a power value, so a planner bug can never exceed the profile cap.
/// NaN is treated as zero.
pub fn effective_power_pct(requested_pct: f64, profile: &MachineProfile) -> f64 {
    let cap = profile.limits.max_power_pct.clamp(0.0, 100.0);
    if requested_pct.is_nan() {
        return 0.0;
    }
    requested_pct.clamp(0.0, cap)
}

/// How a layer is processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerMode {
    /// Follow vector outlines.
    Cut,
    /// Scanline fill of closed shapes.
    Fill,
    /// Raster image engraving.
    Image,
    /// Galvo hatch fill.
    GalvoHatch,
}

/// Per-layer settings (requirement FR-6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    /// Display name.
    pub name: String,
    /// Processing mode.
    pub mode: LayerMode,
    /// Speed in mm/s.
    pub speed_mm_s: f64,
    /// Power in percent of the machine maximum, 0 to 100.
    pub power_pct: f64,
    /// Number of passes.
    pub passes: u32,
    /// Pulse frequency in kHz (fiber), if set.
    pub frequency_khz: Option<f64>,
    /// MOPA pulse width in ns, if set.
    pub pulse_width_ns: Option<u32>,
    /// Q-pulse width in ns for Q-switched fiber sources, if set.
    #[serde(default)]
    pub q_pulse_ns: Option<u32>,
}

impl Default for Layer {
    /// A cut layer at zero power. Zero is the safe default: a layer that was
    /// never configured does not fire the laser.
    fn default() -> Self {
        Self {
            name: String::new(),
            mode: LayerMode::Cut,
            speed_mm_s: 10.0,
            power_pct: 0.0,
            passes: 1,
            frequency_khz: None,
            pulse_width_ns: None,
            q_pulse_ns: None,
        }
    }
}

/// Whether a move fires the laser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveKind {
    /// Laser off.
    Travel,
    /// Laser on at the layer's power.
    Burn,
}

/// One straight move. Units are millimetres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Move {
    /// Laser off or on.
    pub kind: MoveKind,
    /// Target X.
    pub x: f64,
    /// Target Y.
    pub y: f64,
    /// Target Z, for machines that have one.
    pub z: Option<f64>,
    /// Tool or spindle index, for generic CNC.
    pub tool: Option<u32>,
    /// Auxiliary outputs to set during the move (bit mask).
    pub aux: Option<u32>,
    /// Index into [`Job::layers`].
    pub layer: usize,
    /// Power for this move in percent, overriding the layer's power (used for
    /// grayscale raster). `None` means the layer's power. Ignored for travel.
    #[serde(default)]
    pub power_pct: Option<f64>,
}

impl Move {
    /// A laser-off move to `(x, y)`.
    pub fn travel(x: f64, y: f64, layer: usize) -> Self {
        Self {
            kind: MoveKind::Travel,
            x,
            y,
            z: None,
            tool: None,
            aux: None,
            layer,
            power_pct: None,
        }
    }

    /// A laser-on move to `(x, y)` at the layer's power.
    pub fn burn(x: f64, y: f64, layer: usize) -> Self {
        Self {
            kind: MoveKind::Burn,
            ..Self::travel(x, y, layer)
        }
    }

    /// The same move with a per-move power override.
    pub fn with_power(mut self, power_pct: f64) -> Self {
        self.power_pct = Some(power_pct);
        self
    }
}

/// A planned job, ready for a driver to stream.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Job {
    /// Layers in run order.
    pub layers: Vec<Layer>,
    /// Moves in run order.
    pub moves: Vec<Move>,
}

impl Job {
    /// Axis-aligned bounding box of every move, as `(min_x, min_y, max_x, max_y)`.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let mut it = self.moves.iter();
        let first = it.next()?;
        Some(
            it.fold((first.x, first.y, first.x, first.y), |(a, b, c, d), m| {
                (a.min(m.x), b.min(m.y), c.max(m.x), d.max(m.y))
            }),
        )
    }

    /// The power to fire for `mv`, already clamped to the profile's cap.
    ///
    /// Travel moves are always 0. A burn uses the move's override if set,
    /// otherwise its layer's power. A move whose layer index is out of range
    /// gets 0, so a malformed job fails safe.
    pub fn power_for(&self, mv: &Move, profile: &MachineProfile) -> f64 {
        if mv.kind == MoveKind::Travel {
            return 0.0;
        }
        let Some(layer) = self.layers.get(mv.layer) else {
            return 0.0;
        };
        effective_power_pct(mv.power_pct.unwrap_or(layer.power_pct), profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(x: f64, y: f64) -> Move {
        Move::burn(x, y, 0)
    }

    fn profile(max_power: f64) -> MachineProfile {
        MachineProfile::from_toml(&format!(
            "[machine]\nname='t'\ndriver='mock'\nlaser='co2'\n\
             [work_area]\nwidth_mm=100\nheight_mm=100\n\
             [limits]\nmax_power_pct={max_power}\nmax_speed_mm_s=500\n\
             [framing]\ndefault_source='laser_off'\n"
        ))
        .unwrap()
    }

    fn layer(power: f64) -> Layer {
        Layer {
            name: "l".into(),
            power_pct: power,
            ..Default::default()
        }
    }

    #[test]
    fn effective_power_is_clamped_to_the_cap() {
        let p = profile(60.0);
        assert_eq!(effective_power_pct(80.0, &p), 60.0);
        assert_eq!(effective_power_pct(-5.0, &p), 0.0);
        assert_eq!(effective_power_pct(f64::NAN, &p), 0.0);
        assert_eq!(effective_power_pct(30.0, &p), 30.0);
    }

    #[test]
    fn power_for_uses_override_then_layer_and_clamps() {
        let p = profile(50.0);
        let job = Job {
            layers: vec![layer(40.0)],
            moves: vec![],
        };
        assert_eq!(job.power_for(&Move::burn(0.0, 0.0, 0), &p), 40.0);
        assert_eq!(
            job.power_for(&Move::burn(0.0, 0.0, 0).with_power(20.0), &p),
            20.0
        );
        assert_eq!(
            job.power_for(&Move::burn(0.0, 0.0, 0).with_power(90.0), &p),
            50.0
        );
        assert_eq!(
            job.power_for(&Move::travel(0.0, 0.0, 0).with_power(90.0), &p),
            0.0
        );
        assert_eq!(job.power_for(&Move::burn(0.0, 0.0, 7), &p), 0.0);
    }

    #[test]
    fn hil_profile_cap_forces_zero_power() {
        let job = Job {
            layers: vec![layer(100.0)],
            moves: vec![],
        };
        assert_eq!(job.power_for(&mv(0.0, 0.0), &profile(0.0)), 0.0);
    }

    #[test]
    fn default_layer_does_not_fire() {
        assert_eq!(Layer::default().power_pct, 0.0);
    }

    #[test]
    fn older_json_without_new_fields_still_parses() {
        let text = r#"{"layers":[{"name":"a","mode":"cut","speed_mm_s":10,"power_pct":5,
            "passes":1,"frequency_khz":null,"pulse_width_ns":null}],
            "moves":[{"kind":"burn","x":1,"y":2,"z":null,"tool":null,"aux":null,"layer":0}]}"#;
        let job: Job = serde_json::from_str(text).unwrap();
        assert_eq!(job.moves[0].power_pct, None);
        assert_eq!(job.layers[0].q_pulse_ns, None);
    }

    #[test]
    fn bounds_of_empty_job_is_none() {
        assert_eq!(Job::default().bounds(), None);
    }

    #[test]
    fn bounds_cover_every_move() {
        let job = Job {
            layers: vec![],
            moves: vec![mv(1.0, 5.0), mv(-2.0, 3.0), mv(4.0, -1.0)],
        };
        assert_eq!(job.bounds(), Some((-2.0, -1.0, 4.0, 5.0)));
    }

    #[test]
    fn job_round_trips_through_json() {
        let job = Job {
            layers: vec![],
            moves: vec![mv(1.0, 2.0)],
        };
        let text = serde_json::to_string(&job).unwrap();
        assert_eq!(serde_json::from_str::<Job>(&text).unwrap(), job);
    }
}
