// SPDX-License-Identifier: AGPL-3.0-or-later
//! File import (requirements FR-1 and FR-2).
//!
//! Every importer returns geometry in millimetres in job space (origin top
//! left, Y down), grouped by layer key and color.

mod dxf;
mod image;
mod svg;

use kurbo::{Affine, BezPath};
use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorCode, Result};
use crate::raster::GrayImage;

/// A supported input format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileFormat {
    /// Scalable Vector Graphics.
    Svg,
    /// AutoCAD Drawing Exchange Format (ASCII or binary).
    Dxf,
    /// PNG image.
    Png,
    /// JPEG image.
    Jpeg,
    /// WebP image.
    Webp,
}

impl FileFormat {
    /// Detect the format from the content, falling back to the file extension.
    pub fn detect(bytes: &[u8], name: &str) -> Result<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Ok(Self::Png);
        }
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Ok(Self::Jpeg);
        }
        if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return Ok(Self::Webp);
        }
        if bytes.starts_with(b"AutoCAD Binary DXF") {
            return Ok(Self::Dxf);
        }
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "svg" | "svgz" => return Ok(Self::Svg),
            "dxf" => return Ok(Self::Dxf),
            _ => {}
        }
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
        if head.contains("<svg") {
            return Ok(Self::Svg);
        }
        if head.trim_start().starts_with('0') && head.contains("SECTION") {
            return Ok(Self::Dxf);
        }
        Err(Error::new(
            ErrorCode::ImportUnsupported,
            format!("Cannot import \"{name}\""),
            "the file is not SVG, DXF, PNG, JPEG or WebP",
            "export the design as SVG or DXF, or the image as PNG",
        ))
    }
}

/// Import settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportOptions {
    /// Resolution for raster images, in dots per inch.
    pub raster_dpi: f64,
    /// DPI assumed for SVG files that give sizes in px without units (CSS uses 96).
    pub svg_dpi: f64,
    /// Units for DXF files whose header says "unitless". Millimetres if unset.
    pub dxf_unitless_mm: f64,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            raster_dpi: 254.0,
            svg_dpi: 96.0,
            dxf_unitless_mm: 1.0,
        }
    }
}

/// Geometry from an importer, before it joins a project.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportedKind {
    /// Curves in mm.
    Vector(BezPath),
    /// A grayscale image covering `(0, 0)` to `(width_mm, height_mm)`.
    Raster {
        /// Pixels.
        image: GrayImage,
        /// Width in mm.
        width_mm: f64,
        /// Height in mm.
        height_mm: f64,
    },
}

/// One imported object.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedObject {
    /// Display name.
    pub name: String,
    /// Layer key the object belongs to.
    pub layer_key: String,
    /// Layer display name.
    pub layer_name: String,
    /// Layer color as `#rrggbb`.
    pub color: String,
    /// Placement.
    pub transform: Affine,
    /// Geometry.
    pub kind: ImportedKind,
}

/// The result of importing one file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Imported {
    /// Objects in file order.
    pub objects: Vec<ImportedObject>,
    /// Things that were skipped or approximated.
    pub warnings: Vec<String>,
}

/// Import a file. `name` is used for format detection and object names.
pub fn import(bytes: &[u8], name: &str, options: &ImportOptions) -> Result<Imported> {
    match FileFormat::detect(bytes, name)? {
        FileFormat::Svg => svg::import(bytes, name, options),
        FileFormat::Dxf => dxf::import(bytes, name, options),
        FileFormat::Png | FileFormat::Jpeg | FileFormat::Webp => {
            image::import(bytes, name, options)
        }
    }
}

/// Format an RGB color as `#rrggbb`.
pub(crate) fn hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

pub(crate) fn failed(name: &str, why: impl Into<String>, hint: &str) -> Error {
    Error::new(
        ErrorCode::ImportFailed,
        format!("Could not import \"{name}\""),
        why,
        hint,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_magic_then_extension() {
        assert_eq!(
            FileFormat::detect(b"\x89PNG\r\n\x1a\nxx", "a.svg").unwrap(),
            FileFormat::Png
        );
        assert_eq!(
            FileFormat::detect(b"anything", "A.DXF").unwrap(),
            FileFormat::Dxf
        );
        assert_eq!(
            FileFormat::detect(b"<?xml?><svg>", "noext").unwrap(),
            FileFormat::Svg
        );
        assert_eq!(
            FileFormat::detect(b"zzz", "x.pdf").unwrap_err().code,
            ErrorCode::ImportUnsupported
        );
    }
}
