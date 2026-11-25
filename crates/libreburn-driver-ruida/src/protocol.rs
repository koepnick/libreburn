// SPDX-License-Identifier: AGPL-3.0-or-later
//! Ruida byte scrambling and packet framing.

/// Scramble key used by RDC644x boards.
pub const MAGIC_644X: u8 = 0x88;
/// UDP port the controller listens on for commands.
pub const UDP_COMMAND_PORT: u16 = 50200;
/// UDP port replies are sent to.
pub const UDP_REPLY_PORT: u16 = 40200;
/// Largest payload sent in one UDP packet. Jobs are split at command boundaries.
pub const MAX_PACKET_PAYLOAD: usize = 1000;

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

/// Scramble a whole buffer.
pub fn scramble_all(data: &[u8], magic: u8) -> Vec<u8> {
    data.iter().map(|&b| scramble(b, magic)).collect()
}

/// Unscramble a whole buffer.
pub fn unscramble_all(data: &[u8], magic: u8) -> Vec<u8> {
    data.iter().map(|&b| unscramble(b, magic)).collect()
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
    fn ack_on_the_wire_matches_known_byte() {
        // RDC644x boards answer with 0xC6, which is the ACK payload 0xCC scrambled.
        assert_eq!(scramble(crate::codec::ACK, MAGIC_644X), 0xC6);
        assert_eq!(scramble(crate::codec::NAK, MAGIC_644X), 0x48);
    }

    #[test]
    fn known_packet_bytes() {
        // ENQ (0xCE) with key 0x88: scrambled 0xC8, checksum 0x00C8.
        assert_eq!(
            encode_packet(&[crate::codec::ENQ], MAGIC_644X),
            [0x00, 0xC8, 0xC8]
        );
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
