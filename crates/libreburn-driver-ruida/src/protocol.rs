// SPDX-License-Identifier: AGPL-3.0-or-later
//! Ruida byte scrambling and packet framing.

/// Scramble key used by RDC644x boards.
pub const MAGIC_644X: u8 = 0x88;
/// Single-byte ACK reply. Placeholder: confirm against recorded fixtures.
pub const ACK: u8 = 0xC6;
/// Keep-alive payload. Placeholder: confirm against recorded fixtures.
pub const CMD_PING: [u8; 2] = [0xCC, 0x00];
/// Stop payload. Placeholder: confirm against recorded fixtures.
pub const CMD_STOP: [u8; 2] = [0xD7, 0x00];

/// Scramble one byte with the board's magic key.
pub const fn scramble(b: u8, magic: u8) -> u8 {
    let mut b = b;
    b ^= b >> 7;
    b ^= b << 7;
    b ^= b >> 7;
    b ^= magic;
    b.wrapping_add(1)
}

/// Reverse [`scramble`].
pub const fn unscramble(b: u8, magic: u8) -> u8 {
    let mut b = b.wrapping_sub(1);
    b ^= magic;
    b ^= b >> 7;
    b ^= b << 7;
    b ^= b >> 7;
    b
}

/// 16-bit sum of the scrambled bytes, as sent before each UDP packet.
pub fn checksum(scrambled: &[u8]) -> u16 {
    scrambled
        .iter()
        .fold(0u16, |acc, &b| acc.wrapping_add(u16::from(b)))
}

/// Scramble a payload and prefix its big-endian checksum.
pub fn encode_packet(payload: &[u8], magic: u8) -> Vec<u8> {
    let body: Vec<u8> = payload.iter().map(|&b| scramble(b, magic)).collect();
    let mut packet = checksum(&body).to_be_bytes().to_vec();
    packet.extend_from_slice(&body);
    packet
}

/// Check the checksum and unscramble a packet. Returns `None` if the checksum is wrong.
pub fn decode_packet(packet: &[u8], magic: u8) -> Option<Vec<u8>> {
    let (sum, body) = packet.split_first_chunk::<2>()?;
    if u16::from_be_bytes(*sum) != checksum(body) {
        return None;
    }
    Some(body.iter().map(|&b| unscramble(b, magic)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scramble_round_trips_every_byte_and_key() {
        for magic in [0x11u8, 0x38, 0x88] {
            for b in 0..=255u8 {
                assert_eq!(unscramble(scramble(b, magic), magic), b);
            }
        }
    }

    #[test]
    fn scramble_is_a_permutation() {
        let mut seen = [false; 256];
        for b in 0..=255u8 {
            seen[scramble(b, MAGIC_644X) as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn packet_round_trips_and_rejects_bad_checksum() {
        let packet = encode_packet(&[0xD7, 0x00, 0x12], MAGIC_644X);
        assert_eq!(
            decode_packet(&packet, MAGIC_644X).unwrap(),
            vec![0xD7, 0x00, 0x12]
        );
        let mut bad = packet.clone();
        bad[0] ^= 0xFF;
        assert_eq!(decode_packet(&bad, MAGIC_644X), None);
    }
}
