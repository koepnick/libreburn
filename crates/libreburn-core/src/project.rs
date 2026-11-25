// SPDX-License-Identifier: AGPL-3.0-or-later
//! The editable project: layers with settings, and the objects on them.
//!
//! A [`Project`] is what the user designs. The planner turns it into a
//! device-neutral [`crate::Job`].

use kurbo::{Affine, BezPath};
use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorCode, Result};
use crate::geometry::{Bounds, DEFAULT_TOLERANCE_MM, flatten};
use crate::import::{self, ImportOptions, ImportedKind};
use crate::job::{Layer, LayerMode};
use crate::raster::{Dither, GrayImage};

/// Hatch settings for fill and galvo hatch layers (FR-9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HatchSettings {
    /// Line angle in degrees, 0 is horizontal.
    pub angle_deg: f64,
    /// Distance between lines in mm.
    pub spacing_mm: f64,
    /// Number of hatch sets drawn over each other.
    pub sets: u32,
    /// Angle added for each following set, in degrees.
    pub angle_increment_deg: f64,
    /// Alternate line direction (faster) instead of always drawing one way.
    pub bidirectional: bool,
    /// Shift of the line grid in mm, within one spacing.
    pub offset_mm: f64,
}

impl Default for HatchSettings {
    fn default() -> Self {
        Self {
            angle_deg: 0.0,
            spacing_mm: 0.1,
            sets: 1,
            angle_increment_deg: 90.0,
            bidirectional: true,
            offset_mm: 0.0,
        }
    }
}

/// Raster settings for image layers (FR-2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RasterSettings {
    /// Distance between scan lines and between dots in mm (0.1 mm = 254 DPI).
    pub line_interval_mm: f64,
    /// How gray becomes dots.
    pub dither: Dither,
    /// Scan in both directions.
    pub bidirectional: bool,
    /// Extra travel past each line end so the head is at speed while burning, in mm.
    pub overscan_mm: f64,
    /// Power for the lightest non-white gray in grayscale mode, in percent.
    pub min_power_pct: f64,
    /// Engrave the negative.
    pub invert: bool,
}

impl Default for RasterSettings {
    fn default() -> Self {
        Self {
            line_interval_mm: 0.1,
            dither: Dither::default(),
            bidirectional: true,
            overscan_mm: 2.0,
            min_power_pct: 0.0,
            invert: false,
        }
    }
}

/// A layer in the project, with everything the planner needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectLayer {
    /// Stable key: a `#rrggbb` color for SVG, the layer name for DXF, `image` for rasters.
    pub key: String,
    /// Display color as `#rrggbb`.
    pub color: String,
    /// Include this layer in the job.
    #[serde(default = "yes")]
    pub output: bool,
    /// Mode, speed, power and source settings.
    pub settings: Layer,
    /// Used by [`LayerMode::Fill`] and [`LayerMode::GalvoHatch`].
    #[serde(default)]
    pub hatch: HatchSettings,
    /// Used by [`LayerMode::Image`].
    #[serde(default)]
    pub raster: RasterSettings,
}

fn yes() -> bool {
    true
}

impl ProjectLayer {
    /// A new layer with safe defaults: zero power until the user sets it.
    pub fn new(
        key: impl Into<String>,
        name: impl Into<String>,
        color: impl Into<String>,
        mode: LayerMode,
    ) -> Self {
        Self {
            key: key.into(),
            color: color.into(),
            output: true,
            settings: Layer {
                name: name.into(),
                mode,
                ..Default::default()
            },
            hatch: HatchSettings::default(),
            raster: RasterSettings::default(),
        }
    }
}

/// What an object is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ObjectKind {
    /// Vector paths in mm, before the object's transform.
    Vector {
        /// The curves, kept editable.
        path: BezPath,
    },
    /// A grayscale image covering `(0, 0)` to `(width_mm, height_mm)` before the transform.
    Raster {
        /// Pixels.
        image: GrayImage,
        /// Width in mm.
        width_mm: f64,
        /// Height in mm.
        height_mm: f64,
    },
}

/// One object on the canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    /// Unique within the project.
    pub id: u64,
    /// Display name, usually from the source file.
    pub name: String,
    /// Key of the [`ProjectLayer`] this object belongs to.
    pub layer: String,
    /// Placement: move, scale, rotate and mirror (FR-4).
    #[serde(default = "identity")]
    pub transform: Affine,
    /// The geometry.
    #[serde(flatten)]
    pub kind: ObjectKind,
}

fn identity() -> Affine {
    Affine::IDENTITY
}

impl Object {
    /// Bounds after the transform, in mm.
    pub fn bounds(&self) -> Option<Bounds> {
        match &self.kind {
            ObjectKind::Vector { path } => Bounds::of_points(
                flatten(path, self.transform, DEFAULT_TOLERANCE_MM * 10.0)
                    .into_iter()
                    .flat_map(|p| p.points),
            ),
            ObjectKind::Raster {
                width_mm,
                height_mm,
                ..
            } => Bounds::of_points(
                [
                    (0.0, 0.0),
                    (*width_mm, 0.0),
                    (*width_mm, *height_mm),
                    (0.0, *height_mm),
                ]
                .map(|(x, y)| {
                    let p = self.transform * kurbo::Point::new(x, y);
                    (p.x, p.y)
                }),
            ),
        }
    }
}

/// What an import added.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportReport {
    /// Ids of the new objects.
    pub object_ids: Vec<u64>,
    /// Keys of layers created by this import.
    pub new_layers: Vec<String>,
    /// Things the importer skipped or approximated.
    pub warnings: Vec<String>,
}

/// The whole design.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Layers in run order.
    pub layers: Vec<ProjectLayer>,
    /// Objects in drawing order.
    pub objects: Vec<Object>,
    #[serde(default)]
    next_id: u64,
}

impl Project {
    /// An empty project.
    pub fn new() -> Self {
        Self::default()
    }

    /// The layer with `key`.
    pub fn layer(&self, key: &str) -> Option<&ProjectLayer> {
        self.layers.iter().find(|l| l.key == key)
    }

    /// The layer with `key`, for editing.
    pub fn layer_mut(&mut self, key: &str) -> Option<&mut ProjectLayer> {
        self.layers.iter_mut().find(|l| l.key == key)
    }

    /// The object with `id`.
    pub fn object(&self, id: u64) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    /// The object with `id`, for editing.
    pub fn object_mut(&mut self, id: u64) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    /// Remove an object. Returns whether it existed.
    pub fn remove_object(&mut self, id: u64) -> bool {
        let before = self.objects.len();
        self.objects.retain(|o| o.id != id);
        before != self.objects.len()
    }

    /// Bounds of every object on an output layer.
    pub fn bounds(&self) -> Option<Bounds> {
        self.objects
            .iter()
            .filter(|o| self.layer(&o.layer).is_some_and(|l| l.output))
            .filter_map(Object::bounds)
            .reduce(|a, b| a.union(&b))
    }

    /// Add an object, creating its layer if needed. Returns the new id.
    pub fn add_object(
        &mut self,
        name: impl Into<String>,
        layer: &ProjectLayer,
        transform: Affine,
        kind: ObjectKind,
    ) -> u64 {
        if self.layer(&layer.key).is_none() {
            self.layers.push(layer.clone());
        }
        self.next_id = self
            .next_id
            .max(self.objects.iter().map(|o| o.id).max().unwrap_or(0))
            + 1;
        let id = self.next_id;
        self.objects.push(Object {
            id,
            name: name.into(),
            layer: layer.key.clone(),
            transform,
            kind,
        });
        id
    }

    /// Import a file's contents. `name` is the file name, used to detect the
    /// format when the content alone is ambiguous and to name the objects.
    pub fn import(
        &mut self,
        bytes: &[u8],
        name: &str,
        options: &ImportOptions,
    ) -> Result<ImportReport> {
        let imported = import::import(bytes, name, options)?;
        let mut report = ImportReport {
            warnings: imported.warnings,
            ..Default::default()
        };
        for obj in imported.objects {
            let mode = match obj.kind {
                ImportedKind::Vector(_) => LayerMode::Cut,
                ImportedKind::Raster { .. } => LayerMode::Image,
            };
            let layer = ProjectLayer::new(&obj.layer_key, &obj.layer_name, &obj.color, mode);
            if self.layer(&layer.key).is_none() {
                report.new_layers.push(layer.key.clone());
            }
            let kind = match obj.kind {
                ImportedKind::Vector(path) => ObjectKind::Vector { path },
                ImportedKind::Raster {
                    image,
                    width_mm,
                    height_mm,
                } => ObjectKind::Raster {
                    image,
                    width_mm,
                    height_mm,
                },
            };
            report
                .object_ids
                .push(self.add_object(obj.name, &layer, obj.transform, kind));
        }
        if report.object_ids.is_empty() {
            return Err(Error::new(
                ErrorCode::ImportFailed,
                format!("Nothing to import from \"{name}\""),
                if report.warnings.is_empty() {
                    "the file has no paths or images".to_owned()
                } else {
                    report.warnings.join("; ")
                },
                "check the file opens in another editor and contains visible shapes",
            ));
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_after_removal() {
        let mut p = Project::new();
        let layer = ProjectLayer::new("#000000", "black", "#000000", LayerMode::Cut);
        let a = p.add_object(
            "a",
            &layer,
            Affine::IDENTITY,
            ObjectKind::Vector {
                path: BezPath::new(),
            },
        );
        let b = p.add_object(
            "b",
            &layer,
            Affine::IDENTITY,
            ObjectKind::Vector {
                path: BezPath::new(),
            },
        );
        assert!(p.remove_object(b));
        let c = p.add_object(
            "c",
            &layer,
            Affine::IDENTITY,
            ObjectKind::Vector {
                path: BezPath::new(),
            },
        );
        assert_ne!(a, c);
        assert_ne!(b, c);
        assert_eq!(p.layers.len(), 1);
    }

    #[test]
    fn importing_svg_creates_color_layers_once() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10">
            <path d="M0 0 H5" stroke="#ff0000"/><path d="M0 2 H5" stroke="#ff0000"/>
            <path d="M0 4 H5" stroke="#00ff00"/></svg>"##;
        let mut p = Project::new();
        let r = p.import(svg, "a.svg", &ImportOptions::default()).unwrap();
        assert_eq!(r.object_ids.len(), 3);
        assert_eq!(r.new_layers, vec!["#ff0000", "#00ff00"]);
        let r2 = p.import(svg, "a.svg", &ImportOptions::default()).unwrap();
        assert!(r2.new_layers.is_empty());
        assert_eq!(p.objects.len(), 6);
    }

    #[test]
    fn empty_import_is_an_error() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#;
        let err = Project::new()
            .import(svg, "a.svg", &ImportOptions::default())
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::ImportFailed);
    }

    #[test]
    fn new_layers_do_not_fire() {
        let l = ProjectLayer::new("k", "n", "#ff0000", LayerMode::Fill);
        assert_eq!(l.settings.power_pct, 0.0);
    }

    #[test]
    fn project_round_trips_through_json() {
        let mut p = Project::new();
        let layer = ProjectLayer::new("image", "Image", "#808080", LayerMode::Image);
        p.add_object(
            "img",
            &layer,
            Affine::translate((5.0, 5.0)),
            ObjectKind::Raster {
                image: GrayImage::new(1, 1, vec![0]).unwrap(),
                width_mm: 1.0,
                height_mm: 1.0,
            },
        );
        let text = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);
        let b = back.bounds().unwrap();
        assert_eq!((b.min_x, b.max_y), (5.0, 6.0));
    }
}
