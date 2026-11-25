// SPDX-License-Identifier: AGPL-3.0-or-later
//! Turn a planned [`Job`] into a Ruida `.rd` command stream.
//!
//! The layout (header, per-layer settings, moves, file sum, end of file)
//! follows MeerK40t's `RDJob.write_header` / `write_settings` / `write_tail`
//! (Copyright (c) 2021 meerk40t, MIT License), trimmed to the commands the
//! board needs.
//!
//! Safety: power for every layer is clamped to the profile's
//! `limits.max_power_pct`. If that clamp leaves a layer at zero power, its
//! burn moves are sent as plain moves, so no cut opcode reaches the board.
//! HIL profiles cap power at zero, so a HIL job never contains a cut.

use libreburn_core::{Error, ErrorCode, Job, MachineProfile, Result, effective_power_pct};

use crate::codec::{self, REL_LIMIT_UM, cmd, encode_coord, encode_power, encode_speed};
use crate::protocol::MAX_PACKET_PAYLOAD;

/// Machine geometry the encoder needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// Work area width in mm.
    pub width_mm: f64,
    /// Work area height in mm.
    pub height_mm: f64,
    /// Mirror X (machines whose origin is at the right). Set `mirror_x = true`
    /// in the profile's `[connection]` table.
    pub mirror_x: bool,
}

impl Geometry {
    /// Geometry from a profile.
    pub fn from_profile(profile: &MachineProfile) -> Self {
        Self {
            width_mm: profile.work_area.width_mm,
            height_mm: profile.work_area.height_mm,
            mirror_x: profile
                .connection
                .get("mirror_x")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }
    }

    /// Job coordinates (mm, origin top left) to machine µm.
    pub fn to_machine(&self, x: f64, y: f64) -> (i32, i32) {
        let x = if self.mirror_x { self.width_mm - x } else { x };
        ((x * 1000.0).round() as i32, (y * 1000.0).round() as i32)
    }

    /// Machine µm to job coordinates in mm.
    pub fn from_machine(&self, x_um: i32, y_um: i32) -> (f64, f64) {
        let x = f64::from(x_um) / 1000.0;
        let x = if self.mirror_x { self.width_mm - x } else { x };
        (x, f64::from(y_um) / 1000.0)
    }

    /// Refuse points outside the work area.
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
                "Move outside the work area",
                format!(
                    "({x:.3}, {y:.3}) mm is outside 0..{} x 0..{} mm",
                    self.width_mm, self.height_mm
                ),
                "move the design inside the work area or fix work_area in the profile",
            ));
        }
        Ok(())
    }
}

fn bad_job(why: String) -> Error {
    Error::new(
        ErrorCode::ProtocolViolation,
        "Job could not be encoded for the Ruida controller",
        why,
        "re-plan the job; if it keeps happening, report a bug with the job file",
    )
}

struct Writer {
    cmds: Vec<Vec<u8>>,
    pos: Option<(i32, i32)>,
    /// Power currently set on the board, percent.
    power: f64,
}

impl Writer {
    fn new() -> Self {
        Self {
            cmds: Vec::new(),
            pos: None,
            power: 0.0,
        }
    }

    fn push(&mut self, c: Vec<u8>) {
        self.cmds.push(c);
    }

    /// Set laser power for the following cuts. Per-move power changes
    /// (grayscale) reuse the layer power commands; unconfirmed on hardware.
    fn set_power(&mut self, pct: f64) {
        let pw = encode_power(pct);
        for op in [
            codec::power::MIN_POWER_1,
            codec::power::MAX_POWER_1,
            codec::power::MIN_POWER_2,
            codec::power::MAX_POWER_2,
        ] {
            self.push(cmd(&[&op, &pw]));
        }
        self.power = pct;
    }

    fn travel(&mut self, x: i32, y: i32, cut: bool) {
        let (abs, rel, rel_x, rel_y) = if cut {
            (
                codec::CUT_ABS_XY,
                codec::CUT_REL_XY,
                codec::CUT_REL_X,
                codec::CUT_REL_Y,
            )
        } else {
            (
                codec::MOVE_ABS_XY,
                codec::MOVE_REL_XY,
                codec::MOVE_REL_X,
                codec::MOVE_REL_Y,
            )
        };
        let c = match self.pos {
            Some((px, py)) => {
                let (dx, dy) = (x - px, y - py);
                if dx == 0 && dy == 0 {
                    return;
                }
                if dx.abs() > REL_LIMIT_UM || dy.abs() > REL_LIMIT_UM {
                    cmd(&[&[abs], &encode_coord(x), &encode_coord(y)])
                } else if dy == 0 {
                    cmd(&[&[rel_x], &codec::encode14(dx)])
                } else if dx == 0 {
                    cmd(&[&[rel_y], &codec::encode14(dy)])
                } else {
                    cmd(&[&[rel], &codec::encode14(dx), &codec::encode14(dy)])
                }
            }
            // The first move of a file is always absolute.
            None => cmd(&[&[abs], &encode_coord(x), &encode_coord(y)]),
        };
        self.push(c);
        self.pos = Some((x, y));
    }
}

struct Part {
    index: u8,
    speed_mm_s: f64,
    power_pct: f64,
    /// Some move in this layer fires the laser.
    fires: bool,
    min: (i32, i32),
    max: (i32, i32),
}

fn header(w: &mut Writer, parts: &[Part]) {
    let min = parts.iter().fold((i32::MAX, i32::MAX), |a, p| {
        (a.0.min(p.min.0), a.1.min(p.min.1))
    });
    let max = parts.iter().fold((i32::MIN, i32::MIN), |a, p| {
        (a.0.max(p.max.0), a.1.max(p.max.1))
    });
    w.push(codec::REF_POINT_2.to_vec());
    w.push(codec::SET_ABSOLUTE.to_vec());
    w.push(vec![codec::REF_POINT_SET]);
    w.push(cmd(&[&codec::ENABLE_BLOCK_CUTTING, &[0]]));
    w.push(codec::START_PROCESS.to_vec());
    let pt = |op: [u8; 2], p: (i32, i32)| cmd(&[&op, &encode_coord(p.0), &encode_coord(p.1)]);
    w.push(pt(codec::PROCESS_TOP_LEFT, min));
    w.push(pt(codec::PROCESS_BOTTOM_RIGHT, max));
    w.push(pt(codec::DOCUMENT_MIN_POINT, min));
    w.push(pt(codec::DOCUMENT_MAX_POINT, max));
    for p in parts {
        let pw = encode_power(p.power_pct);
        w.push(cmd(&[
            &codec::SPEED_LASER_1_PART,
            &[p.index],
            &encode_speed(p.speed_mm_s),
        ]));
        for op in [
            codec::power::MIN_POWER_1_PART,
            codec::power::MAX_POWER_1_PART,
            codec::power::MIN_POWER_2_PART,
            codec::power::MAX_POWER_2_PART,
        ] {
            w.push(cmd(&[&op, &[p.index], &pw]));
        }
        w.push(cmd(&[
            &codec::LAYER_COLOR_PART,
            &[p.index],
            &codec::encode35(0),
        ]));
        w.push(cmd(&[&codec::WORK_MODE_PART, &[p.index], &[0]]));
        let ppt = |op: [u8; 2], q: (i32, i32)| {
            cmd(&[&op, &[p.index], &encode_coord(q.0), &encode_coord(q.1)])
        };
        w.push(ppt(codec::PART_MIN_POINT, p.min));
        w.push(ppt(codec::PART_MAX_POINT, p.max));
    }
    let last = parts.last().map_or(0, |p| p.index);
    w.push(cmd(&[&codec::MAX_LAYER_PART, &[last]]));
}

fn layer_settings(w: &mut Writer, p: &Part) {
    w.push(cmd(&[&codec::LAYER_NUMBER_PART, &[p.index]]));
    w.push(codec::LASER_DEVICE_0.to_vec());
    w.push(cmd(&[&codec::SPEED_LASER_1, &encode_speed(p.speed_mm_s)]));
    w.push(cmd(&[&codec::power::LASER_ON_DELAY, &codec::encode35(0)]));
    w.push(cmd(&[&codec::power::LASER_OFF_DELAY, &codec::encode35(0)]));
    w.set_power(p.power_pct);
    let fire = u8::from(p.fires);
    w.push(cmd(&[&codec::EN_LASER_TUBE_START, &[fire]]));
}

fn layer_end(w: &mut Writer) {
    w.push(codec::BLOCK_END.to_vec());
    w.push(codec::LAYER_END.to_vec());
    w.push(codec::EN_LASER_2_OFFSET_0.to_vec());
}

fn tail(w: &mut Writer) {
    layer_end(w);
    let sum: u64 =
        w.cmds.iter().flatten().map(|&b| u64::from(b)).sum::<u64>() + u64::from(codec::END_OF_FILE);
    w.push(cmd(&[&codec::SET_FILE_SUM, &codec::encode35(sum as i64)]));
    w.push(vec![codec::END_OF_FILE]);
}

/// Encode a job into Ruida commands (unscrambled), one `Vec` per command.
pub fn encode_job(job: &Job, profile: &MachineProfile) -> Result<Vec<Vec<u8>>> {
    let geo = Geometry::from_profile(profile);
    if job.moves.is_empty() {
        return Err(bad_job("the job has no moves".into()));
    }
    if job.layers.len() > 255 {
        return Err(bad_job(format!(
            "the job has {} layers; Ruida supports 255",
            job.layers.len()
        )));
    }
    let max_speed = profile.limits.max_speed_mm_s;
    let mut parts: Vec<Option<Part>> = (0..job.layers.len()).map(|_| None).collect();
    let mut targets = Vec::with_capacity(job.moves.len());
    for m in &job.moves {
        let layer = job.layers.get(m.layer).ok_or_else(|| {
            bad_job(format!(
                "a move uses layer {} but the job has {} layers",
                m.layer,
                job.layers.len()
            ))
        })?;
        geo.check(m.x, m.y)?;
        let p = geo.to_machine(m.x, m.y);
        let part = parts[m.layer].get_or_insert_with(|| Part {
            index: m.layer as u8,
            speed_mm_s: layer.speed_mm_s.clamp(0.0, max_speed),
            power_pct: effective_power_pct(layer.power_pct, profile),
            fires: false,
            min: p,
            max: p,
        });
        part.min = (part.min.0.min(p.0), part.min.1.min(p.1));
        part.max = (part.max.0.max(p.0), part.max.1.max(p.1));
        // Travel is 0; burns use the move or layer power, clamped to the cap.
        let power = job.power_for(m, profile);
        part.fires |= power > 0.0;
        targets.push((m.layer, p, power));
    }
    let used: Vec<Part> = parts.into_iter().flatten().collect();
    let mut w = Writer::new();
    header(&mut w, &used);
    let mut current: Option<usize> = None;
    for (layer, (x, y), power) in targets {
        if current != Some(layer) {
            if current.is_some() {
                layer_end(&mut w);
            }
            let part = used
                .iter()
                .find(|p| usize::from(p.index) == layer)
                .expect("every used layer has a part");
            layer_settings(&mut w, part);
            current = Some(layer);
        }
        let cut = power > 0.0;
        if cut && power != w.power {
            w.set_power(power);
        }
        w.travel(x, y, cut);
    }
    tail(&mut w);
    Ok(w.cmds)
}

/// Encode a laser-off frame: a job that only moves the head around `outline`.
///
/// It contains no cut opcodes and every power setting is zero, so the tube
/// cannot fire whatever the profile allows.
pub fn encode_frame(
    outline: &[(f64, f64)],
    profile: &MachineProfile,
    speed_mm_s: f64,
) -> Result<Vec<Vec<u8>>> {
    let geo = Geometry::from_profile(profile);
    let first = *outline
        .first()
        .ok_or_else(|| bad_job("the frame outline is empty".into()))?;
    let mut pts = Vec::with_capacity(outline.len() + 1);
    for &(x, y) in outline.iter().chain(std::iter::once(&first)) {
        geo.check(x, y)?;
        pts.push(geo.to_machine(x, y));
    }
    let min = pts
        .iter()
        .fold((i32::MAX, i32::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1)));
    let max = pts
        .iter()
        .fold((i32::MIN, i32::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1)));
    let part = Part {
        index: 0,
        speed_mm_s: speed_mm_s.clamp(0.0, profile.limits.max_speed_mm_s),
        power_pct: 0.0,
        fires: false,
        min,
        max,
    };
    let mut w = Writer::new();
    header(&mut w, std::slice::from_ref(&part));
    layer_settings(&mut w, &part);
    for (x, y) in pts {
        w.travel(x, y, false);
    }
    tail(&mut w);
    Ok(w.cmds)
}

/// Group commands into packets of at most [`MAX_PACKET_PAYLOAD`] bytes,
/// never splitting a command.
pub fn chunk(cmds: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    for c in cmds {
        if !cur.is_empty() && cur.len() + c.len() > MAX_PACKET_PAYLOAD {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend_from_slice(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use libreburn_core::{Layer, LayerMode, Move, MoveKind};

    pub(crate) fn profile(max_power: f64) -> MachineProfile {
        let text = include_str!("../../../profiles/omtech-k40-ruida.toml").replace(
            "max_power_pct = 70",
            &format!("max_power_pct = {max_power}"),
        );
        MachineProfile::from_toml(&text).unwrap()
    }

    fn mv(kind: MoveKind, x: f64, y: f64) -> Move {
        match kind {
            MoveKind::Travel => Move::travel(x, y, 0),
            MoveKind::Burn => Move::burn(x, y, 0),
        }
    }

    fn square(power: f64) -> Job {
        Job {
            layers: vec![Layer {
                name: "cut".into(),
                mode: LayerMode::Cut,
                speed_mm_s: 20.0,
                power_pct: power,
                ..Default::default()
            }],
            moves: vec![
                mv(MoveKind::Travel, 10.0, 10.0),
                mv(MoveKind::Burn, 20.0, 10.0),
                mv(MoveKind::Burn, 20.0, 20.0),
                mv(MoveKind::Burn, 10.0, 20.0),
                mv(MoveKind::Burn, 10.0, 10.0),
            ],
        }
    }

    fn cuts(cmds: &[Vec<u8>]) -> usize {
        cmds.iter()
            .filter(|c| (0xA8..=0xAB).contains(&c[0]))
            .count()
    }

    #[test]
    fn square_has_expected_moves_and_power() {
        let cmds = encode_job(&square(40.0), &profile(70.0)).unwrap();
        assert_eq!(cmds.first().unwrap(), &codec::REF_POINT_2.to_vec());
        assert_eq!(cmds.last().unwrap(), &vec![codec::END_OF_FILE]);
        // First move absolute to (10 mm, 10 mm); 10 000 µm = 0x2710.
        assert!(cmds.contains(&vec![0x88, 0, 0, 0, 0x4E, 0x10, 0, 0, 0, 0x4E, 0x10]));
        // +10 mm in X is 10 000 µm > 8191, so it is absolute.
        assert_eq!(cuts(&cmds), 4);
        assert!(cmds.contains(&cmd(&[&codec::power::MAX_POWER_1, &encode_power(40.0)])));
    }

    #[test]
    fn small_steps_use_relative_moves() {
        let mut job = square(10.0);
        job.moves = vec![
            mv(MoveKind::Travel, 1.0, 1.0),
            mv(MoveKind::Burn, 2.0, 1.0),
            mv(MoveKind::Burn, 2.0, 2.5),
            mv(MoveKind::Burn, 1.0, 1.0),
        ];
        let cmds = encode_job(&job, &profile(70.0)).unwrap();
        // 1000 µm = 0x3E8 -> 07 68.
        assert!(cmds.contains(&vec![codec::CUT_REL_X, 0x07, 0x68]));
        assert!(cmds.contains(&vec![codec::CUT_REL_Y, 0x0B, 0x5C]));
        let diag = cmd(&[
            &[codec::CUT_REL_XY],
            &codec::encode14(-1000),
            &codec::encode14(-1500),
        ]);
        assert!(cmds.contains(&diag));
    }

    #[test]
    fn power_is_capped_by_the_profile() {
        let cmds = encode_job(&square(95.0), &profile(70.0)).unwrap();
        assert!(cmds.contains(&cmd(&[&codec::power::MAX_POWER_1, &encode_power(70.0)])));
        assert!(!cmds.contains(&cmd(&[&codec::power::MAX_POWER_1, &encode_power(95.0)])));
    }

    #[test]
    fn zero_power_cap_sends_no_cut_commands() {
        let cmds = encode_job(&square(80.0), &profile(0.0)).unwrap();
        assert_eq!(cuts(&cmds), 0);
        assert!(cmds.contains(&cmd(&[&codec::EN_LASER_TUBE_START, &[0]])));
    }

    #[test]
    fn per_move_power_overrides_are_capped() {
        let mut job = square(30.0);
        job.moves[2] = job.moves[2].clone().with_power(90.0);
        job.moves[3] = job.moves[3].clone().with_power(10.0);
        let cmds = encode_job(&job, &profile(70.0)).unwrap();
        let max1 = |p: f64| cmd(&[&codec::power::MAX_POWER_1, &encode_power(p)]);
        assert!(cmds.contains(&max1(70.0)));
        assert!(cmds.contains(&max1(10.0)));
        assert!(!cmds.contains(&max1(90.0)));
    }

    #[test]
    fn file_sum_covers_every_byte() {
        let cmds = encode_job(&square(30.0), &profile(70.0)).unwrap();
        let n = cmds.len();
        let body: u64 = cmds[..n - 2].iter().flatten().map(|&b| u64::from(b)).sum();
        let sum_cmd = &cmds[n - 2];
        assert_eq!(&sum_cmd[..2], &codec::SET_FILE_SUM);
        assert_eq!(codec::decode_u35(&sum_cmd[2..]), body + 0xD7);
    }

    #[test]
    fn moves_outside_the_bed_are_refused() {
        let mut job = square(30.0);
        job.moves.push(mv(MoveKind::Burn, 301.0, 10.0));
        let err = encode_job(&job, &profile(70.0)).unwrap_err();
        assert_eq!(err.code, ErrorCode::SafetyBlocked);
    }

    #[test]
    fn mirror_x_flips_coordinates() {
        let mut p = profile(70.0);
        p.connection
            .insert("mirror_x".into(), toml::Value::Boolean(true));
        let g = Geometry::from_profile(&p);
        assert_eq!(g.to_machine(10.0, 5.0), (290_000, 5_000));
        assert_eq!(g.from_machine(290_000, 5_000), (10.0, 5.0));
    }

    #[test]
    fn frame_has_no_cuts_and_zero_power() {
        let cmds = encode_frame(
            &[(10.0, 10.0), (50.0, 10.0), (50.0, 40.0)],
            &profile(70.0),
            100.0,
        )
        .unwrap();
        assert_eq!(cuts(&cmds), 0);
        assert!(cmds.contains(&cmd(&[&codec::power::MAX_POWER_1, &encode_power(0.0)])));
    }

    #[test]
    fn chunks_respect_the_size_limit_and_command_boundaries() {
        let mut job = square(30.0);
        for i in 0..2000 {
            let x = 10.0 + f64::from(i % 100) * 0.1;
            job.moves
                .push(mv(MoveKind::Burn, x, 10.0 + f64::from(i / 100)));
        }
        let cmds = encode_job(&job, &profile(70.0)).unwrap();
        let packets = chunk(&cmds);
        assert!(packets.len() > 1);
        assert!(
            packets
                .iter()
                .all(|p| p.len() <= MAX_PACKET_PAYLOAD && p[0] >= 0x80)
        );
        assert_eq!(packets.concat(), cmds.concat());
    }
}
