// SPDX-License-Identifier: AGPL-3.0-or-later
//! PNG, JPEG and WebP import (FR-2).

use kurbo::Affine;

use super::{ImportOptions, Imported, ImportedKind, ImportedObject, failed};
use crate::raster::GrayImage;

/// Largest accepted image, in pixels, so a hostile file cannot exhaust memory.
const MAX_PIXELS: u64 = 200_000_000;

pub(super) fn import(bytes: &[u8], name: &str, options: &ImportOptions) -> crate::Result<Imported> {
    if !(options.raster_dpi.is_finite() && options.raster_dpi > 0.0) {
        return Err(failed(
            name,
            format!("raster DPI is {}", options.raster_dpi),
            "use a positive DPI such as 254",
        ));
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| failed(name, e.to_string(), "check the file is a complete image"))?;
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| failed(name, e.to_string(), "check the file is a complete image"))?;
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(failed(
            name,
            format!("the image is {w}x{h} pixels, over the {MAX_PIXELS} pixel limit"),
            "scale the image down before importing",
        ));
    }
    let decoded = image::load_from_memory(bytes)
        .map_err(|e| failed(name, e.to_string(), "re-export the image as PNG"))?;
    // Composite transparency onto white, which is "no burn".
    let rgba = decoded.to_rgba8();
    let pixels = rgba
        .pixels()
        .map(|p| {
            let [r, g, b, a] = p.0;
            let luma = 0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
            let alpha = f64::from(a) / 255.0;
            (luma * alpha + 255.0 * (1.0 - alpha)).round() as u8
        })
        .collect();
    let image = GrayImage::new(w, h, pixels)?;
    let mm_per_px = 25.4 / options.raster_dpi;
    Ok(Imported {
        objects: vec![ImportedObject {
            name: name.to_owned(),
            layer_key: "image".into(),
            layer_name: "Image".into(),
            color: "#808080".into(),
            transform: Affine::IDENTITY,
            kind: ImportedKind::Raster {
                image,
                width_mm: f64::from(w) * mm_per_px,
                height_mm: f64::from(h) * mm_per_px,
            },
        }],
        warnings: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn png_size_follows_dpi() {
        let bytes = png(254, 127, [0, 0, 0, 255]);
        let out = import(&bytes, "a.png", &ImportOptions::default()).unwrap();
        let ImportedKind::Raster {
            width_mm,
            height_mm,
            image,
        } = &out.objects[0].kind
        else {
            panic!("expected raster")
        };
        assert!((width_mm - 25.4).abs() < 1e-9);
        assert!((height_mm - 12.7).abs() < 1e-9);
        assert!(image.pixels.iter().all(|&p| p == 0));
    }

    #[test]
    fn transparent_pixels_become_white() {
        let bytes = png(2, 2, [0, 0, 0, 0]);
        let out = import(&bytes, "a.png", &ImportOptions::default()).unwrap();
        let ImportedKind::Raster { image, .. } = &out.objects[0].kind else {
            panic!()
        };
        assert!(image.pixels.iter().all(|&p| p == 255));
    }

    #[test]
    fn webp_and_jpeg_decode() {
        for fmt in [image::ImageFormat::Jpeg, image::ImageFormat::WebP] {
            let img = image::RgbImage::from_pixel(4, 4, image::Rgb([255, 255, 255]));
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, fmt).unwrap();
            let got = import(out.get_ref(), "x", &ImportOptions::default()).unwrap();
            assert_eq!(got.objects.len(), 1, "{fmt:?}");
        }
    }

    #[test]
    fn truncated_file_reports_an_error() {
        let bytes = png(4, 4, [0, 0, 0, 255]);
        let err = import(&bytes[..20], "a.png", &ImportOptions::default()).unwrap_err();
        assert_eq!(err.code, crate::ErrorCode::ImportFailed);
    }
}
