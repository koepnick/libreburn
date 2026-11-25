// SPDX-License-Identifier: AGPL-3.0-or-later
//! Device-neutral job model (requirement HW-3).
//!
//! Moves already carry Z, tool and auxiliary-output fields so generic CNC
//! drivers can be added later without reshaping the model.

use serde::{Deserialize, Serialize};

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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(x: f64, y: f64) -> Move {
        Move {
            kind: MoveKind::Burn,
            x,
            y,
            z: None,
            tool: None,
            aux: None,
            layer: 0,
        }
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
