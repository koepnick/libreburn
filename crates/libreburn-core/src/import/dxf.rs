// SPDX-License-Identifier: AGPL-3.0-or-later
//! DXF import (FR-1).
//!
//! Supports LINE, LWPOLYLINE and POLYLINE (with bulges), CIRCLE, ARC,
//! ELLIPSE, SPLINE and block INSERTs. Units come from `$INSUNITS`. DXF is
//! Y-up, so the drawing is flipped into job space (Y down) and its
//! bounding box is moved to the origin. Objects keep their DXF layer.

use std::collections::{BTreeMap, HashMap};

use ::dxf::entities::{Entity, EntityType};
use ::dxf::enums::Units;
use ::dxf::{Color, Drawing};
use kurbo::{Affine, BezPath, Point, SvgArc, Vec2};

use super::{ImportOptions, Imported, ImportedKind, ImportedObject, failed, hex};
use crate::geometry::{Bounds, flatten};

const MAX_INSERT_DEPTH: usize = 8;

fn unit_mm(units: Units, unitless: f64) -> f64 {
    match units {
        Units::Unitless => unitless,
        Units::Inches => 25.4,
        Units::Feet => 304.8,
        Units::Millimeters => 1.0,
        Units::Centimeters => 10.0,
        Units::Meters => 1000.0,
        Units::Microinches => 25.4e-6,
        Units::Mils => 0.0254,
        Units::Yards => 914.4,
        Units::Microns => 0.001,
        Units::Decimeters => 100.0,
        Units::USSurveyFeet => 1200.0 / 3937.0 * 1000.0,
        Units::USSurveyInch => 1000.0 / 39.37,
        Units::Nanometers => 1e-6,
        _ => unitless,
    }
}

pub(super) fn import(bytes: &[u8], name: &str, options: &ImportOptions) -> crate::Result<Imported> {
    let drawing = Drawing::load(&mut std::io::Cursor::new(bytes)).map_err(|e| {
        failed(
            name,
            format!("the DXF could not be parsed: {e}"),
            "save the drawing as DXF R2000 or newer (ASCII) and try again",
        )
    })?;
    let mut ctx = Ctx {
        layers: drawing
            .layers()
            .map(|l| (l.name.clone(), (l.color.clone(), l.is_layer_on)))
            .collect(),
        blocks: drawing.blocks().map(|b| (b.name.clone(), b)).collect(),
        out: Vec::new(),
        skipped: BTreeMap::new(),
        hidden: 0,
    };
    for e in drawing.entities() {
        if !e.common.is_in_paper_space {
            ctx.entity(e, Affine::IDENTITY, None, None, 0);
        }
    }

    let scale = unit_mm(
        drawing.header.default_drawing_units,
        options.dxf_unitless_mm,
    );
    // Flip Y and move the drawing's bounds to the origin.
    let flip = Affine::new([scale, 0.0, 0.0, -scale, 0.0, 0.0]);
    let bounds = ctx
        .out
        .iter()
        .flat_map(|(_, _, p)| flatten(p, flip, 0.01))
        .flat_map(|p| p.points)
        .collect::<Vec<_>>();
    let to_origin = match Bounds::of_points(bounds) {
        Some(b) => Affine::translate((-b.min_x, -b.min_y)) * flip,
        None => flip,
    };

    let mut imported = Imported::default();
    for (i, (layer, color, path)) in ctx.out.into_iter().enumerate() {
        imported.objects.push(ImportedObject {
            name: format!("{layer} {}", i + 1),
            layer_key: layer.clone(),
            layer_name: layer,
            color,
            transform: Affine::IDENTITY,
            kind: ImportedKind::Vector(to_origin * path),
        });
    }
    for (kind, n) in ctx.skipped {
        imported.warnings.push(format!(
            "skipped {n} {kind} entit{}",
            if n == 1 { "y" } else { "ies" }
        ));
    }
    if ctx.hidden > 0 {
        imported.warnings.push(format!(
            "skipped {} entities on layers that are off",
            ctx.hidden
        ));
    }
    if matches!(drawing.header.default_drawing_units, Units::Unitless) {
        imported.warnings.push(format!(
            "the DXF does not state its units; assumed 1 unit = {} mm",
            options.dxf_unitless_mm
        ));
    }
    Ok(imported)
}

struct Ctx<'a> {
    layers: HashMap<String, (Color, bool)>,
    blocks: HashMap<String, &'a ::dxf::Block>,
    out: Vec<(String, String, BezPath)>,
    skipped: BTreeMap<&'static str, usize>,
    hidden: usize,
}

impl<'a> Ctx<'a> {
    fn entity(
        &mut self,
        e: &'a Entity,
        transform: Affine,
        parent_layer: Option<&str>,
        parent_color: Option<&str>,
        depth: usize,
    ) {
        if !e.common.is_visible {
            return;
        }
        // Entities on layer "0" inside a block take the INSERT's layer.
        let layer = match parent_layer {
            Some(p) if e.common.layer == "0" => p.to_owned(),
            _ => e.common.layer.clone(),
        };
        let (layer_color, on) = self
            .layers
            .get(&layer)
            .cloned()
            .unwrap_or((Color::from_index(7), true));
        if !on {
            self.hidden += 1;
            return;
        }
        let color = if e.common.color_24_bit != 0 {
            let c = e.common.color_24_bit;
            hex((c >> 16) as u8, (c >> 8) as u8, c as u8)
        } else if e.common.color.is_by_block() {
            parent_color.map_or_else(|| aci(&layer_color), str::to_owned)
        } else if e.common.color.is_by_layer() {
            aci(&layer_color)
        } else {
            aci(&e.common.color)
        };

        let mut path = BezPath::new();
        match &e.specific {
            EntityType::Line(l) => {
                path.move_to((l.p1.x, l.p1.y));
                path.line_to((l.p2.x, l.p2.y));
            }
            EntityType::LwPolyline(p) => {
                let pts: Vec<_> = p.vertices.iter().map(|v| ((v.x, v.y), v.bulge)).collect();
                polyline(&mut path, &pts, p.is_closed());
                path = ocs(path, e_normal_z(&e.specific));
            }
            EntityType::Polyline(p) => {
                let pts: Vec<_> = p
                    .vertices()
                    .map(|v| ((v.location.x, v.location.y), v.bulge))
                    .collect();
                polyline(&mut path, &pts, p.is_closed());
                path = ocs(path, p.normal.z);
            }
            EntityType::Circle(c) => {
                let circle = kurbo::Circle::new((c.center.x, c.center.y), c.radius);
                path = ocs(kurbo::Shape::to_path(&circle, 1e-4), c.normal.z);
            }
            EntityType::Arc(a) => {
                let start = a.start_angle.to_radians();
                let mut sweep = a.end_angle.to_radians() - start;
                if sweep <= 0.0 {
                    sweep += std::f64::consts::TAU;
                }
                let arc = kurbo::Arc::new(
                    (a.center.x, a.center.y),
                    (a.radius, a.radius),
                    start,
                    sweep,
                    0.0,
                );
                path.move_to(arc.center + Vec2::from_angle(start) * a.radius);
                path.extend(arc.append_iter(1e-4));
                path = ocs(path, a.normal.z);
            }
            EntityType::Ellipse(el) => {
                let major = Vec2::new(el.major_axis.x, el.major_axis.y);
                let a = major.hypot();
                let mut sweep = el.end_parameter - el.start_parameter;
                if sweep <= 0.0 {
                    sweep += std::f64::consts::TAU;
                }
                let arc = kurbo::Arc::new(
                    (el.center.x, el.center.y),
                    (a, a * el.minor_axis_ratio),
                    el.start_parameter,
                    sweep,
                    major.atan2(),
                );
                let (sin, cos) = el.start_parameter.sin_cos();
                let local = Vec2::new(arc.radii.x * cos, arc.radii.y * sin);
                path.move_to(
                    arc.center + (Affine::rotate(arc.x_rotation) * local.to_point()).to_vec2(),
                );
                path.extend(arc.append_iter(1e-4));
                path = ocs(path, el.normal.z);
            }
            EntityType::Spline(s) => spline(&mut path, s),
            EntityType::Insert(ins) => {
                if depth >= MAX_INSERT_DEPTH {
                    *self.skipped.entry("deeply nested INSERT").or_default() += 1;
                    return;
                }
                let Some(block) = self.blocks.get(&ins.name).copied() else {
                    *self.skipped.entry("INSERT of a missing block").or_default() += 1;
                    return;
                };
                if ins.row_count > 1 || ins.column_count > 1 {
                    *self
                        .skipped
                        .entry("array copies of INSERT (first copy kept)")
                        .or_default() += 1;
                }
                let t = transform
                    * Affine::translate((ins.location.x, ins.location.y))
                    * Affine::rotate(ins.rotation.to_radians())
                    * Affine::scale_non_uniform(ins.x_scale_factor, ins.y_scale_factor)
                    * Affine::translate((-block.base_point.x, -block.base_point.y));
                for child in &block.entities {
                    self.entity(child, t, Some(&layer), Some(&color), depth + 1);
                }
                return;
            }
            other => {
                *self.skipped.entry(kind_name(other)).or_default() += 1;
                return;
            }
        }
        if !path.elements().is_empty() {
            self.out.push((layer, color, transform * path));
        }
    }
}

fn e_normal_z(e: &EntityType) -> f64 {
    match e {
        EntityType::LwPolyline(p) => p.extrusion_direction.z,
        _ => 1.0,
    }
}

/// Map object coordinates to world coordinates for the common 2D case: a
/// normal of (0, 0, -1) mirrors X (the DXF arbitrary-axis rule).
fn ocs(path: BezPath, normal_z: f64) -> BezPath {
    if normal_z < 0.0 {
        Affine::scale_non_uniform(-1.0, 1.0) * path
    } else {
        path
    }
}

fn polyline(path: &mut BezPath, pts: &[((f64, f64), f64)], closed: bool) {
    let Some(&(first, _)) = pts.first() else {
        return;
    };
    path.move_to(first);
    let n = pts.len();
    let segments = if closed { n } else { n - 1 };
    for i in 0..segments {
        let (from, bulge) = pts[i];
        let (to, _) = pts[(i + 1) % n];
        if bulge.abs() < 1e-12 {
            path.line_to(to);
            continue;
        }
        let theta = 4.0 * bulge.atan();
        let chord = Point::from(from).distance(Point::from(to));
        let r = chord / (2.0 * (theta / 2.0).sin()).abs();
        let svg = SvgArc {
            from: from.into(),
            to: to.into(),
            radii: Vec2::new(r, r),
            x_rotation: 0.0,
            large_arc: theta.abs() > std::f64::consts::PI,
            // Positive bulge is counter-clockwise in DXF's Y-up space, which
            // is kurbo's positive-angle direction.
            sweep: bulge > 0.0,
        };
        match kurbo::Arc::from_svg_arc(&svg) {
            Some(arc) => path.extend(arc.append_iter(1e-4)),
            None => path.line_to(to),
        }
    }
    if closed {
        path.close_path();
    }
}

fn spline(path: &mut BezPath, s: &::dxf::entities::Spline) {
    let ctrl: Vec<(f64, f64)> = s.control_points.iter().map(|p| (p.x, p.y)).collect();
    let degree = usize::try_from(s.degree_of_curve).unwrap_or(3);
    let knots = &s.knot_values;
    let valid = degree >= 1 && ctrl.len() > degree && knots.len() == ctrl.len() + degree + 1;
    if !valid {
        // Fall back to the fit points (or control polygon) as straight lines.
        let pts: Vec<(f64, f64)> = if s.fit_points.is_empty() {
            ctrl
        } else {
            s.fit_points.iter().map(|p| (p.x, p.y)).collect()
        };
        for (i, p) in pts.iter().enumerate() {
            if i == 0 {
                path.move_to(*p)
            } else {
                path.line_to(*p)
            }
        }
        return;
    }
    let weights: Vec<f64> = if s.weight_values.len() == ctrl.len() {
        s.weight_values.clone()
    } else {
        vec![1.0; ctrl.len()]
    };
    let (t0, t1) = (knots[degree], knots[ctrl.len()]);
    let samples = ctrl.len() * 24;
    for i in 0..=samples {
        let t = t0 + (t1 - t0) * (i as f64 / samples as f64);
        let p = de_boor(t, degree, knots, &ctrl, &weights);
        if i == 0 {
            path.move_to(p)
        } else {
            path.line_to(p)
        }
    }
}

/// Evaluate a (rational) B-spline at `t` with de Boor's algorithm.
fn de_boor(t: f64, p: usize, knots: &[f64], ctrl: &[(f64, f64)], w: &[f64]) -> (f64, f64) {
    let n = ctrl.len();
    let mut k = p;
    while k < n - 1 && t >= knots[k + 1] {
        k += 1;
    }
    let mut d: Vec<(f64, f64, f64)> = (0..=p)
        .map(|j| {
            let i = j + k - p;
            (ctrl[i].0 * w[i], ctrl[i].1 * w[i], w[i])
        })
        .collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = j + k - p;
            let denom = knots[i + p + 1 - r] - knots[i];
            let a = if denom.abs() < 1e-12 {
                0.0
            } else {
                (t - knots[i]) / denom
            };
            d[j] = (
                (1.0 - a) * d[j - 1].0 + a * d[j].0,
                (1.0 - a) * d[j - 1].1 + a * d[j].1,
                (1.0 - a) * d[j - 1].2 + a * d[j].2,
            );
        }
    }
    let (x, y, wt) = d[p];
    (x / wt, y / wt)
}

fn kind_name(e: &EntityType) -> &'static str {
    match e {
        EntityType::Text(_) | EntityType::MText(_) => "TEXT/MTEXT (explode text to curves first)",
        EntityType::Solid(_) => "SOLID",
        EntityType::Image(_) => "IMAGE",
        EntityType::ModelPoint(_) => "POINT",
        EntityType::Leader(_) => "LEADER",
        EntityType::RotatedDimension(_)
        | EntityType::RadialDimension(_)
        | EntityType::DiameterDimension(_)
        | EntityType::AngularThreePointDimension(_)
        | EntityType::OrdinateDimension(_) => "DIMENSION",
        _ => "unsupported",
    }
}

/// AutoCAD Color Index to `#rrggbb`. Exact for the standard colors 1 to 9;
/// the rest of the palette is reproduced from its hue/shade layout.
fn aci(color: &Color) -> String {
    let Some(i) = color.index() else {
        return hex(0, 0, 0);
    };
    let (r, g, b) = match i {
        1 => (255, 0, 0),
        2 => (255, 255, 0),
        3 => (0, 255, 0),
        4 => (0, 255, 255),
        5 => (0, 0, 255),
        6 => (255, 0, 255),
        // 7 is white on a dark screen and black on paper; black reads better on the canvas.
        0 | 7 => (0, 0, 0),
        8 => (128, 128, 128),
        9 => (192, 192, 192),
        250..=255 => {
            let v = [51, 91, 132, 173, 214, 255][usize::from(i - 250)];
            (v, v, v)
        }
        _ => {
            let hue = f64::from((i - 10) / 10) * 15.0;
            let shade = (i - 10) % 10;
            let value = [1.0, 1.0, 0.8, 0.8, 0.6, 0.6, 0.5, 0.5, 0.3, 0.3][usize::from(shade)];
            let sat = if shade % 2 == 0 { 1.0 } else { 0.5 };
            hsv(hue, sat, value)
        }
    };
    hex(r, g, b)
}

fn hsv(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let f = |v: f64| ((v + m) * 255.0).round() as u8;
    (f(r), f(g), f(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::dxf::entities::{Circle, Line, LwPolyline};
    use ::dxf::{LwPolylineVertex, Point as DPoint};

    fn save(mut d: Drawing) -> Vec<u8> {
        let mut out = Vec::new();
        d.normalize();
        d.save(&mut out).unwrap();
        out
    }

    fn drawing(units: Units) -> Drawing {
        let mut d = Drawing::new();
        d.header.version = ::dxf::enums::AcadVersion::R2018;
        d.header.default_drawing_units = units;
        d
    }

    fn line_entity(layer: &str, a: (f64, f64), b: (f64, f64)) -> Entity {
        let mut e = Entity::new(EntityType::Line(Line::new(
            DPoint::new(a.0, a.1, 0.0),
            DPoint::new(b.0, b.1, 0.0),
        )));
        e.common.layer = layer.into();
        e
    }

    fn all_bounds(out: &Imported) -> Bounds {
        out.objects
            .iter()
            .flat_map(|o| match &o.kind {
                ImportedKind::Vector(p) => flatten(p, o.transform, 0.001),
                ImportedKind::Raster { .. } => vec![],
            })
            .flat_map(|p| p.points)
            .fold(None, |acc: Option<Bounds>, p| {
                let b = Bounds::of_points([p]).unwrap();
                Some(acc.map_or(b, |a| a.union(&b)))
            })
            .unwrap()
    }

    #[test]
    fn inch_lines_are_scaled_flipped_and_moved_to_origin() {
        let mut d = drawing(Units::Inches);
        d.add_entity(line_entity("cuts", (1.0, 1.0), (2.0, 1.0)));
        d.add_entity(line_entity("cuts", (1.0, 1.0), (1.0, 3.0)));
        let out = import(&save(d), "a.dxf", &ImportOptions::default()).unwrap();
        assert_eq!(out.objects.len(), 2);
        assert_eq!(out.objects[0].layer_key, "cuts");
        let b = all_bounds(&out);
        assert!((b.min_x).abs() < 1e-6 && (b.min_y).abs() < 1e-6);
        assert!((b.width() - 25.4).abs() < 1e-6 && (b.height() - 50.8).abs() < 1e-6);
        // The vertical line went up from y=1 in DXF; flipped, its start is now at the bottom.
        let ImportedKind::Vector(p) = &out.objects[1].kind else {
            panic!()
        };
        let pts = &flatten(p, Affine::IDENTITY, 0.01)[0].points;
        assert!((pts[0].1 - 50.8).abs() < 1e-6 && pts[1].1.abs() < 1e-6);
    }

    #[test]
    fn circle_and_bulged_polyline() {
        let mut d = drawing(Units::Millimeters);
        d.add_entity(Entity::new(EntityType::Circle(Circle::new(
            DPoint::new(0.0, 0.0, 0.0),
            5.0,
        ))));
        let mut lw = LwPolyline::default();
        // A half-circle of radius 5 from (20,0) to (30,0), bulge 1 = 180 degrees.
        lw.vertices.push(LwPolylineVertex {
            x: 20.0,
            y: 0.0,
            bulge: 1.0,
            ..Default::default()
        });
        lw.vertices.push(LwPolylineVertex {
            x: 30.0,
            y: 0.0,
            ..Default::default()
        });
        d.add_entity(Entity::new(EntityType::LwPolyline(lw)));
        let out = import(&save(d), "a.dxf", &ImportOptions::default()).unwrap();
        assert_eq!(out.objects.len(), 2);
        let ImportedKind::Vector(p) = &out.objects[1].kind else {
            panic!()
        };
        let len: f64 = flatten(p, Affine::IDENTITY, 0.001)
            .iter()
            .map(|l| l.length())
            .sum();
        assert!(
            (len - std::f64::consts::PI * 5.0).abs() < 0.01,
            "arc length {len}"
        );
        let b = all_bounds(&out);
        // Circle spans y -5..5, the CCW half-circle bulges below y=0 (y -5..0).
        assert!((b.height() - 10.0).abs() < 0.01);
        assert!((b.width() - 35.0).abs() < 0.01);
    }

    #[test]
    fn unitless_warns_and_text_is_reported() {
        let mut d = drawing(Units::Unitless);
        d.add_entity(line_entity("0", (0.0, 0.0), (1.0, 0.0)));
        d.add_entity(Entity::new(EntityType::Text(Default::default())));
        let out = import(&save(d), "a.dxf", &ImportOptions::default()).unwrap();
        assert!(out.warnings.iter().any(|w| w.contains("units")));
        assert!(out.warnings.iter().any(|w| w.contains("TEXT")));
    }

    #[test]
    fn spline_passes_through_its_clamped_ends() {
        let s = ::dxf::entities::Spline {
            degree_of_curve: 2,
            knot_values: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            control_points: vec![
                DPoint::new(0.0, 0.0, 0.0),
                DPoint::new(5.0, 10.0, 0.0),
                DPoint::new(10.0, 0.0, 0.0),
            ],
            ..Default::default()
        };
        let mut p = BezPath::new();
        spline(&mut p, &s);
        let pts = &flatten(&p, Affine::IDENTITY, 0.01)[0].points;
        assert_eq!(pts[0], (0.0, 0.0));
        let last = pts[pts.len() - 1];
        assert!((last.0 - 10.0).abs() < 1e-9 && last.1.abs() < 1e-9);
        // Quadratic Bezier peak is half the control height.
        let peak = pts.iter().map(|p| p.1).fold(0.0, f64::max);
        assert!((peak - 5.0).abs() < 0.05);
    }

    #[test]
    fn aci_standard_colors() {
        assert_eq!(aci(&Color::from_index(1)), "#ff0000");
        assert_eq!(aci(&Color::from_index(5)), "#0000ff");
        assert_eq!(aci(&Color::from_index(10)), "#ff0000");
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(import(b"0\nSECTION\n2\nBOGUS", "a.dxf", &ImportOptions::default()).is_err());
    }
}
