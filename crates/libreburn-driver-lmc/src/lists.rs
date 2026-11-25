// SPDX-License-Identifier: AGPL-3.0-or-later
//! Build LMC command lists for jobs and red-light frames.
//!
//! The list layouts follow MeerK40t's `GalvoController.program_mode`,
//! `light_mode` and `set_settings` (Copyright (c) 2021 meerk40t, MIT License).
//!
//! Safety rules enforced here, and checked again by the simulator tests:
//! - A frame list never contains `MARK_TO` or `LASER_ON_POINT`, and every
//!   port write in it sets only the guide-light bit.
//! - The guide-light bit may not be the laser-enable bit (this is the
//!   misconfiguration that makes "framing" fire the fiber source).
//! - Job power goes through `Job::power_for`, so it never exceeds
//!   `limits.max_power_pct`. If nothing in the job fires (a HIL profile's
//!   zero cap), the list has no `MARK_TO` and never sets the enable bit.

use libreburn_core::{
    Error, ErrorCode, Job, MachineProfile, MoveKind, Result, effective_power_pct,
};

use crate::protocol::{Command, command, list, power_to_current, qswitch_period};

/// Galvo field mapping and timing settings, from the profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Field width, mm (the lens's marking area).
    pub width_mm: f64,
    /// Field height, mm.
    pub height_mm: f64,
    /// Mirror X.
    pub mirror_x: bool,
    /// Mirror Y.
    pub mirror_y: bool,
    /// Swap X and Y.
    pub swap_xy: bool,
    /// Output bit of the red guide light.
    pub guide_light_bit: Option<u8>,
    /// Output bit that enables the fiber source (MeerK40t uses bit 0).
    pub laser_enable_bit: u8,
    /// Jump (travel) speed, mm/s.
    pub rapid_speed_mm_s: f64,
    /// Jump speed while framing, mm/s.
    pub light_speed_mm_s: f64,
    /// Pulse frequency when a layer sets none, kHz.
    pub default_frequency_khz: f64,
    /// Laser on/off delay, µs.
    pub laser_delay_us: u16,
    /// Polygon delay, µs.
    pub polygon_delay_us: u16,
    /// Wait after opening the master oscillator, ms.
    pub open_mo_delay_ms: f64,
}

fn invalid(why: String, hint: &str) -> Error {
    Error::new(
        ErrorCode::ProfileInvalid,
        "Invalid LMC connection settings",
        why,
        hint,
    )
}

impl Settings {
    /// Read settings from the profile's `[work_area]`, `[framing]` and
    /// `[connection]` tables. Unknown connection keys are ignored here.
    pub fn from_profile(profile: &MachineProfile) -> Result<Self> {
        let c = &profile.connection;
        let flag = |k: &str| c.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
        let num = |k: &str, d: f64| -> Result<f64> {
            match c.get(k) {
                None => Ok(d),
                Some(v) => v
                    .as_float()
                    .or_else(|| v.as_integer().map(|i| i as f64))
                    .filter(|f| f.is_finite() && *f >= 0.0)
                    .ok_or_else(|| {
                        invalid(
                            format!("connection.{k} is {v}"),
                            "use a non-negative number",
                        )
                    }),
            }
        };
        let laser_enable_bit = num("laser_enable_port", 0.0)?;
        if laser_enable_bit > 15.0 || laser_enable_bit.fract() != 0.0 {
            return Err(invalid(
                format!("connection.laser_enable_port is {laser_enable_bit}"),
                "use an output bit from 0 to 15",
            ));
        }
        let s = Self {
            width_mm: profile.work_area.width_mm,
            height_mm: profile.work_area.height_mm,
            mirror_x: flag("mirror_x"),
            mirror_y: flag("mirror_y"),
            swap_xy: flag("swap_xy"),
            guide_light_bit: profile.framing.guide_light_port,
            laser_enable_bit: laser_enable_bit as u8,
            rapid_speed_mm_s: num("rapid_speed_mm_s", 2000.0)?,
            light_speed_mm_s: num("light_speed_mm_s", 2000.0)?,
            default_frequency_khz: num("default_frequency_khz", 30.0)?,
            laser_delay_us: num("laser_delay_us", 100.0)?.min(65535.0) as u16,
            polygon_delay_us: num("polygon_delay_us", 100.0)?.min(655_350.0) as u16,
            open_mo_delay_ms: num("open_mo_delay_ms", 8.0)?,
        };
        if s.width_mm <= 0.0 || s.height_mm <= 0.0 {
            return Err(invalid(
                "the work area is empty".into(),
                "set work_area to the lens field size",
            ));
        }
        Ok(s)
    }

    /// The guide-light port mask, after the safety checks.
    pub fn guide_light_mask(&self) -> Result<u16> {
        let bit = self.guide_light_bit.ok_or_else(|| {
            Error::new(
                ErrorCode::SafetyBlocked,
                "Galvo framing was blocked",
                "the profile has no framing.guide_light_port",
                "set the guide light's output port in the machine profile",
            )
        })?;
        if bit > 15 {
            return Err(invalid(
                format!("framing.guide_light_port is {bit}"),
                "use an output bit from 0 to 15",
            ));
        }
        if bit == self.laser_enable_bit {
            return Err(Error::new(
                ErrorCode::SafetyBlocked,
                "Galvo framing was blocked",
                format!(
                    "framing.guide_light_port ({bit}) is the laser-enable output, \
                     so turning the \"light\" on would fire the fiber source"
                ),
                "set guide_light_port to the bit your red diode is wired to (often 8); \
                 check it in EZCAD's markcfg or with the board's wiring diagram",
            ));
        }
        Ok(1 << bit)
    }

    /// The laser-enable port mask.
    pub fn laser_enable_mask(&self) -> u16 {
        1 << self.laser_enable_bit
    }

    /// Galvo units per mm along X.
    fn units_per_mm(&self) -> f64 {
        65536.0 / self.width_mm
    }

    /// Convert mm/s to the board's galvo units per ms.
    pub fn speed(&self, mm_s: f64) -> u16 {
        (mm_s * self.units_per_mm() / 1000.0)
            .round()
            .clamp(1.0, 65535.0) as u16
    }

    /// Check a point is inside the field.
    pub fn check(&self, x: f64, y: f64) -> Result<()> {
        let eps = 1e-6;
        if !(x.is_finite() && y.is_finite())
            || x < -eps
            || y < -eps
            || x > self.width_mm + eps
            || y > self.height_mm + eps
        {
            return Err(Error::new(
                ErrorCode::SafetyBlocked,
                "Move outside the galvo field",
                format!(
                    "({x:.3}, {y:.3}) mm is outside 0..{} x 0..{} mm",
                    self.width_mm, self.height_mm
                ),
                "move the design inside the field or fix work_area in the profile",
            ));
        }
        Ok(())
    }

    /// Job mm (origin top left) to galvo units 0..=0xFFFF.
    pub fn to_galvo(&self, x: f64, y: f64) -> (u16, u16) {
        let gx = (x / self.width_mm * 65535.0).round().clamp(0.0, 65535.0) as u16;
        let gy = (y / self.height_mm * 65535.0).round().clamp(0.0, 65535.0) as u16;
        let gx = if self.mirror_x { 0xFFFF - gx } else { gx };
        let gy = if self.mirror_y { 0xFFFF - gy } else { gy };
        if self.swap_xy { (gy, gx) } else { (gx, gy) }
    }

    /// Galvo units back to job mm.
    pub fn from_galvo(&self, gx: u16, gy: u16) -> (f64, f64) {
        let (gx, gy) = if self.swap_xy { (gy, gx) } else { (gx, gy) };
        let gx = if self.mirror_x { 0xFFFF - gx } else { gx };
        let gy = if self.mirror_y { 0xFFFF - gy } else { gy };
        (
            f64::from(gx) / 65535.0 * self.width_mm,
            f64::from(gy) / 65535.0 * self.height_mm,
        )
    }
}

fn distance(a: (u16, u16), b: (u16, u16)) -> u16 {
    let dx = f64::from(a.0) - f64::from(b.0);
    let dy = f64::from(a.1) - f64::from(b.1);
    dx.hypot(dy).min(65535.0) as u16
}

fn signed_delay(op: u16, us: u16) -> Command {
    command(op, [us, 0, 0, 0, 0])
}

/// A built job list.
#[derive(Debug, Clone, PartialEq)]
pub struct JobList {
    /// List commands, ending with `END_OF_LIST`.
    pub commands: Vec<Command>,
    /// Whether any command fires the laser. If not, the driver never opens
    /// the master oscillator or sets the laser-enable bit.
    pub fires: bool,
    /// Beam position at the end, galvo units.
    pub end: (u16, u16),
}

/// Build the list for a job, starting from galvo position `start`.
pub fn job_list(
    job: &Job,
    profile: &MachineProfile,
    s: &Settings,
    start: (u16, u16),
) -> Result<JobList> {
    if job.moves.is_empty() {
        return Err(Error::new(
            ErrorCode::ProtocolViolation,
            "Job could not be encoded for the LMC board",
            "the job has no moves",
            "re-plan the job",
        ));
    }
    let mut powers = Vec::with_capacity(job.moves.len());
    for m in &job.moves {
        if m.layer >= job.layers.len() {
            return Err(Error::new(
                ErrorCode::ProtocolViolation,
                "Job could not be encoded for the LMC board",
                format!(
                    "a move uses layer {} but the job has {} layers",
                    m.layer,
                    job.layers.len()
                ),
                "re-plan the job",
            ));
        }
        s.check(m.x, m.y)?;
        powers.push(job.power_for(m, profile));
    }
    let fires = powers.iter().any(|&p| p > 0.0);
    let mut out = vec![command(list::READY_MARK, [0; 5])];
    if fires {
        let wait = (s.open_mo_delay_ms * 100.0).round().clamp(0.0, 65535.0) as u16;
        if wait > 0 {
            out.push(command(list::DELAY_TIME, [wait, 0, 0, 0, 0]));
        }
    }
    let port = if fires { s.laser_enable_mask() } else { 0 };
    out.push(command(list::WRITE_PORT, [port, 0, 0, 0, 0]));
    let rapid = s.speed(s.rapid_speed_mm_s);
    out.push(command(list::JUMP_SPEED, [rapid, 0, 0, 0, 0]));
    let mut jump_speed = rapid;
    let max_speed = profile.limits.max_speed_mm_s;
    let mut mark_speed = rapid;
    let mut pos = start;
    let mut layer_now: Option<usize> = None;
    let mut current: Option<u16> = None;
    for (m, &power) in job.moves.iter().zip(&powers) {
        if layer_now != Some(m.layer) {
            let layer = &job.layers[m.layer];
            let freq = layer.frequency_khz.unwrap_or(s.default_frequency_khz);
            out.push(command(
                list::QSWITCH_PERIOD,
                [qswitch_period(freq), 0, 0, 0, 0],
            ));
            let lp = power_to_current(effective_power_pct(layer.power_pct, profile));
            out.push(command(list::MARK_CURRENT, [lp, 0, 0, 0, 0]));
            current = Some(lp);
            if let Some(ns) = layer.pulse_width_ns {
                out.push(command(
                    list::FIBER_YLPM_PULSE_WIDTH,
                    [ns.min(65535) as u16, 0, 0, 0, 0],
                ));
            }
            mark_speed = s.speed(layer.speed_mm_s.clamp(0.0, max_speed));
            out.push(command(list::MARK_SPEED, [mark_speed, 0, 0, 0, 0]));
            out.push(signed_delay(list::LASER_ON_DELAY, s.laser_delay_us));
            out.push(signed_delay(list::LASER_OFF_DELAY, s.laser_delay_us));
            out.push(command(
                list::POLYGON_DELAY,
                [s.polygon_delay_us / 10, 0, 0, 0, 0],
            ));
            layer_now = Some(m.layer);
        }
        let target = s.to_galvo(m.x, m.y);
        if target == pos {
            continue;
        }
        let d = distance(pos, target);
        if power > 0.0 {
            let c = power_to_current(power);
            if current != Some(c) {
                out.push(command(list::MARK_CURRENT, [c, 0, 0, 0, 0]));
                current = Some(c);
            }
            out.push(command(list::MARK_TO, [target.0, target.1, 0, d, 0]));
        } else {
            // Travel uses the rapid speed. A burn at zero power (raster gaps
            // and overscan) is a laser-off jump at the layer's mark speed.
            let want = if m.kind == MoveKind::Burn {
                mark_speed
            } else {
                rapid
            };
            if want != jump_speed {
                out.push(command(list::JUMP_SPEED, [want, 0, 0, 0, 0]));
                jump_speed = want;
            }
            out.push(command(list::JUMP_TO, [target.0, target.1, 0, d, 0]));
        }
        pos = target;
    }
    out.push(command(list::END_OF_LIST, [0; 5]));
    Ok(JobList {
        commands: out,
        fires,
        end: pos,
    })
}

/// Build a red-light frame list: jumps only, with only the guide light on.
///
/// The outline is traced `passes` times and closed back to its first point.
/// The light is switched off at the end of the list.
pub fn frame_list(
    outline: &[(f64, f64)],
    s: &Settings,
    start: (u16, u16),
    passes: u32,
) -> Result<(Vec<Command>, (u16, u16))> {
    let light = s.guide_light_mask()?;
    let first = *outline.first().ok_or_else(|| {
        Error::new(
            ErrorCode::ProtocolViolation,
            "Frame could not be encoded",
            "the frame outline is empty",
            "frame a job with at least one point",
        )
    })?;
    let mut pts = Vec::with_capacity(outline.len() + 1);
    for &(x, y) in outline.iter().chain(std::iter::once(&first)) {
        s.check(x, y)?;
        pts.push(s.to_galvo(x, y));
    }
    let mut out = vec![
        command(list::READY_MARK, [0; 5]),
        command(list::WRITE_PORT, [0, 0, 0, 0, 0]),
        command(list::JUMP_SPEED, [s.speed(s.light_speed_mm_s), 0, 0, 0, 0]),
    ];
    let mut pos = start;
    // Move to the start with the light off, then light up and trace.
    let p0 = pts[0];
    out.push(command(
        list::JUMP_TO,
        [p0.0, p0.1, 0, distance(pos, p0), 0],
    ));
    pos = p0;
    out.push(command(list::WRITE_PORT, [light, 0, 0, 0, 0]));
    for _ in 0..passes.max(1) {
        for &p in &pts[1..] {
            out.push(command(list::JUMP_TO, [p.0, p.1, 0, distance(pos, p), 0]));
            pos = p;
        }
    }
    out.push(command(list::WRITE_PORT, [0, 0, 0, 0, 0]));
    out.push(command(list::END_OF_LIST, [0; 5]));
    Ok((out, pos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::decode;
    use libreburn_core::{Layer, LayerMode, Move};

    fn profile() -> MachineProfile {
        MachineProfile::from_toml(include_str!("../../../profiles/jcz-lmc-fiber.toml")).unwrap()
    }

    fn ops(cmds: &[Command]) -> Vec<u16> {
        cmds.iter().map(|c| decode(c).0).collect()
    }

    fn job(power: f64) -> Job {
        Job {
            layers: vec![Layer {
                name: "mark".into(),
                mode: LayerMode::GalvoHatch,
                speed_mm_s: 500.0,
                power_pct: power,
                frequency_khz: Some(30.0),
                ..Default::default()
            }],
            moves: vec![
                Move::travel(10.0, 10.0, 0),
                Move::burn(20.0, 10.0, 0),
                Move::burn(20.0, 20.0, 0),
            ],
        }
    }

    #[test]
    fn frame_list_has_no_marks_and_only_the_light_bit() {
        let s = Settings::from_profile(&profile()).unwrap();
        let (cmds, _) = frame_list(&[(10.0, 10.0), (50.0, 50.0)], &s, (0x8000, 0x8000), 3).unwrap();
        assert!(!ops(&cmds).contains(&list::MARK_TO));
        assert!(!ops(&cmds).contains(&list::LASER_ON_POINT));
        for c in &cmds {
            let (op, a) = decode(c);
            if op == list::WRITE_PORT {
                assert!(a[0] == 0 || a[0] == 1 << 8, "port {:#x}", a[0]);
            }
        }
    }

    #[test]
    fn light_port_on_the_enable_bit_is_refused() {
        let text = include_str!("../../../profiles/jcz-lmc-fiber.toml")
            .replace("guide_light_port = 8", "guide_light_port = 0");
        let s = Settings::from_profile(&MachineProfile::from_toml(&text).unwrap()).unwrap();
        let err = frame_list(&[(1.0, 1.0)], &s, (0, 0), 1).unwrap_err();
        assert_eq!(err.code, ErrorCode::SafetyBlocked);
    }

    #[test]
    fn job_list_marks_with_capped_power() {
        let text = include_str!("../../../profiles/jcz-lmc-fiber.toml")
            .replace("max_power_pct = 100", "max_power_pct = 40");
        let p = MachineProfile::from_toml(&text).unwrap();
        let s = Settings::from_profile(&p).unwrap();
        let l = job_list(&job(90.0), &p, &s, (0x8000, 0x8000)).unwrap();
        assert!(l.fires);
        let cur: Vec<u16> = l
            .commands
            .iter()
            .map(|c| decode(c))
            .filter(|(op, _)| *op == list::MARK_CURRENT)
            .map(|(_, a)| a[0])
            .collect();
        assert_eq!(cur, vec![power_to_current(40.0)]);
        assert_eq!(
            ops(&l.commands)
                .iter()
                .filter(|&&o| o == list::MARK_TO)
                .count(),
            2
        );
        assert_eq!(*ops(&l.commands).last().unwrap(), list::END_OF_LIST);
    }

    #[test]
    fn zero_cap_has_no_marks_and_no_enable_bit() {
        let p =
            MachineProfile::from_toml(include_str!("../../../hil/profiles/hil-jcz-lmc-fiber.toml"))
                .unwrap();
        let s = Settings::from_profile(&p).unwrap();
        let l = job_list(&job(90.0), &p, &s, (0, 0)).unwrap();
        assert!(!l.fires);
        assert!(!ops(&l.commands).contains(&list::MARK_TO));
        for c in &l.commands {
            let (op, a) = decode(c);
            if op == list::WRITE_PORT {
                assert_eq!(a[0], 0);
            }
        }
    }

    #[test]
    fn zero_power_burns_are_slow_jumps() {
        let p = profile();
        let s = Settings::from_profile(&p).unwrap();
        let mut j = job(50.0);
        j.moves.push(Move::burn(30.0, 20.0, 0).with_power(0.0));
        j.moves.push(Move::travel(40.0, 40.0, 0));
        let l = job_list(&j, &p, &s, (0, 0)).unwrap();
        let tail: Vec<(u16, [u16; 5])> = l
            .commands
            .iter()
            .rev()
            .skip(1)
            .take(4)
            .map(|c| decode(c))
            .collect();
        // Reversed: travel jump, rapid speed, raster-gap jump, mark speed.
        assert_eq!(tail[0].0, list::JUMP_TO);
        assert_eq!(tail[1], (list::JUMP_SPEED, [s.speed(2000.0), 0, 0, 0, 0]));
        assert_eq!(tail[2].0, list::JUMP_TO);
        assert_eq!(tail[3], (list::JUMP_SPEED, [s.speed(500.0), 0, 0, 0, 0]));
    }

    #[test]
    fn field_mapping() {
        let s = Settings::from_profile(&profile()).unwrap();
        assert_eq!(s.to_galvo(0.0, 0.0), (0, 0));
        assert_eq!(s.to_galvo(110.0, 110.0), (0xFFFF, 0xFFFF));
        assert_eq!(s.to_galvo(55.0, 55.0), (32768, 32768));
        let (x, y) = s.from_galvo(32768, 0);
        assert!((x - 55.0).abs() < 0.01 && y == 0.0);
        // 1000 mm/s on a 110 mm field: 1000 * 65536 / 110 / 1000 = 596 units/ms.
        assert_eq!(s.speed(1000.0), 596);
        assert!(s.check(111.0, 5.0).is_err());
    }
}
