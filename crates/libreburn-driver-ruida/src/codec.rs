// SPDX-License-Identifier: AGPL-3.0-or-later
//! Ruida command encoding: opcodes, 7-bit number packing and command splitting.
//!
//! Every command starts with a byte that has its top bit set; every argument
//! byte has it clear. Numbers are packed big-endian, seven bits per byte.
//!
//! Opcodes and argument layouts follow MeerK40t's `ruida/rdjob.py`
//! (Copyright (c) 2021 meerk40t, MIT License; see `docs/protocols.md`).
//! Values marked "unconfirmed" have not yet been checked against a capture
//! from a real board in `hil/fixtures/`.

/// Move to absolute X, Y (µm), laser off. Args: abs(5), abs(5).
pub const MOVE_ABS_XY: u8 = 0x88;
/// Move by dX, dY (µm), laser off. Args: rel(2), rel(2).
pub const MOVE_REL_XY: u8 = 0x89;
/// Move by dX, laser off. Args: rel(2).
pub const MOVE_REL_X: u8 = 0x8A;
/// Move by dY, laser off. Args: rel(2).
pub const MOVE_REL_Y: u8 = 0x8B;
/// Cut to absolute X, Y (µm), laser on. Args: abs(5), abs(5).
pub const CUT_ABS_XY: u8 = 0xA8;
/// Cut by dX, dY, laser on. Args: rel(2), rel(2).
pub const CUT_REL_XY: u8 = 0xA9;
/// Cut by dX, laser on. Args: rel(2).
pub const CUT_REL_X: u8 = 0xAA;
/// Cut by dY, laser on. Args: rel(2).
pub const CUT_REL_Y: u8 = 0xAB;

/// Power commands carry a 14-bit value where 16383 is 100 %.
pub mod power {
    /// Immediate power, laser 1. Args: power(2).
    pub const IMD_POWER_1: [u8; 1] = [0xC7];
    /// End power, laser 1. Args: power(2).
    pub const END_POWER_1: [u8; 1] = [0xC8];
    /// Minimum power, laser 1. Args: power(2).
    pub const MIN_POWER_1: [u8; 2] = [0xC6, 0x01];
    /// Maximum power, laser 1. Args: power(2).
    pub const MAX_POWER_1: [u8; 2] = [0xC6, 0x02];
    /// Minimum power, laser 2. Args: power(2).
    pub const MIN_POWER_2: [u8; 2] = [0xC6, 0x21];
    /// Maximum power, laser 2. Args: power(2).
    pub const MAX_POWER_2: [u8; 2] = [0xC6, 0x22];
    /// Laser on delay. Args: time(5), µs.
    pub const LASER_ON_DELAY: [u8; 2] = [0xC6, 0x12];
    /// Laser off delay. Args: time(5), µs.
    pub const LASER_OFF_DELAY: [u8; 2] = [0xC6, 0x13];
    /// Per-layer minimum power, laser 1. Args: part(1), power(2).
    pub const MIN_POWER_1_PART: [u8; 2] = [0xC6, 0x31];
    /// Per-layer maximum power, laser 1. Args: part(1), power(2).
    pub const MAX_POWER_1_PART: [u8; 2] = [0xC6, 0x32];
    /// Per-layer minimum power, laser 2. Args: part(1), power(2).
    pub const MIN_POWER_2_PART: [u8; 2] = [0xC6, 0x41];
    /// Per-layer maximum power, laser 2. Args: part(1), power(2).
    pub const MAX_POWER_2_PART: [u8; 2] = [0xC6, 0x42];
}

/// Cut speed, laser 1. Args: speed(5), µm/s.
pub const SPEED_LASER_1: [u8; 2] = [0xC9, 0x02];
/// Per-layer cut speed. Args: part(1), speed(5), µm/s.
pub const SPEED_LASER_1_PART: [u8; 2] = [0xC9, 0x04];
/// End of a layer's settings.
pub const LAYER_END: [u8; 3] = [0xCA, 0x01, 0x00];
/// Select laser 1.
pub const LASER_DEVICE_0: [u8; 3] = [0xCA, 0x01, 0x10];
/// Air assist off.
pub const AIR_ASSIST_OFF: [u8; 3] = [0xCA, 0x01, 0x12];
/// Air assist on.
pub const AIR_ASSIST_ON: [u8; 3] = [0xCA, 0x01, 0x13];
/// Laser 2 offset off.
pub const EN_LASER_2_OFFSET_0: [u8; 3] = [0xCA, 0x01, 0x30];
/// Select the layer that following moves belong to. Args: part(1).
pub const LAYER_NUMBER_PART: [u8; 2] = [0xCA, 0x02];
/// Enable the tube for this layer. Args: switch(1).
pub const EN_LASER_TUBE_START: [u8; 2] = [0xCA, 0x03];
/// Layer colour shown on the panel. Args: part(1), colour(5).
pub const LAYER_COLOR_PART: [u8; 2] = [0xCA, 0x06];
/// Highest layer number in the file. Args: part(1).
pub const MAX_LAYER_PART: [u8; 2] = [0xCA, 0x22];
/// Per-layer work mode. Args: part(1), mode(1).
pub const WORK_MODE_PART: [u8; 2] = [0xCA, 0x41];

/// Session payloads. On UDP the controller answers every packet with one of
/// these, swizzled.
pub const ACK: u8 = 0xCC;
/// Reply: error.
pub const ERR: u8 = 0xCD;
/// Keep-alive / enquiry.
pub const ENQ: u8 = 0xCE;
/// Reply: bad checksum, resend.
pub const NAK: u8 = 0xCF;

/// End of file. Must be the last byte of a job.
pub const END_OF_FILE: u8 = 0xD7;
/// Start processing.
pub const START_PROCESS: [u8; 2] = [0xD8, 0x00];
/// Stop processing (abort).
pub const STOP_PROCESS: [u8; 2] = [0xD8, 0x01];
/// Pause processing.
pub const PAUSE_PROCESS: [u8; 2] = [0xD8, 0x02];
/// Resume processing.
pub const RESTORE_PROCESS: [u8; 2] = [0xD8, 0x03];
/// Use machine zero as the reference point.
pub const REF_POINT_2: [u8; 2] = [0xD8, 0x10];
/// Home X and Y against the end stops.
pub const HOME_XY: [u8; 2] = [0xD8, 0x2A];

/// Rapid move to absolute X, Y. Args: options(1), abs(5), abs(5).
pub const RAPID_MOVE_XY: [u8; 2] = [0xD9, 0x10];
/// Rapid option: coordinates from the origin, no light. Unconfirmed, see module docs.
pub const RAPID_OPTION_ORIGIN: u8 = 0x00;
/// Rapid option: plain move.
pub const RAPID_OPTION_NONE: u8 = 0x02;

/// Read a controller memory word. Args: address(2).
pub const GET_SETTING: [u8; 2] = [0xDA, 0x00];
/// Write a controller memory word, or the reply to a read. Args: address(2), v(5), v(5).
pub const SET_SETTING: [u8; 2] = [0xDA, 0x01];

/// Checksum of the job body, sent just before [`END_OF_FILE`]. Args: value(5).
pub const SET_FILE_SUM: [u8; 2] = [0xE5, 0x05];
/// Absolute coordinates follow.
pub const SET_ABSOLUTE: [u8; 2] = [0xE6, 0x01];
/// End of a block of moves.
pub const BLOCK_END: [u8; 2] = [0xE7, 0x00];
/// Job bounds, top left. Args: abs(5), abs(5).
pub const PROCESS_TOP_LEFT: [u8; 2] = [0xE7, 0x03];
/// Job bounds, bottom right. Args: abs(5), abs(5).
pub const PROCESS_BOTTOM_RIGHT: [u8; 2] = [0xE7, 0x07];
/// Document minimum point. Args: abs(5), abs(5).
pub const DOCUMENT_MIN_POINT: [u8; 2] = [0xE7, 0x50];
/// Document maximum point. Args: abs(5), abs(5).
pub const DOCUMENT_MAX_POINT: [u8; 2] = [0xE7, 0x51];
/// Layer minimum point. Args: part(1), abs(5), abs(5).
pub const PART_MIN_POINT: [u8; 2] = [0xE7, 0x52];
/// Layer maximum point. Args: part(1), abs(5), abs(5).
pub const PART_MAX_POINT: [u8; 2] = [0xE7, 0x53];
/// Set the reference point.
pub const REF_POINT_SET: u8 = 0xF0;
/// Block cutting on or off. Args: enable(1).
pub const ENABLE_BLOCK_CUTTING: [u8; 2] = [0xF1, 0x02];

/// Memory addresses for [`GET_SETTING`].
pub mod mem {
    /// Machine status bits.
    pub const MACHINE_STATUS: u16 = 0x0400;
    /// Current X, µm.
    pub const CURRENT_X: u16 = 0x0421;
    /// Current Y, µm.
    pub const CURRENT_Y: u16 = 0x0431;
    /// Current Z, µm.
    pub const CURRENT_Z: u16 = 0x0441;
    /// Current U, µm.
    pub const CURRENT_U: u16 = 0x0451;
    /// Bed size X, µm.
    pub const BED_SIZE_X: u16 = 0x0026;
    /// Bed size Y, µm.
    pub const BED_SIZE_Y: u16 = 0x0036;
    /// Controller card ID.
    pub const CARD_ID: u16 = 0x057E;

    /// Status bit: a job is running.
    pub const STATUS_JOB_RUNNING: u32 = 0x0000_0001;
    /// Status bit: end of a part.
    pub const STATUS_PART_END: u32 = 0x0000_0002;
    /// Status bit: the head is moving.
    pub const STATUS_MOVING: u32 = 0x0100_0000;

    /// Address for a parameter name used by `Driver::read_param`.
    pub fn by_name(name: &str) -> Option<u16> {
        Some(match name {
            "machine_status" => MACHINE_STATUS,
            "x" => CURRENT_X,
            "y" => CURRENT_Y,
            "z" => CURRENT_Z,
            "u" => CURRENT_U,
            "bed_x" => BED_SIZE_X,
            "bed_y" => BED_SIZE_Y,
            "card_id" => CARD_ID,
            _ => {
                let hex = name.strip_prefix("0x")?;
                return u16::from_str_radix(hex, 16).ok();
            }
        })
    }
}

/// Pack a 14-bit two's-complement value into two 7-bit bytes.
pub fn encode14(v: i32) -> [u8; 2] {
    [((v >> 7) & 0x7F) as u8, (v & 0x7F) as u8]
}

/// Pack a 35-bit value into five 7-bit bytes (used for 32-bit signed values).
pub fn encode35(v: i64) -> [u8; 5] {
    [
        ((v >> 28) & 0x7F) as u8,
        ((v >> 21) & 0x7F) as u8,
        ((v >> 14) & 0x7F) as u8,
        ((v >> 7) & 0x7F) as u8,
        (v & 0x7F) as u8,
    ]
}

/// Unpack two 7-bit bytes into a signed 14-bit value.
pub fn decode14(b: &[u8]) -> i32 {
    let v = (i32::from(b[0] & 0x7F) << 7) | i32::from(b[1] & 0x7F);
    if v > 0x1FFF { v - 0x4000 } else { v }
}

/// Unpack two 7-bit bytes into an unsigned 14-bit value.
pub fn decode_u14(b: &[u8]) -> u16 {
    ((u16::from(b[0] & 0x7F)) << 7) | u16::from(b[1] & 0x7F)
}

/// Unpack five 7-bit bytes into an unsigned 35-bit value.
pub fn decode_u35(b: &[u8]) -> u64 {
    b[..5]
        .iter()
        .fold(0u64, |acc, &x| (acc << 7) | u64::from(x & 0x7F))
}

/// Unpack five 7-bit bytes into a signed 32-bit coordinate.
pub fn decode32(b: &[u8]) -> i32 {
    (decode_u35(b) & 0xFFFF_FFFF) as u32 as i32
}

/// Relative moves are limited to the 14-bit signed range.
pub const REL_LIMIT_UM: i32 = 8191;

/// Encode a power percentage (0 to 100) as the 14-bit wire value.
pub fn encode_power(pct: f64) -> [u8; 2] {
    let v = (pct.clamp(0.0, 100.0) * 16383.0 / 100.0).round() as i32;
    encode14(v)
}

/// Decode a 14-bit wire power into percent.
pub fn decode_power(b: &[u8]) -> f64 {
    f64::from(decode_u14(b)) * 100.0 / 16383.0
}

/// Encode a speed in mm/s as µm/s.
pub fn encode_speed(mm_s: f64) -> [u8; 5] {
    encode35((mm_s * 1000.0).round() as i64)
}

/// Encode a coordinate in µm.
pub fn encode_coord(um: i32) -> [u8; 5] {
    encode35(i64::from(um))
}

/// Encode a memory address. Addresses are written as their two raw bytes
/// (each below 0x80), as MeerK40t's `MEM_*` constants are.
pub fn encode_mem(addr: u16) -> [u8; 2] {
    (addr & 0x7F7F).to_be_bytes()
}

/// Decode a memory address from its two raw bytes.
pub fn decode_mem(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// Split a payload into commands: each starts at a byte with the top bit set.
pub fn split_commands(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &b) in data.iter().enumerate() {
        if b >= 0x80 && i != start {
            out.push(&data[start..i]);
            start = i;
        }
    }
    if start != data.len() {
        out.push(&data[start..]);
    }
    out
}

/// Concatenate parts into one command.
pub fn cmd(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// A short human-readable description of one command, for the device panel.
pub fn describe(c: &[u8]) -> String {
    let arg = |from: usize, len: usize| c.get(from..from + len);
    let um = |from: usize| arg(from, 5).map(decode32);
    match c {
        [MOVE_ABS_XY, ..] => format!("move abs {:?} {:?} µm", um(1), um(6)),
        [CUT_ABS_XY, ..] => format!("cut abs {:?} {:?} µm", um(1), um(6)),
        [MOVE_REL_XY, a, b, d, e] => {
            format!(
                "move rel {} {} µm",
                decode14(&[*a, *b]),
                decode14(&[*d, *e])
            )
        }
        [CUT_REL_XY, a, b, d, e] => {
            format!("cut rel {} {} µm", decode14(&[*a, *b]), decode14(&[*d, *e]))
        }
        [MOVE_REL_X, a, b] => format!("move rel x {} µm", decode14(&[*a, *b])),
        [MOVE_REL_Y, a, b] => format!("move rel y {} µm", decode14(&[*a, *b])),
        [CUT_REL_X, a, b] => format!("cut rel x {} µm", decode14(&[*a, *b])),
        [CUT_REL_Y, a, b] => format!("cut rel y {} µm", decode14(&[*a, *b])),
        [0xC6, 0x01 | 0x02 | 0x21 | 0x22, a, b] => {
            format!("power {:.1} %", decode_power(&[*a, *b]))
        }
        [0xC6, 0x31 | 0x32 | 0x41 | 0x42, part, a, b] => {
            format!("layer {part} power {:.1} %", decode_power(&[*a, *b]))
        }
        [0xC9, 0x02, rest @ ..] if rest.len() == 5 => {
            format!("speed {:.1} mm/s", decode_u35(rest) as f64 / 1000.0)
        }
        [0xC9, 0x04, part, rest @ ..] if rest.len() == 5 => {
            format!(
                "layer {part} speed {:.1} mm/s",
                decode_u35(rest) as f64 / 1000.0
            )
        }
        [ACK] => "ack".into(),
        [NAK] => "nak".into(),
        [ERR] => "error".into(),
        [ENQ] => "enquiry".into(),
        [END_OF_FILE] => "end of file".into(),
        [0xD8, 0x00] => "start process".into(),
        [0xD8, 0x01] => "stop".into(),
        [0xD8, 0x02] => "pause".into(),
        [0xD8, 0x03] => "resume".into(),
        [0xD8, 0x2A] => "home xy".into(),
        [0xD9, 0x10, opt, ..] => format!("rapid xy opt {opt:#04x} {:?} {:?} µm", um(3), um(8)),
        [0xDA, 0x00, a, b] => format!("read mem {:#06x}", decode_mem(&[*a, *b])),
        [0xDA, 0x01, a, b, rest @ ..] if rest.len() >= 5 => format!(
            "mem {:#06x} = {:#x}",
            decode_mem(&[*a, *b]),
            decode_u35(rest)
        ),
        _ => format!("{:02x?}", c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_round_trip() {
        for v in [-8192, -1, 0, 1, 127, 128, 8191] {
            assert_eq!(decode14(&encode14(v)), v);
        }
        for v in [i32::MIN, -1, 0, 1, 300_000, i32::MAX] {
            assert_eq!(decode32(&encode35(i64::from(v))), v);
        }
    }

    #[test]
    fn known_encodings() {
        // 100 mm = 100 000 µm = 0x186A0 -> 7-bit groups 00 00 06 0D 20.
        assert_eq!(encode_coord(100_000), [0x00, 0x00, 0x06, 0x0D, 0x20]);
        assert_eq!(encode_power(100.0), [0x7F, 0x7F]);
        assert_eq!(encode_power(50.0), [0x40, 0x00]);
        assert_eq!(encode_power(0.0), [0x00, 0x00]);
        assert_eq!(encode14(-1), [0x7F, 0x7F]);
        assert_eq!(encode_mem(mem::CURRENT_X), [0x04, 0x21]);
        // A status read, as MeerK40t sends it: DA 00 04 00.
        assert_eq!(
            cmd(&[&GET_SETTING, &encode_mem(mem::MACHINE_STATUS)]),
            [0xDA, 0x00, 0x04, 0x00]
        );
        assert_eq!(encode_mem(mem::CARD_ID), [0x05, 0x7E]);
    }

    #[test]
    fn power_is_clamped() {
        assert_eq!(encode_power(250.0), encode_power(100.0));
        assert_eq!(encode_power(-5.0), encode_power(0.0));
    }

    #[test]
    fn every_argument_byte_has_the_top_bit_clear() {
        for b in encode35(-123_456_789).iter().chain(encode14(-4000).iter()) {
            assert!(*b < 0x80);
        }
    }

    #[test]
    fn splits_on_opcodes() {
        let data = [0x88, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0xD8, 0x00, 0xD7];
        let parts = split_commands(&data);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1], &[0xD8, 0x00]);
        assert_eq!(parts[2], &[0xD7]);
    }

    #[test]
    fn param_names_resolve() {
        assert_eq!(mem::by_name("x"), Some(mem::CURRENT_X));
        assert_eq!(mem::by_name("0x057e"), Some(mem::CARD_ID));
        assert_eq!(mem::by_name("nope"), None);
    }
}
