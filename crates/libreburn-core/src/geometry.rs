// SPDX-License-Identifier: AGPL-3.0-or-later
//! Geometry helpers: flattening curves to polylines and bounding boxes.
//!
//! Vector objects keep their curves as [`kurbo::BezPath`] so they stay
//! editable. Planning flattens them to [`Polyline`]s at a chosen tolerance.

use kurbo::{Affine, BezPath, PathEl};
use serde::{Deserialize, Serialize};

/// Default flattening tolerance in mm. Well under a typical 0.08 mm kerf.
pub const DEFAULT_TOLERANCE_MM: f64 = 0.01;

/// A flattened subpath.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polyline {
    /// Points in mm. A closed polyline does not repeat its first point.
    pub points: Vec<(f64, f64)>,
    /// The last point connects back to the first.
    pub closed: bool,
}

impl Polyline {
    /// Points in drawing order, repeating the first point at the end when closed.
    pub fn trace(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        let close = if self.closed {
            self.points.first().copied()
        } else {
            None
        };
        self.points.iter().copied().chain(close)
    }

    /// Total drawn length in mm.
    pub fn length(&self) -> f64 {
        let pts: Vec<_> = self.trace().collect();
        pts.windows(2).map(|w| dist(w[0], w[1])).sum()
    }

    /// The same polyline drawn in the opposite direction.
    pub fn reversed(&self) -> Self {
        let mut points = self.points.clone();
        points.reverse();
        if self.closed {
            // Keep the same start point so a closed loop still begins where it did.
            points.rotate_right(1);
        }
        Self {
            points,
            closed: self.closed,
        }
    }
}

/// Euclidean distance between two points.
pub fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Flatten `path` (after applying `transform`) into polylines.
///
/// Subpaths with fewer than two distinct points are dropped.
pub fn flatten(path: &BezPath, transform: Affine, tolerance: f64) -> Vec<Polyline> {
    let mut out = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    let mut closed = false;
    let finish = |pts: &mut Vec<(f64, f64)>, closed: bool, out: &mut Vec<Polyline>| {
        pts.dedup_by(|a, b| dist(*a, *b) < 1e-9);
        if closed && pts.len() > 1 && dist(pts[0], pts[pts.len() - 1]) < 1e-9 {
            pts.pop();
        }
        if pts.len() >= 2 {
            // A closed "loop" of two points is just a line drawn twice.
            let closed = closed && pts.len() >= 3;
            out.push(Polyline {
                points: std::mem::take(pts),
                closed,
            });
        }
        pts.clear();
    };
    let transformed = transform * path.clone();
    kurbo::flatten(transformed.iter(), tolerance.max(1e-6), |el| match el {
        PathEl::MoveTo(p) => {
            finish(&mut current, closed, &mut out);
            closed = false;
            current.push((p.x, p.y));
        }
        PathEl::LineTo(p) => current.push((p.x, p.y)),
        PathEl::ClosePath => {
            closed = true;
            finish(&mut current, closed, &mut out);
            closed = false;
        }
        // flatten() only emits MoveTo, LineTo and ClosePath.
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
    });
    finish(&mut current, closed, &mut out);
    out
}

/// An axis-aligned box in mm.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    /// Left.
    pub min_x: f64,
    /// Top.
    pub min_y: f64,
    /// Right.
    pub max_x: f64,
    /// Bottom.
    pub max_y: f64,
}

impl Bounds {
    /// The box around a set of points, or `None` if there are none.
    pub fn of_points(points: impl IntoIterator<Item = (f64, f64)>) -> Option<Self> {
        points.into_iter().fold(None, |acc, (x, y)| {
            Some(match acc {
                None => Self {
                    min_x: x,
                    min_y: y,
                    max_x: x,
                    max_y: y,
                },
                Some(b) => b.union(&Self {
                    min_x: x,
                    min_y: y,
                    max_x: x,
                    max_y: y,
                }),
            })
        })
    }

    /// The smallest box holding both.
    pub fn union(&self, other: &Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    /// Width in mm.
    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    /// Height in mm.
    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    /// The four corners as a closed outline, clockwise from top-left.
    pub fn outline(&self) -> Vec<(f64, f64)> {
        vec![
            (self.min_x, self.min_y),
            (self.max_x, self.min_y),
            (self.max_x, self.max_y),
            (self.min_x, self.max_y),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Shape};

    #[test]
    fn square_flattens_to_one_closed_polyline() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        p.line_to((0.0, 10.0));
        p.close_path();
        let lines = flatten(&p, Affine::IDENTITY, 0.01);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
        assert_eq!(lines[0].points.len(), 4);
        assert!((lines[0].length() - 40.0).abs() < 1e-9);
    }

    #[test]
    fn circle_is_flattened_within_tolerance() {
        let c = Circle::new((0.0, 0.0), 10.0).to_path(1e-6);
        let lines = flatten(&c, Affine::IDENTITY, 0.01);
        assert_eq!(lines.len(), 1);
        for (x, y) in &lines[0].points {
            assert!((x.hypot(*y) - 10.0).abs() < 0.011);
        }
        assert!((lines[0].length() - std::f64::consts::TAU * 10.0).abs() < 0.05);
    }

    #[test]
    fn transform_is_applied() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((1.0, 0.0));
        let lines = flatten(&p, Affine::translate((5.0, 7.0)) * Affine::scale(2.0), 0.01);
        assert_eq!(lines[0].points, vec![(5.0, 7.0), (7.0, 7.0)]);
    }

    #[test]
    fn reversed_closed_polyline_keeps_its_start() {
        let p = Polyline {
            points: vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)],
            closed: true,
        };
        let r = p.reversed();
        assert_eq!(r.points, vec![(0.0, 0.0), (1.0, 1.0), (1.0, 0.0)]);
    }

    #[test]
    fn bounds_of_points() {
        let b = Bounds::of_points([(1.0, 2.0), (-1.0, 5.0)]).unwrap();
        assert_eq!((b.width(), b.height()), (2.0, 3.0));
        assert!(Bounds::of_points([]).is_none());
    }
}
