// SPDX-License-Identifier: AGPL-3.0-or-later
//! EZCAD `.cor` lens correction files.
//!
//! A correction table is 65 x 65 (dx, dy) offsets. Two file layouts exist,
//! per MeerK40t's `GalvoController._read_correction_file` (Copyright (c)
//! 2021 meerk40t, MIT License):
//! - `LMC1COR_1.0` (UTF-16 label, 0x16 bytes), then 0x1FA header bytes,
//!   then 65 x 65 pairs of little-endian f64;
//! - otherwise 0x16 + 0xE header bytes, then 65 x 65 pairs of little-endian i32.
//!
//! Each offset is rounded and sent as sign-magnitude: negative values become
//! `0x8000 + |v|`.

use libreburn_core::{Error, ErrorCode, Result};

/// Table side length.
pub const SIDE: usize = 65;
/// Entries in a table.
pub const ENTRIES: usize = SIDE * SIDE;

const LABEL_LEN: usize = 0x16;
const FLOAT_HEADER: usize = 0x1FA;
const INT_HEADER: usize = 0xE;

fn bad(why: String) -> Error {
    Error::new(
        ErrorCode::ProfileInvalid,
        "Could not read the lens correction file",
        why,
        "point connection.correction_file at the .cor file from the machine's EZCAD folder, \
         or leave it empty for no correction",
    )
}

fn encode(v: f64) -> u16 {
    let v = v.round() as i64;
    let v = if v >= 0 { v } else { -v + 0x8000 };
    (v & 0xFFFF) as u16
}

/// Parse a `.cor` file into the table the board expects.
pub fn parse(bytes: &[u8]) -> Result<Vec<(u16, u16)>> {
    let label = bytes
        .get(..LABEL_LEN)
        .ok_or_else(|| bad(format!("the file is only {} bytes", bytes.len())))?;
    let words: Vec<u16> = label
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let is_float = String::from_utf16_lossy(&words) == "LMC1COR_1.0";
    let (start, width) = if is_float {
        (LABEL_LEN + FLOAT_HEADER, 8)
    } else {
        (LABEL_LEN + INT_HEADER, 4)
    };
    let need = start + ENTRIES * 2 * width;
    if bytes.len() < need {
        return Err(bad(format!(
            "the file has {} bytes, a {} table needs {need}",
            bytes.len(),
            if is_float { "LMC1COR_1.0" } else { "integer" }
        )));
    }
    let value = |i: usize| -> f64 {
        let at = start + i * width;
        if is_float {
            f64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"))
        } else {
            f64::from(i32::from_le_bytes(
                bytes[at..at + 4].try_into().expect("4 bytes"),
            ))
        }
    };
    (0..ENTRIES)
        .map(|i| {
            let (dx, dy) = (value(2 * i), value(2 * i + 1));
            if !dx.is_finite() || !dy.is_finite() {
                return Err(bad(format!("entry {i} is not a number")));
            }
            Ok((encode(dx), encode(dy)))
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A synthetic integer-format file whose entry i is (i, -i).
    pub(crate) fn int_file() -> Vec<u8> {
        let mut f = vec![0u8; LABEL_LEN + INT_HEADER];
        for i in 0..ENTRIES as i32 {
            f.extend_from_slice(&i.to_le_bytes());
            f.extend_from_slice(&(-i).to_le_bytes());
        }
        f
    }

    #[test]
    fn integer_format() {
        let t = parse(&int_file()).unwrap();
        assert_eq!(t.len(), ENTRIES);
        assert_eq!(t[0], (0, 0));
        assert_eq!(t[5], (5, 0x8005));
    }

    #[test]
    fn float_format() {
        let mut f: Vec<u8> = "LMC1COR_1.0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        f.resize(LABEL_LEN + FLOAT_HEADER, 0);
        for i in 0..ENTRIES {
            f.extend_from_slice(&(i as f64 + 0.4).to_le_bytes());
            f.extend_from_slice(&(-2.6f64).to_le_bytes());
        }
        let t = parse(&f).unwrap();
        assert_eq!(t[7], (7, 0x8003));
    }

    #[test]
    fn short_files_are_rejected() {
        assert_eq!(parse(&[0; 10]).unwrap_err().code, ErrorCode::ProfileInvalid);
        let mut f = int_file();
        f.truncate(f.len() - 1);
        assert!(parse(&f).is_err());
    }
}
