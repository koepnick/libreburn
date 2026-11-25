// SPDX-License-Identifier: AGPL-3.0-or-later
//! Grayscale images, resampling and dithering for raster engraving (FR-2).
//!
//! Pixels are 8-bit luminance: 0 is black (full burn) and 255 is white (no
//! burn). Requirement FR-3 puts user filters in a Python pipeline; the
//! dithering here is the built-in last step that turns gray into on/off dots.

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorCode, Result};

/// An 8-bit grayscale image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrayImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major luminance, `width * height` bytes. Base64 in JSON.
    #[serde(with = "b64")]
    pub pixels: Vec<u8>,
}

mod b64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(serde::de::Error::custom)
    }
}

impl GrayImage {
    /// Build an image, checking the buffer size.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        if pixels.len() as u64 != u64::from(width) * u64::from(height) {
            return Err(Error::new(
                ErrorCode::ImportFailed,
                "Image buffer has the wrong size",
                format!(
                    "{width}x{height} needs {} bytes, got {}",
                    u64::from(width) * u64::from(height),
                    pixels.len()
                ),
                "this is a bug in the importer; please report it with the file",
            ));
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// The pixel at `(x, y)`.
    pub fn get(&self, x: u32, y: u32) -> u8 {
        self.pixels[(y as usize) * (self.width as usize) + x as usize]
    }

    /// Resample to `width` x `height` by area averaging (shrinking) or
    /// bilinear interpolation (enlarging).
    pub fn resample(&self, width: u32, height: u32) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) || self.pixels.is_empty() {
            return self.clone();
        }
        let sx = f64::from(self.width) / f64::from(width);
        let sy = f64::from(self.height) / f64::from(height);
        let mut out = Vec::with_capacity(width as usize * height as usize);
        for oy in 0..height {
            for ox in 0..width {
                let v = if sx > 1.0 || sy > 1.0 {
                    self.area_average(
                        f64::from(ox) * sx,
                        f64::from(oy) * sy,
                        f64::from(ox + 1) * sx,
                        f64::from(oy + 1) * sy,
                    )
                } else {
                    self.bilinear(
                        (f64::from(ox) + 0.5) * sx - 0.5,
                        (f64::from(oy) + 0.5) * sy - 0.5,
                    )
                };
                out.push(v.round().clamp(0.0, 255.0) as u8);
            }
        }
        Self {
            width,
            height,
            pixels: out,
        }
    }

    fn area_average(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
        let (mut sum, mut weight) = (0.0, 0.0);
        let ys = y0.floor() as u32..(y1.ceil() as u32).min(self.height);
        for y in ys {
            let wy = (y1.min(f64::from(y + 1)) - y0.max(f64::from(y))).max(0.0);
            for x in x0.floor() as u32..(x1.ceil() as u32).min(self.width) {
                let wx = (x1.min(f64::from(x + 1)) - x0.max(f64::from(x))).max(0.0);
                sum += f64::from(self.get(x, y)) * wx * wy;
                weight += wx * wy;
            }
        }
        if weight > 0.0 { sum / weight } else { 255.0 }
    }

    fn bilinear(&self, x: f64, y: f64) -> f64 {
        let max_x = f64::from(self.width - 1);
        let max_y = f64::from(self.height - 1);
        let (x, y) = (x.clamp(0.0, max_x), y.clamp(0.0, max_y));
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - f64::from(x0), y - f64::from(y0));
        let p = |x, y| f64::from(self.get(x, y));
        let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
        let bottom = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

/// How gray becomes laser dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Dither {
    /// Burn every pixel darker than `level`.
    Threshold {
        /// Cut-off from 0 to 255.
        level: u8,
    },
    /// Floyd-Steinberg error diffusion. Good general-purpose default.
    #[default]
    FloydSteinberg,
    /// Atkinson error diffusion: lighter, higher contrast, good for wood.
    Atkinson,
    /// 8x8 Bayer ordered dither: a regular pattern, no diffusion artifacts.
    Ordered,
    /// No dithering. The planner modulates power by gray level instead.
    Grayscale,
}

/// A 1-bit image: `true` means burn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major dots.
    pub dots: Vec<bool>,
}

impl Bitmap {
    /// Whether to burn at `(x, y)`.
    pub fn get(&self, x: u32, y: u32) -> bool {
        self.dots[(y as usize) * (self.width as usize) + x as usize]
    }
}

const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// Turn `image` into dots. [`Dither::Grayscale`] thresholds at pure white,
/// so every non-white pixel is a dot (its power comes from the gray level).
pub fn dither(image: &GrayImage, mode: Dither) -> Bitmap {
    let (w, h) = (image.width as usize, image.height as usize);
    let dots = match mode {
        Dither::Threshold { level } => image.pixels.iter().map(|&p| p < level).collect(),
        Dither::Grayscale => image.pixels.iter().map(|&p| p < 255).collect(),
        Dither::Ordered => (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let t = (f64::from(BAYER8[y % 8][x % 8]) + 0.5) * 4.0;
                f64::from(image.pixels[y * w + x]) < t
            })
            .collect(),
        Dither::FloydSteinberg => diffuse(
            image,
            &[
                (1, 0, 7.0 / 16.0),
                (-1, 1, 3.0 / 16.0),
                (0, 1, 5.0 / 16.0),
                (1, 1, 1.0 / 16.0),
            ],
        ),
        Dither::Atkinson => diffuse(
            image,
            &[
                (1, 0, 0.125),
                (2, 0, 0.125),
                (-1, 1, 0.125),
                (0, 1, 0.125),
                (1, 1, 0.125),
                (0, 2, 0.125),
            ],
        ),
    };
    Bitmap {
        width: image.width,
        height: image.height,
        dots,
    }
}

fn diffuse(image: &GrayImage, kernel: &[(isize, usize, f64)]) -> Vec<bool> {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut buf: Vec<f64> = image.pixels.iter().map(|&p| f64::from(p)).collect();
    let mut dots = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let old = buf[y * w + x];
            let burn = old < 128.0;
            dots[y * w + x] = burn;
            let err = old - if burn { 0.0 } else { 255.0 };
            for &(dx, dy, k) in kernel {
                let nx = x as isize + dx;
                let ny = y + dy;
                if nx >= 0 && (nx as usize) < w && ny < h {
                    buf[ny * w + nx as usize] += err * k;
                }
            }
        }
    }
    dots
}

/// Encode bytes as base64, for API payloads.
pub fn to_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(level: u8, w: u32, h: u32) -> GrayImage {
        GrayImage::new(w, h, vec![level; (w * h) as usize]).unwrap()
    }

    fn coverage(b: &Bitmap) -> f64 {
        b.dots.iter().filter(|&&d| d).count() as f64 / b.dots.len() as f64
    }

    #[test]
    fn wrong_buffer_size_is_rejected() {
        assert!(GrayImage::new(2, 2, vec![0; 3]).is_err());
    }

    #[test]
    fn error_diffusion_preserves_mean_gray() {
        for mode in [Dither::FloydSteinberg, Dither::Ordered] {
            let b = dither(&flat(64, 64, 64), mode);
            let c = coverage(&b);
            assert!((c - 0.75).abs() < 0.03, "{mode:?} gave coverage {c}");
        }
    }

    #[test]
    fn white_never_burns_and_black_always_does() {
        for mode in [
            Dither::FloydSteinberg,
            Dither::Atkinson,
            Dither::Ordered,
            Dither::Grayscale,
            Dither::Threshold { level: 128 },
        ] {
            assert_eq!(coverage(&dither(&flat(255, 16, 16), mode)), 0.0, "{mode:?}");
            assert_eq!(coverage(&dither(&flat(0, 16, 16), mode)), 1.0, "{mode:?}");
        }
    }

    #[test]
    fn threshold_splits_at_level() {
        let img = GrayImage::new(3, 1, vec![10, 128, 200]).unwrap();
        let b = dither(&img, Dither::Threshold { level: 128 });
        assert_eq!(b.dots, vec![true, false, false]);
    }

    #[test]
    fn shrinking_averages_area() {
        let img = GrayImage::new(2, 2, vec![0, 255, 255, 0]).unwrap();
        let small = img.resample(1, 1);
        assert_eq!(small.pixels, vec![128]);
    }

    #[test]
    fn enlarging_keeps_corners() {
        let img = GrayImage::new(2, 1, vec![0, 200]).unwrap();
        let big = img.resample(4, 1);
        assert_eq!(big.width, 4);
        assert_eq!(big.pixels[0], 0);
        assert_eq!(big.pixels[3], 200);
    }

    #[test]
    fn json_round_trip_uses_base64() {
        let img = GrayImage::new(2, 1, vec![1, 2]).unwrap();
        let text = serde_json::to_string(&img).unwrap();
        assert!(text.contains("\"AQI=\""));
        assert_eq!(serde_json::from_str::<GrayImage>(&text).unwrap(), img);
    }
}
