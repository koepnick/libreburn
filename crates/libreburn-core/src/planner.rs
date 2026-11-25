// SPDX-License-Identifier: AGPL-3.0-or-later
//! Turns a [`Project`] into a device-neutral [`Job`], checks it against the
//! machine's limits (FR-17, FR-18) and estimates its run time (FR-13).
//!
//! Move conventions drivers can rely on:
//!
//! * Layers run in project order; each layer's passes run back to back.
//! * Cut layers burn inner closed shapes before the shapes around them, so
//!   parts do not drop before their holes are cut.
//! * Raster rows keep the head at the layer speed across gaps and overscan
//!   by emitting [`MoveKind::Burn`] moves with `power_pct == Some(0.0)`,
//!   meaning "laser off, but do not rapid". Drivers must not fire on these.

use std::f64::consts::PI;

use kurbo::{Affine, Point};
use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorCode, Result};
use crate::estimate::{Estimate, EstimateOptions, estimate};
use crate::geometry::{Bounds, DEFAULT_TOLERANCE_MM, Polyline, dist, flatten};
use crate::job::{Job, Layer, LayerMode, Move, MoveKind};
use crate::profile::MachineProfile;
use crate::project::{HatchSettings, ObjectKind, Project, ProjectLayer, RasterSettings};
use crate::raster::{Dither, dither};

/// Planner settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanOptions {
    /// Curve flattening tolerance in mm.
    pub tolerance_mm: f64,
    /// Reorder cut paths to shorten travel (keeps inner-before-outer).
    pub optimize_order: bool,
    /// Time estimate settings.
    pub estimate: EstimateOptions,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            tolerance_mm: DEFAULT_TOLERANCE_MM,
            optimize_order: true,
            estimate: EstimateOptions::default(),
        }
    }
}

/// How serious a [`Warning`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth knowing; does not need acknowledging.
    Info,
    /// Must be acknowledged before the job starts (FR-18).
    Warning,
    /// The profile marks this limit as hard: the job cannot start.
    Blocking,
}

/// A problem found while planning, naming the layer, limit and value (FR-17).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Warning {
    /// Stable code, such as `LB-LIMIT-001`.
    pub code: String,
    /// How serious it is.
    pub severity: Severity,
    /// The layer it concerns, if any.
    pub layer: Option<String>,
    /// The profile limit, such as `max_power_pct`, if any.
    pub limit: Option<String>,
    /// The limit's value.
    pub limit_value: Option<f64>,
    /// The value that breaks it.
    pub value: Option<f64>,
    /// A full sentence for the user.
    pub message: String,
}

/// The planner's output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// The job, ready for a driver.
    pub job: Job,
    /// Run time estimate.
    pub estimate: Estimate,
    /// Limit checks and other notes.
    pub warnings: Vec<Warning>,
}

impl Plan {
    /// Whether any warning blocks the job.
    pub fn is_blocked(&self) -> bool {
        self.warnings
            .iter()
            .any(|w| w.severity == Severity::Blocking)
    }
}

/// Plan `project` for `profile`.
pub fn plan(project: &Project, profile: &MachineProfile, opts: &PlanOptions) -> Result<Plan> {
    let mut job = Job::default();
    let mut warnings = Vec::new();
    let mut pos = opts.estimate.start;
    for layer in project.layers.iter().filter(|l| l.output) {
        let objects: Vec<_> = project
            .objects
            .iter()
            .filter(|o| o.layer == layer.key)
            .collect();
        if objects.is_empty() {
            continue;
        }
        let index = job.layers.len();
        job.layers.push(layer.settings.clone());
        let mut moves = Vec::new();
        match layer.settings.mode {
            LayerMode::Cut => {
                let mut lines = Vec::new();
                for o in &objects {
                    if let ObjectKind::Vector { path } = &o.kind {
                        lines.extend(flatten(path, o.transform, opts.tolerance_mm));
                    } else {
                        warnings.push(note(
                            layer,
                            format!("image \"{}\" is on a cut layer and was skipped", o.name),
                        ));
                    }
                }
                cut(&mut moves, lines, index, &mut pos, opts.optimize_order);
            }
            LayerMode::Fill | LayerMode::GalvoHatch => {
                let mut rings = Vec::new();
                for o in &objects {
                    if let ObjectKind::Vector { path } = &o.kind {
                        rings.extend(
                            flatten(path, o.transform, opts.tolerance_mm)
                                .into_iter()
                                .filter(|p| p.closed),
                        );
                    } else {
                        warnings.push(note(
                            layer,
                            format!("image \"{}\" is on a fill layer and was skipped", o.name),
                        ));
                    }
                }
                if rings.is_empty() {
                    warnings.push(note(layer, "no closed shapes to fill".into()));
                }
                hatch(&mut moves, &rings, &layer.hatch, index, &mut pos)?;
            }
            LayerMode::Image => {
                for o in &objects {
                    match &o.kind {
                        ObjectKind::Raster {
                            image,
                            width_mm,
                            height_mm,
                        } => raster(
                            &mut moves,
                            RasterInput {
                                image,
                                width_mm: *width_mm,
                                height_mm: *height_mm,
                                transform: o.transform,
                                name: &o.name,
                            },
                            &layer.raster,
                            &layer.settings,
                            index,
                            &mut pos,
                        )?,
                        ObjectKind::Vector { .. } => {
                            warnings.push(note(
                                layer,
                                format!(
                                    "vector \"{}\" is on an image layer and was skipped",
                                    o.name
                                ),
                            ));
                        }
                    }
                }
            }
        }
        let passes = layer.settings.passes.max(1) as usize;
        for _ in 0..passes {
            job.moves.extend(moves.iter().cloned());
        }
    }
    if job.moves.is_empty() {
        return Err(Error::new(
            ErrorCode::PlanFailed,
            "Nothing to run",
            "no output layer has anything to burn",
            "import a file and make sure its layer has output turned on",
        ));
    }
    let est = estimate(&job, profile, &opts.estimate);
    check_limits(&job, &est, profile, opts, &mut warnings);
    Ok(Plan {
        job,
        estimate: est,
        warnings,
    })
}

fn note(layer: &ProjectLayer, message: String) -> Warning {
    Warning {
        code: "LB-PLAN-101".into(),
        severity: Severity::Info,
        layer: Some(layer.settings.name.clone()),
        limit: None,
        limit_value: None,
        value: None,
        message: format!("Layer \"{}\": {message}.", layer.settings.name),
    }
}

// ----------------------------------------------------------------- cutting

/// Whether `p` is inside `ring` (even-odd).
fn inside(p: (f64, f64), ring: &[(f64, f64)]) -> bool {
    let mut c = false;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + n - 1) % n]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

fn cut(
    out: &mut Vec<Move>,
    lines: Vec<Polyline>,
    layer: usize,
    pos: &mut (f64, f64),
    optimize: bool,
) {
    // Nesting depth: how many closed shapes contain this one.
    let depth: Vec<usize> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let probe = l.points[0];
            lines
                .iter()
                .enumerate()
                .filter(|(j, o)| *j != i && o.closed && inside(probe, &o.points))
                .count()
        })
        .collect();
    let max_depth = depth.iter().copied().max().unwrap_or(0);
    for d in (0..=max_depth).rev() {
        let mut group: Vec<Polyline> = lines
            .iter()
            .zip(&depth)
            .filter(|(_, dd)| **dd == d)
            .map(|(l, _)| l.clone())
            .collect();
        while !group.is_empty() {
            let (i, line) = if optimize {
                let mut best = (0, f64::INFINITY, group[0].clone());
                for (i, l) in group.iter().enumerate() {
                    for cand in candidates(l) {
                        let dd = dist(*pos, cand.points[0]);
                        if dd < best.1 {
                            best = (i, dd, cand);
                        }
                    }
                }
                (best.0, best.2)
            } else {
                (0, group[0].clone())
            };
            group.swap_remove(i);
            emit_polyline(out, &line, layer, pos);
        }
    }
}

/// Ways to draw a polyline: both directions for open ones, every start vertex for closed ones.
fn candidates(l: &Polyline) -> Vec<Polyline> {
    if !l.closed {
        return vec![l.clone(), l.reversed()];
    }
    (0..l.points.len())
        .map(|k| {
            let mut points = l.points.clone();
            points.rotate_left(k);
            Polyline {
                points,
                closed: true,
            }
        })
        .collect()
}

fn emit_polyline(out: &mut Vec<Move>, line: &Polyline, layer: usize, pos: &mut (f64, f64)) {
    let mut pts = line.trace();
    let Some(first) = pts.next() else { return };
    if dist(*pos, first) > 1e-9 {
        out.push(Move::travel(first.0, first.1, layer));
    }
    for p in pts {
        out.push(Move::burn(p.0, p.1, layer));
    }
    *pos = out.last().map_or(*pos, |m| (m.x, m.y));
}

// ----------------------------------------------------------------- hatching

fn hatch(
    out: &mut Vec<Move>,
    rings: &[Polyline],
    settings: &HatchSettings,
    layer: usize,
    pos: &mut (f64, f64),
) -> Result<()> {
    if !(settings.spacing_mm.is_finite() && settings.spacing_mm >= 0.001) {
        return Err(Error::new(
            ErrorCode::PlanFailed,
            "Hatch spacing is too small",
            format!("spacing is {} mm", settings.spacing_mm),
            "use a spacing of at least 0.001 mm; 0.05 to 0.1 mm is typical",
        ));
    }
    for set in 0..settings.sets.max(1) {
        let angle =
            (settings.angle_deg + f64::from(set) * settings.angle_increment_deg) * PI / 180.0;
        // Rotate shapes so hatch lines are horizontal, scan, rotate back.
        let to_flat = Affine::rotate(-angle);
        let back = Affine::rotate(angle);
        let flat: Vec<Vec<(f64, f64)>> = rings
            .iter()
            .map(|r| {
                r.points
                    .iter()
                    .map(|&(x, y)| xy(to_flat * Point::new(x, y)))
                    .collect()
            })
            .collect();
        let Some(b) = Bounds::of_points(flat.iter().flatten().copied()) else {
            continue;
        };
        let offset = settings.offset_mm.rem_euclid(settings.spacing_mm);
        let mut y = b.min_y + offset + settings.spacing_mm / 2.0;
        let mut forward = true;
        while y < b.max_y {
            let mut xs = Vec::new();
            for ring in &flat {
                let n = ring.len();
                for i in 0..n {
                    let (a, c) = (ring[i], ring[(i + 1) % n]);
                    if (a.1 <= y) != (c.1 <= y) {
                        xs.push(a.0 + (y - a.1) * (c.0 - a.0) / (c.1 - a.1));
                    }
                }
            }
            xs.sort_by(f64::total_cmp);
            let mut spans: Vec<(f64, f64)> = xs.chunks_exact(2).map(|c| (c[0], c[1])).collect();
            if !forward {
                spans.reverse();
            }
            for (x0, x1) in spans {
                let (s, e) = if forward { (x0, x1) } else { (x1, x0) };
                let s = xy(back * Point::new(s, y));
                let e = xy(back * Point::new(e, y));
                if dist(*pos, s) > 1e-9 {
                    out.push(Move::travel(s.0, s.1, layer));
                }
                out.push(Move::burn(e.0, e.1, layer));
                *pos = e;
            }
            if settings.bidirectional {
                forward = !forward;
            }
            y += settings.spacing_mm;
        }
    }
    Ok(())
}

fn xy(p: Point) -> (f64, f64) {
    (p.x, p.y)
}

// ----------------------------------------------------------------- raster

struct RasterInput<'a> {
    image: &'a crate::raster::GrayImage,
    width_mm: f64,
    height_mm: f64,
    transform: Affine,
    name: &'a str,
}

fn raster(
    out: &mut Vec<Move>,
    input: RasterInput<'_>,
    settings: &RasterSettings,
    layer: &Layer,
    index: usize,
    pos: &mut (f64, f64),
) -> Result<()> {
    let [a, b, c, d, e, f] = input.transform.as_coeffs();
    if b.abs() > 1e-9 || c.abs() > 1e-9 {
        return Err(Error::new(
            ErrorCode::PlanFailed,
            format!("Image \"{}\" is rotated or skewed", input.name),
            "v1 engraves images only along the machine's X axis",
            "rotate the image by a multiple of 180 degrees, or rotate it in an image editor first",
        ));
    }
    let interval = settings.line_interval_mm;
    if !(interval.is_finite() && interval >= 0.01) {
        return Err(Error::new(
            ErrorCode::PlanFailed,
            "Raster line interval is too small",
            format!("line_interval_mm is {interval}"),
            "use at least 0.01 mm; 0.1 mm (254 DPI) is typical",
        ));
    }
    let (w, h) = ((input.width_mm * a).abs(), (input.height_mm * d).abs());
    let left = e + (input.width_mm * a).min(0.0);
    let top = f + (input.height_mm * d).min(0.0);
    let cols = (w / interval).round().max(1.0) as u32;
    let rows = (h / interval).round().max(1.0) as u32;
    if u64::from(cols) * u64::from(rows) > 400_000_000 {
        return Err(Error::new(
            ErrorCode::PlanFailed,
            format!(
                "Image \"{}\" is too large at this line interval",
                input.name
            ),
            format!("{cols} x {rows} dots"),
            "increase the line interval or scale the image down",
        ));
    }
    let mut img = input.image.resample(cols, rows);
    if a < 0.0 || d < 0.0 {
        let mut px = img.pixels.clone();
        for y in 0..rows {
            for x in 0..cols {
                let sx = if a < 0.0 { cols - 1 - x } else { x };
                let sy = if d < 0.0 { rows - 1 - y } else { y };
                px[(y * cols + x) as usize] = img.get(sx, sy);
            }
        }
        img.pixels = px;
    }
    if settings.invert {
        img.pixels.iter_mut().for_each(|p| *p = 255 - *p);
    }
    let dots = dither(&img, settings.dither);
    let grayscale = settings.dither == Dither::Grayscale;
    let power_of = |gray: u8| {
        let t = 1.0 - f64::from(gray) / 255.0;
        // Round to 0.1 % so neighbouring dots merge into one move.
        let p = settings.min_power_pct + (layer.power_pct - settings.min_power_pct) * t;
        (p * 10.0).round() / 10.0
    };
    let mut forward = true;
    for r in 0..rows {
        let y = top + (f64::from(r) + 0.5) * interval;
        // Runs of equal output: (start col, end col exclusive, power or None for off).
        let mut runs: Vec<(u32, u32, Option<f64>)> = Vec::new();
        for x in 0..cols {
            let v = dots.get(x, r).then(|| {
                if grayscale {
                    power_of(img.get(x, r))
                } else {
                    layer.power_pct
                }
            });
            match runs.last_mut() {
                Some(last) if last.2 == v => last.1 = x + 1,
                _ => runs.push((x, x + 1, v)),
            }
        }
        // Trim off-runs at both ends; skip empty rows.
        while runs.last().is_some_and(|r| r.2.is_none()) {
            runs.pop();
        }
        let lead = runs.iter().take_while(|r| r.2.is_none()).count();
        runs.drain(..lead);
        if runs.is_empty() {
            continue;
        }
        let x_at = |col: u32| left + f64::from(col) * interval;
        let (first, last) = (x_at(runs[0].0), x_at(runs[runs.len() - 1].1));
        let dir = if forward { 1.0 } else { -1.0 };
        let (start, end) = if forward {
            (first, last)
        } else {
            (last, first)
        };
        let over = settings.overscan_mm.max(0.0);
        let lead_in = start - dir * over;
        if dist(*pos, (lead_in, y)) > 1e-9 {
            out.push(Move::travel(lead_in, y, index));
        }
        if over > 0.0 {
            out.push(Move::burn(start, y, index).with_power(0.0));
        }
        let ordered: Vec<_> = if forward {
            runs
        } else {
            runs.into_iter().rev().collect()
        };
        for (c0, c1, v) in ordered {
            let x = if forward { x_at(c1) } else { x_at(c0) };
            let m = Move::burn(x, y, index);
            out.push(match v {
                None => m.with_power(0.0),
                Some(p) if grayscale => m.with_power(p),
                Some(_) => m,
            });
        }
        if over > 0.0 {
            out.push(Move::burn(end + dir * over, y, index).with_power(0.0));
        }
        *pos = out.last().map_or(*pos, |m| (m.x, m.y));
        if settings.bidirectional {
            forward = !forward;
        }
    }
    Ok(())
}

// ----------------------------------------------------------------- checks

fn check_limits(
    job: &Job,
    est: &Estimate,
    profile: &MachineProfile,
    opts: &PlanOptions,
    out: &mut Vec<Warning>,
) {
    let limits = &profile.limits;
    let severity = |name: &str| {
        if limits.hard.iter().any(|h| h == name) {
            Severity::Blocking
        } else {
            Severity::Warning
        }
    };
    let mut push = |code: &str,
                    layer: Option<&str>,
                    limit: &str,
                    limit_value: f64,
                    value: f64,
                    message: String| {
        out.push(Warning {
            code: code.into(),
            severity: severity(limit),
            layer: layer.map(str::to_owned),
            limit: Some(limit.into()),
            limit_value: Some(limit_value),
            value: Some(value),
            message,
        });
    };
    for l in &job.layers {
        if l.power_pct > limits.max_power_pct {
            push(
                "LB-LIMIT-001",
                Some(&l.name),
                "max_power_pct",
                limits.max_power_pct,
                l.power_pct,
                format!(
                    "Layer \"{}\" asks for {}% power but the profile allows {}%; it will run at {}%.",
                    l.name, l.power_pct, limits.max_power_pct, limits.max_power_pct
                ),
            );
        }
        if l.speed_mm_s > limits.max_speed_mm_s {
            push(
                "LB-LIMIT-002",
                Some(&l.name),
                "max_speed_mm_s",
                limits.max_speed_mm_s,
                l.speed_mm_s,
                format!(
                    "Layer \"{}\" asks for {} mm/s but the profile allows {} mm/s; it will run at {} mm/s.",
                    l.name, l.speed_mm_s, limits.max_speed_mm_s, limits.max_speed_mm_s
                ),
            );
        }
    }
    let times = crate::estimate::move_times(job, profile, &opts.estimate);
    if let Some(max_on) = limits.max_continuous_on_s {
        // Longest stretch of consecutive firing moves, per layer it starts on.
        let mut run = 0.0;
        let mut worst: Option<(f64, usize)> = None;
        for (m, t) in job.moves.iter().zip(&times) {
            if m.kind == MoveKind::Burn && m.power_pct != Some(0.0) {
                run += t * opts.estimate.time_scale.max(0.0);
                if worst.is_none_or(|(w, _)| run > w) {
                    worst = Some((run, m.layer));
                }
            } else {
                run = 0.0;
            }
        }
        if let Some((secs, layer)) = worst.filter(|(s, _)| *s > max_on) {
            let name = job.layers.get(layer).map(|l| l.name.as_str());
            push(
                "LB-LIMIT-003",
                name,
                "max_continuous_on_s",
                max_on,
                secs,
                format!(
                    "Layer \"{}\" keeps the laser on for {:.1} s without a break; the profile allows {} s.",
                    name.unwrap_or("?"),
                    secs,
                    max_on
                ),
            );
        }
    }
    if let Some(duty) = limits.duty_cycle_pct {
        let on: f64 = est.layers.iter().map(|l| l.burn_seconds).sum();
        if est.seconds > 0.0 {
            let pct = on / est.seconds * 100.0;
            if pct > duty {
                push(
                    "LB-LIMIT-004",
                    None,
                    "duty_cycle_pct",
                    duty,
                    pct,
                    format!(
                        "The laser is on {pct:.0}% of the job time; the profile's duty cycle limit is {duty}%."
                    ),
                );
            }
        }
    }
    if let Some((x0, y0, x1, y1)) = job.bounds() {
        let (w, h) = (profile.work_area.width_mm, profile.work_area.height_mm);
        if x0 < -1e-6 || y0 < -1e-6 || x1 > w + 1e-6 || y1 > h + 1e-6 {
            out.push(Warning {
                code: "LB-LIMIT-005".into(),
                severity: severity("work_area"),
                layer: None,
                limit: Some("work_area".into()),
                limit_value: None,
                value: None,
                message: format!(
                    "The job spans ({x0:.1}, {y0:.1}) to ({x1:.1}, {y1:.1}) mm, outside the {w} x {h} mm work area."
                ),
            });
        }
    }
    for (i, l) in job.layers.iter().enumerate() {
        if l.power_pct <= 0.0
            && job
                .moves
                .iter()
                .any(|m| m.layer == i && m.kind == MoveKind::Burn)
        {
            out.push(Warning {
                code: "LB-PLAN-102".into(),
                severity: Severity::Info,
                layer: Some(l.name.clone()),
                limit: None,
                limit_value: None,
                value: Some(l.power_pct),
                message: format!(
                    "Layer \"{}\" has 0% power and will not mark anything.",
                    l.name
                ),
            });
        }
    }
}

/// The outline to trace when framing (FR-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameShape {
    /// The bounding box of everything that burns.
    #[default]
    BoundingBox,
    /// The convex hull of everything that burns: hugs round or angled parts.
    Hull,
}

/// The closed outline of the burning parts of `job`, for a [`crate::FrameRequest`].
pub fn frame_outline(job: &Job, shape: FrameShape) -> Vec<(f64, f64)> {
    let mut pts = Vec::new();
    let mut prev: Option<(f64, f64)> = None;
    for m in &job.moves {
        if m.kind == MoveKind::Burn && m.power_pct != Some(0.0) {
            if let Some(p) = prev {
                pts.push(p);
            }
            pts.push((m.x, m.y));
        }
        prev = Some((m.x, m.y));
    }
    match shape {
        FrameShape::BoundingBox => Bounds::of_points(pts)
            .map(|b| b.outline())
            .unwrap_or_default(),
        FrameShape::Hull => hull(pts),
    }
}

/// Convex hull, clockwise in Y-down space (Andrew's monotone chain).
fn hull(mut pts: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut lower: Vec<(f64, f64)> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<(f64, f64)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Object;
    use crate::raster::GrayImage;
    use kurbo::{BezPath, Rect, Shape};

    fn profile(extra: &str) -> MachineProfile {
        MachineProfile::from_toml(&format!(
            "[machine]\nname='t'\ndriver='mock'\nlaser='co2'\n\
             [work_area]\nwidth_mm=300\nheight_mm=200\n\
             [limits]\nmax_power_pct=60\nmax_speed_mm_s=400\n{extra}\
             [framing]\ndefault_source='laser_off'\n"
        ))
        .unwrap()
    }

    fn layer(key: &str, mode: LayerMode, power: f64) -> ProjectLayer {
        let mut l = ProjectLayer::new(key, key, "#000000", mode);
        l.settings.power_pct = power;
        l.settings.speed_mm_s = 20.0;
        l
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> ObjectKind {
        ObjectKind::Vector {
            path: Rect::new(x0, y0, x1, y1).to_path(0.01),
        }
    }

    fn project(objs: Vec<(ProjectLayer, ObjectKind)>) -> Project {
        let mut p = Project::new();
        for (i, (l, k)) in objs.into_iter().enumerate() {
            p.add_object(format!("o{i}"), &l, Affine::IDENTITY, k);
        }
        p
    }

    fn burned_len(job: &Job) -> f64 {
        let mut prev = (0.0, 0.0);
        let mut total = 0.0;
        for m in &job.moves {
            if m.kind == MoveKind::Burn && m.power_pct != Some(0.0) {
                total += dist(prev, (m.x, m.y));
            }
            prev = (m.x, m.y);
        }
        total
    }

    #[test]
    fn holes_are_cut_before_the_outline() {
        let cut = layer("c", LayerMode::Cut, 50.0);
        let p = project(vec![
            (cut.clone(), rect(0.0, 0.0, 100.0, 100.0)),
            (cut, rect(40.0, 40.0, 60.0, 60.0)),
        ]);
        let plan = plan(&p, &profile(""), &PlanOptions::default()).unwrap();
        let first_burn = plan
            .job
            .moves
            .iter()
            .find(|m| m.kind == MoveKind::Burn)
            .unwrap();
        assert!(first_burn.x >= 40.0 && first_burn.x <= 60.0, "hole first");
        assert!((burned_len(&plan.job) - 480.0).abs() < 1e-6);
    }

    #[test]
    fn passes_repeat_the_layer() {
        let mut cut = layer("c", LayerMode::Cut, 50.0);
        cut.settings.passes = 3;
        let p = project(vec![(cut, rect(0.0, 0.0, 10.0, 10.0))]);
        let plan = plan(&p, &profile(""), &PlanOptions::default()).unwrap();
        assert!((burned_len(&plan.job) - 120.0).abs() < 1e-6);
    }

    #[test]
    fn fill_covers_area_and_leaves_the_hole() {
        let mut fill = layer("f", LayerMode::Fill, 30.0);
        fill.hatch.spacing_mm = 1.0;
        // A 10x10 square with a 4x4 hole: area 84, at 1 mm spacing ≈ 84 mm of lines.
        let mut path = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01);
        path.extend(Rect::new(3.0, 3.0, 7.0, 7.0).to_path(0.01));
        let p = project(vec![(fill, ObjectKind::Vector { path })]);
        let plan = plan(&p, &profile(""), &PlanOptions::default()).unwrap();
        assert!(
            (burned_len(&plan.job) - 84.0).abs() < 1e-6,
            "{}",
            burned_len(&plan.job)
        );
        assert!(
            plan.job
                .moves
                .iter()
                .all(|m| !(m.kind == MoveKind::Burn && m.x > 3.5 && m.x < 6.5))
        );
    }

    #[test]
    fn galvo_hatch_sets_rotate() {
        let mut g = layer("g", LayerMode::GalvoHatch, 30.0);
        g.hatch.spacing_mm = 1.0;
        g.hatch.sets = 2;
        let p = project(vec![(g, rect(0.0, 0.0, 10.0, 10.0))]);
        let plan = plan(&p, &profile(""), &PlanOptions::default()).unwrap();
        assert!(
            (burned_len(&plan.job) - 200.0).abs() < 1e-6,
            "{}",
            burned_len(&plan.job)
        );
    }

    #[test]
    fn raster_burns_dark_pixels_with_overscan_at_zero_power() {
        let mut img = layer("image", LayerMode::Image, 40.0);
        img.raster.line_interval_mm = 1.0;
        img.raster.overscan_mm = 2.0;
        img.raster.dither = Dither::Threshold { level: 128 };
        // 4x2 px at 1 px/mm: row 0 = black, white, white, black; row 1 all white.
        let image = GrayImage::new(4, 2, vec![0, 255, 255, 0, 255, 255, 255, 255]).unwrap();
        let mut p = Project::new();
        p.add_object(
            "img",
            &img,
            Affine::translate((10.0, 20.0)),
            ObjectKind::Raster {
                image,
                width_mm: 4.0,
                height_mm: 2.0,
            },
        );
        let plan = plan(&p, &profile(""), &PlanOptions::default()).unwrap();
        let m = &plan.job.moves;
        assert_eq!(m[0], Move::travel(8.0, 20.5, 0));
        assert_eq!(m[1], Move::burn(10.0, 20.5, 0).with_power(0.0));
        assert_eq!(m[2], Move::burn(11.0, 20.5, 0));
        assert_eq!(m[3], Move::burn(13.0, 20.5, 0).with_power(0.0));
        assert_eq!(m[4], Move::burn(14.0, 20.5, 0));
        assert_eq!(m[5], Move::burn(16.0, 20.5, 0).with_power(0.0));
        assert_eq!(m.len(), 6, "empty row skipped");
        assert!((burned_len(&plan.job) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn grayscale_modulates_power() {
        let mut img = layer("image", LayerMode::Image, 50.0);
        img.raster.line_interval_mm = 1.0;
        img.raster.overscan_mm = 0.0;
        img.raster.dither = Dither::Grayscale;
        let image = GrayImage::new(2, 1, vec![0, 128]).unwrap();
        let mut p = Project::new();
        p.add_object(
            "i",
            &img,
            Affine::IDENTITY,
            ObjectKind::Raster {
                image,
                width_mm: 2.0,
                height_mm: 1.0,
            },
        );
        let plan = plan(&p, &profile(""), &PlanOptions::default()).unwrap();
        let powers: Vec<_> = plan.job.moves.iter().filter_map(|m| m.power_pct).collect();
        assert_eq!(powers, vec![50.0, 24.9]);
    }

    #[test]
    fn rotated_image_is_rejected() {
        let img = layer("image", LayerMode::Image, 50.0);
        let mut p = Project::new();
        p.add_object(
            "i",
            &img,
            Affine::rotate(0.3),
            ObjectKind::Raster {
                image: GrayImage::new(1, 1, vec![0]).unwrap(),
                width_mm: 1.0,
                height_mm: 1.0,
            },
        );
        assert_eq!(
            plan(&p, &profile(""), &PlanOptions::default())
                .unwrap_err()
                .code,
            ErrorCode::PlanFailed
        );
    }

    #[test]
    fn limit_warnings_name_layer_limit_and_value() {
        let mut cut = layer("hot", LayerMode::Cut, 80.0);
        cut.settings.speed_mm_s = 1.0;
        let p = project(vec![(cut, rect(0.0, 0.0, 100.0, 100.0))]);
        let prof = profile("max_continuous_on_s = 60\nhard = [\"max_continuous_on_s\"]\n");
        let plan = plan(&p, &prof, &PlanOptions::default()).unwrap();
        let power = plan
            .warnings
            .iter()
            .find(|w| w.code == "LB-LIMIT-001")
            .unwrap();
        assert_eq!(power.layer.as_deref(), Some("hot"));
        assert_eq!((power.limit_value, power.value), (Some(60.0), Some(80.0)));
        assert_eq!(power.severity, Severity::Warning);
        let on = plan
            .warnings
            .iter()
            .find(|w| w.code == "LB-LIMIT-003")
            .unwrap();
        assert_eq!(on.severity, Severity::Blocking);
        assert!(plan.is_blocked());
    }

    #[test]
    fn out_of_bounds_and_zero_power_are_reported() {
        let p = project(vec![(
            layer("c", LayerMode::Cut, 0.0),
            rect(250.0, 0.0, 350.0, 10.0),
        )]);
        let plan = plan(&p, &profile(""), &PlanOptions::default()).unwrap();
        assert!(plan.warnings.iter().any(|w| w.code == "LB-LIMIT-005"));
        assert!(plan.warnings.iter().any(|w| w.code == "LB-PLAN-102"));
    }

    #[test]
    fn disabled_layers_and_empty_projects() {
        let mut l = layer("c", LayerMode::Cut, 10.0);
        l.output = false;
        let p = project(vec![(l, rect(0.0, 0.0, 1.0, 1.0))]);
        assert_eq!(
            plan(&p, &profile(""), &PlanOptions::default())
                .unwrap_err()
                .code,
            ErrorCode::PlanFailed
        );
    }

    #[test]
    fn frame_outline_shapes() {
        let mut path = BezPath::new();
        path.move_to((10.0, 0.0));
        path.line_to((20.0, 10.0));
        path.line_to((0.0, 10.0));
        path.close_path();
        let p = project(vec![(
            layer("c", LayerMode::Cut, 10.0),
            ObjectKind::Vector { path },
        )]);
        let plan = plan(&p, &profile(""), &PlanOptions::default()).unwrap();
        let bb = frame_outline(&plan.job, FrameShape::BoundingBox);
        assert_eq!(bb, vec![(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)]);
        let h = frame_outline(&plan.job, FrameShape::Hull);
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn object_layers_with_wrong_kind_are_noted() {
        let mut p = project(vec![(
            layer("c", LayerMode::Cut, 10.0),
            rect(0.0, 0.0, 1.0, 1.0),
        )]);
        p.objects.push(Object {
            id: 99,
            name: "pic".into(),
            layer: "c".into(),
            transform: Affine::IDENTITY,
            kind: ObjectKind::Raster {
                image: GrayImage::new(1, 1, vec![0]).unwrap(),
                width_mm: 1.0,
                height_mm: 1.0,
            },
        });
        let plan = plan(&p, &profile(""), &PlanOptions::default()).unwrap();
        assert!(plan.warnings.iter().any(|w| w.message.contains("pic")));
    }
}
