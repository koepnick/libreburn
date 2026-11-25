// SPDX-License-Identifier: AGPL-3.0-or-later
//! SVG import (FR-1), via usvg.
//!
//! usvg resolves CSS, `use` references, units and transforms. Each visible
//! path becomes one object on the layer named after its stroke color (or its
//! fill color when it has no stroke), which is how laser users mark
//! operations by color.

use kurbo::{Affine, BezPath};
use usvg::tiny_skia_path::PathSegment;

use super::{ImportOptions, Imported, ImportedKind, ImportedObject, failed, hex};

pub(super) fn import(bytes: &[u8], name: &str, options: &ImportOptions) -> crate::Result<Imported> {
    if !(options.svg_dpi.is_finite() && options.svg_dpi > 0.0) {
        return Err(failed(
            name,
            format!("SVG DPI is {}", options.svg_dpi),
            "use 96, the CSS default",
        ));
    }
    let opt = usvg::Options {
        dpi: options.svg_dpi as f32,
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(bytes, &opt).map_err(|e| {
        failed(
            name,
            format!("the SVG could not be parsed: {e}"),
            "open and re-save the file in Inkscape as plain SVG",
        )
    })?;
    let mm_per_px = 25.4 / options.svg_dpi;
    let mut out = Imported::default();
    let mut state = State {
        scale: Affine::scale(mm_per_px),
        count: 0,
        skipped_images: 0,
        skipped_text: 0,
        non_solid: 0,
    };
    walk(tree.root(), &mut state, &mut out);
    if state.skipped_images > 0 {
        out.warnings.push(format!(
            "skipped {} embedded image(s); import images as separate PNG or JPEG files",
            state.skipped_images
        ));
    }
    if state.skipped_text > 0 {
        out.warnings.push(format!(
            "skipped {} text element(s); convert text to paths before exporting",
            state.skipped_text
        ));
    }
    if state.non_solid > 0 {
        out.warnings.push(format!(
            "{} path(s) use gradients or patterns and were put on the black layer",
            state.non_solid
        ));
    }
    Ok(out)
}

struct State {
    scale: Affine,
    count: usize,
    skipped_images: usize,
    skipped_text: usize,
    non_solid: usize,
}

fn walk(group: &usvg::Group, state: &mut State, out: &mut Imported) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk(g, state, out),
            usvg::Node::Image(_) => state.skipped_images += 1,
            usvg::Node::Text(_) => state.skipped_text += 1,
            usvg::Node::Path(p) => {
                if !p.is_visible() {
                    continue;
                }
                let paint = p
                    .stroke()
                    .map(|s| s.paint())
                    .or_else(|| p.fill().map(|f| f.paint()));
                let color = match paint {
                    Some(usvg::Paint::Color(c)) => hex(c.red, c.green, c.blue),
                    Some(_) => {
                        state.non_solid += 1;
                        hex(0, 0, 0)
                    }
                    // Neither stroke nor fill: invisible in a viewer, skip it.
                    None => continue,
                };
                let path = convert(p.data(), state.scale * to_affine(p.abs_transform()));
                if path.elements().is_empty() {
                    continue;
                }
                state.count += 1;
                let name = if p.id().is_empty() {
                    format!("path {}", state.count)
                } else {
                    p.id().to_owned()
                };
                out.objects.push(ImportedObject {
                    name,
                    layer_key: color.clone(),
                    layer_name: color.clone(),
                    color,
                    transform: Affine::IDENTITY,
                    kind: ImportedKind::Vector(path),
                });
            }
        }
    }
}

fn to_affine(t: usvg::Transform) -> Affine {
    Affine::new([
        f64::from(t.sx),
        f64::from(t.ky),
        f64::from(t.kx),
        f64::from(t.sy),
        f64::from(t.tx),
        f64::from(t.ty),
    ])
}

fn convert(data: &usvg::tiny_skia_path::Path, transform: Affine) -> BezPath {
    let p = |pt: usvg::tiny_skia_path::Point| {
        transform * kurbo::Point::new(f64::from(pt.x), f64::from(pt.y))
    };
    let mut path = BezPath::new();
    for seg in data.segments() {
        match seg {
            PathSegment::MoveTo(a) => path.move_to(p(a)),
            PathSegment::LineTo(a) => path.line_to(p(a)),
            PathSegment::QuadTo(a, b) => path.quad_to(p(a), p(b)),
            PathSegment::CubicTo(a, b, c) => path.curve_to(p(a), p(b), p(c)),
            PathSegment::Close => path.close_path(),
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Bounds, flatten};

    fn bounds(obj: &ImportedObject) -> Bounds {
        let ImportedKind::Vector(path) = &obj.kind else {
            panic!()
        };
        Bounds::of_points(
            flatten(path, obj.transform, 0.001)
                .into_iter()
                .flat_map(|p| p.points),
        )
        .unwrap()
    }

    #[test]
    fn mm_document_keeps_real_size_and_colors() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50">
            <rect id="box" x="10" y="5" width="20" height="10" fill="none" stroke="#ff0000"/>
            <circle cx="50" cy="25" r="5" fill="#0000ff"/>
        </svg>"##;
        let out = import(svg, "t.svg", &ImportOptions::default()).unwrap();
        assert_eq!(out.objects.len(), 2);
        assert_eq!(out.objects[0].name, "box");
        assert_eq!(out.objects[0].layer_key, "#ff0000");
        assert_eq!(out.objects[1].layer_key, "#0000ff");
        let b = bounds(&out.objects[0]);
        assert!((b.min_x - 10.0).abs() < 1e-3 && (b.min_y - 5.0).abs() < 1e-3);
        assert!((b.width() - 20.0).abs() < 1e-3 && (b.height() - 10.0).abs() < 1e-3);
        let c = bounds(&out.objects[1]);
        assert!((c.width() - 10.0).abs() < 1e-2);
    }

    #[test]
    fn px_document_uses_96_dpi() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96">
            <path d="M0 0 H96" stroke="black"/></svg>"#;
        let out = import(svg, "t.svg", &ImportOptions::default()).unwrap();
        assert!((bounds(&out.objects[0]).width() - 25.4).abs() < 1e-3);
    }

    #[test]
    fn group_transforms_are_applied() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
            <g transform="translate(30 40) scale(2)"><path d="M0 0 H5" stroke="black"/></g></svg>"#;
        let out = import(svg, "t.svg", &ImportOptions::default()).unwrap();
        let b = bounds(&out.objects[0]);
        assert!((b.min_x - 30.0).abs() < 1e-3 && (b.min_y - 40.0).abs() < 1e-3);
        assert!((b.width() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn hidden_and_unpainted_paths_are_skipped() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <path d="M0 0 H5" stroke="black" visibility="hidden"/>
            <path d="M0 0 H5" fill="none"/></svg>"#;
        let out = import(svg, "t.svg", &ImportOptions::default()).unwrap();
        assert!(out.objects.is_empty());
    }

    #[test]
    fn broken_svg_is_an_error() {
        let err = import(b"<svg", "t.svg", &ImportOptions::default()).unwrap_err();
        assert_eq!(err.code, crate::ErrorCode::ImportFailed);
    }
}
