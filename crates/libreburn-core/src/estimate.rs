// SPDX-License-Identifier: AGPL-3.0-or-later
//! Job time estimates (requirement FR-13).
//!
//! Each move follows a trapezoidal speed profile limited by the profile's
//! acceleration. Speed through a corner is reduced with the turn angle and
//! the whole sequence gets a forward and backward pass so no move needs more
//! acceleration than the machine has, the same idea GRBL's planner uses.
//! Without an acceleration limit (typical for galvos) moves run at constant
//! speed. `time_scale` is the calibration factor: measured / estimated.

use serde::{Deserialize, Serialize};

use crate::geometry::dist;
use crate::job::{Job, MoveKind};
use crate::profile::MachineProfile;

/// Estimate for one layer of the job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerEstimate {
    /// Layer name.
    pub name: String,
    /// Seconds spent on moves of this layer.
    pub seconds: f64,
    /// Seconds with the laser on.
    pub burn_seconds: f64,
    /// Laser-on distance in mm.
    pub burn_mm: f64,
    /// Laser-off distance in mm.
    pub travel_mm: f64,
}

/// Estimate for a whole job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    /// Total seconds.
    pub seconds: f64,
    /// Per layer, in [`Job::layers`] order.
    pub layers: Vec<LayerEstimate>,
}

/// Settings for [`estimate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EstimateOptions {
    /// Speed for laser-off moves in mm/s. Defaults to the profile's maximum speed.
    pub travel_speed_mm_s: Option<f64>,
    /// Calibration factor applied to the result.
    pub time_scale: f64,
    /// Where the head starts, in mm.
    pub start: (f64, f64),
}

impl Default for EstimateOptions {
    fn default() -> Self {
        Self {
            travel_speed_mm_s: None,
            time_scale: 1.0,
            start: (0.0, 0.0),
        }
    }
}

/// Speed in mm/s at which a move of `job` runs on `profile`.
pub fn move_speed(
    job: &Job,
    kind: MoveKind,
    layer: usize,
    profile: &MachineProfile,
    opts: &EstimateOptions,
) -> f64 {
    let max = profile.limits.max_speed_mm_s.max(1e-3);
    let v = match kind {
        MoveKind::Travel => opts.travel_speed_mm_s.unwrap_or(max),
        MoveKind::Burn => job.layers.get(layer).map_or(max, |l| l.speed_mm_s),
    };
    v.clamp(1e-3, max)
}

/// Seconds for each move of `job`, in order, before `time_scale`.
pub fn move_times(job: &Job, profile: &MachineProfile, opts: &EstimateOptions) -> Vec<f64> {
    let n = job.moves.len();
    let mut starts = Vec::with_capacity(n);
    let mut prev = opts.start;
    for m in &job.moves {
        starts.push(prev);
        prev = (m.x, m.y);
    }
    let len: Vec<f64> = job
        .moves
        .iter()
        .zip(&starts)
        .map(|(m, s)| dist(*s, (m.x, m.y)))
        .collect();
    let speed: Vec<f64> = job
        .moves
        .iter()
        .map(|m| move_speed(job, m.kind, m.layer, profile, opts))
        .collect();
    let Some(accel) = profile.limits.accel_mm_s2.filter(|a| *a > 0.0) else {
        return (0..n).map(|i| len[i] / speed[i]).collect();
    };

    // Junction speed between move i-1 and move i (index i), 0 at both ends.
    let mut junction = vec![0.0; n + 1];
    for i in 1..n {
        let (a, b) = (&job.moves[i - 1], &job.moves[i]);
        if len[i - 1] < 1e-9 || len[i] < 1e-9 {
            continue;
        }
        let d1 = (
            (a.x - starts[i - 1].0) / len[i - 1],
            (a.y - starts[i - 1].1) / len[i - 1],
        );
        let d2 = ((b.x - starts[i].0) / len[i], (b.y - starts[i].1) / len[i]);
        let cos = (d1.0 * d2.0 + d1.1 * d2.1).clamp(-1.0, 1.0);
        let factor = (1.0 + cos) / 2.0;
        junction[i] = speed[i - 1].min(speed[i]) * factor * factor;
    }
    // Backward pass: be able to slow down for what comes next.
    for i in (0..n).rev() {
        let reachable = (junction[i + 1].powi(2) + 2.0 * accel * len[i]).sqrt();
        junction[i] = junction[i].min(reachable);
    }
    // Forward pass: no more speed than accelerating from the previous junction allows.
    for i in 0..n {
        let reachable = (junction[i].powi(2) + 2.0 * accel * len[i]).sqrt();
        junction[i + 1] = junction[i + 1].min(reachable);
    }
    (0..n)
        .map(|i| trapezoid(len[i], junction[i], junction[i + 1], speed[i], accel))
        .collect()
}

/// Time to cover `len` starting at `v0`, ending at `v1`, cruising at most at `v`.
fn trapezoid(len: f64, v0: f64, v1: f64, v: f64, a: f64) -> f64 {
    if len <= 0.0 {
        return 0.0;
    }
    let (v0, v1) = (v0.min(v), v1.min(v));
    let d_acc = (v * v - v0 * v0) / (2.0 * a);
    let d_dec = (v * v - v1 * v1) / (2.0 * a);
    if d_acc + d_dec <= len {
        (v - v0) / a + (v - v1) / a + (len - d_acc - d_dec) / v
    } else {
        // Triangle: peak speed below cruise.
        let peak = ((2.0 * a * len + v0 * v0 + v1 * v1) / 2.0).sqrt();
        (peak - v0).max(0.0) / a + (peak - v1).max(0.0) / a
    }
}

/// Estimate the run time of `job` on `profile`.
pub fn estimate(job: &Job, profile: &MachineProfile, opts: &EstimateOptions) -> Estimate {
    let times = move_times(job, profile, opts);
    let mut layers: Vec<LayerEstimate> = job
        .layers
        .iter()
        .map(|l| LayerEstimate {
            name: l.name.clone(),
            seconds: 0.0,
            burn_seconds: 0.0,
            burn_mm: 0.0,
            travel_mm: 0.0,
        })
        .collect();
    let scale = if opts.time_scale > 0.0 {
        opts.time_scale
    } else {
        1.0
    };
    let mut prev = opts.start;
    let mut total = 0.0;
    for (m, t) in job.moves.iter().zip(times) {
        let t = t * scale;
        let d = dist(prev, (m.x, m.y));
        prev = (m.x, m.y);
        total += t;
        let Some(l) = layers.get_mut(m.layer) else {
            continue;
        };
        l.seconds += t;
        if m.kind == MoveKind::Burn && m.power_pct != Some(0.0) {
            l.burn_seconds += t;
            l.burn_mm += d;
        } else {
            l.travel_mm += d;
        }
    }
    Estimate {
        seconds: total,
        layers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{Layer, Move};

    fn profile(accel: Option<f64>) -> MachineProfile {
        let accel = accel
            .map(|a| format!("accel_mm_s2={a}\n"))
            .unwrap_or_default();
        MachineProfile::from_toml(&format!(
            "[machine]\nname='t'\ndriver='mock'\nlaser='co2'\n\
             [work_area]\nwidth_mm=300\nheight_mm=200\n\
             [limits]\nmax_power_pct=100\nmax_speed_mm_s=400\n{accel}\
             [framing]\ndefault_source='laser_off'\n"
        ))
        .unwrap()
    }

    fn job(moves: Vec<Move>) -> Job {
        Job {
            layers: vec![Layer {
                name: "cut".into(),
                speed_mm_s: 10.0,
                ..Default::default()
            }],
            moves,
        }
    }

    #[test]
    fn constant_speed_without_acceleration() {
        let j = job(vec![Move::burn(100.0, 0.0, 0)]);
        let e = estimate(&j, &profile(None), &EstimateOptions::default());
        assert!((e.seconds - 10.0).abs() < 1e-9);
        assert!((e.layers[0].burn_mm - 100.0).abs() < 1e-9);
    }

    #[test]
    fn acceleration_adds_time() {
        // 10 mm/s with 100 mm/s²: 0.1 s and 0.5 mm to reach speed, same to stop.
        let j = job(vec![Move::burn(100.0, 0.0, 0)]);
        let e = estimate(&j, &profile(Some(100.0)), &EstimateOptions::default());
        assert!((e.seconds - 10.1).abs() < 1e-9, "{}", e.seconds);
    }

    #[test]
    fn short_move_never_reaches_cruise() {
        // 1 mm at 100 mm/s² from rest to rest: 2 * sqrt(0.5 / 100 * 2) / 2 ... = 0.2 s.
        let t = trapezoid(1.0, 0.0, 0.0, 400.0, 100.0);
        assert!((t - 0.2).abs() < 1e-9, "{t}");
    }

    #[test]
    fn straight_segments_do_not_stop_but_corners_slow_down() {
        let p = profile(Some(100.0));
        let straight = job(vec![Move::burn(50.0, 0.0, 0), Move::burn(100.0, 0.0, 0)]);
        let corner = job(vec![Move::burn(50.0, 0.0, 0), Move::burn(50.0, 50.0, 0)]);
        let ts = estimate(&straight, &p, &EstimateOptions::default()).seconds;
        let tc = estimate(&corner, &p, &EstimateOptions::default()).seconds;
        assert!((ts - 10.1).abs() < 1e-9, "{ts}");
        assert!(tc > ts);
    }

    #[test]
    fn travel_uses_profile_max_and_calibration_scales() {
        let j = job(vec![Move::travel(400.0, 0.0, 0)]);
        let opts = EstimateOptions {
            time_scale: 1.5,
            ..Default::default()
        };
        let e = estimate(&j, &profile(None), &opts);
        assert!((e.seconds - 1.5).abs() < 1e-9);
        assert_eq!(e.layers[0].burn_seconds, 0.0);
    }

    #[test]
    fn zero_power_burns_count_as_travel() {
        let j = job(vec![Move::burn(10.0, 0.0, 0).with_power(0.0)]);
        let e = estimate(&j, &profile(None), &EstimateOptions::default());
        assert_eq!(e.layers[0].burn_mm, 0.0);
        assert!((e.layers[0].travel_mm - 10.0).abs() < 1e-9);
    }
}
