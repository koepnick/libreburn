// SPDX-License-Identifier: AGPL-3.0-or-later
//! JCZ / BJJCZ LMC command encoding.
//!
//! Every command is six little-endian 16-bit words: an opcode and five
//! arguments. "Single" commands (opcode < 0x8000) are sent one at a time and
//! answered with four status words. "List" commands (opcode >= 0x8000) are
//! packed 256 at a time into 3072-byte list packets, padded with
//! [`LIST_END_OF_LIST`], and run by the board after `ExecuteList`.
//!
//! Opcodes, argument layouts and unit conversions follow MeerK40t's
//! `balormk/controller.py` (Copyright (c) 2021 meerk40t, MIT License), which
//! descends from the balor project. Values marked "unconfirmed" have not yet
//! been checked against a capture from a real board in `hil/fixtures/`.

/// One 12-byte command.
pub type Command = [u8; 12];

/// Commands per list packet.
pub const LIST_LEN: usize = 256;
/// Bytes per list packet.
pub const LIST_PACKET_BYTES: usize = LIST_LEN * 12;

/// Single commands.
pub mod single {
    /// Disable the laser output stage.
    pub const DISABLE_LASER: u16 = 0x0002;
    /// Enable the laser output stage.
    pub const ENABLE_LASER: u16 = 0x0004;
    /// Start running the uploaded lists.
    pub const EXECUTE_LIST: u16 = 0x0005;
    /// PWM pulse width.
    pub const SET_PWM_PULSE_WIDTH: u16 = 0x0006;
    /// Read the version; word 3 of the reply holds the status bits.
    pub const GET_VERSION: u16 = 0x0007;
    /// Read the serial number.
    pub const GET_SERIAL_NO: u16 = 0x0009;
    /// Read the list status.
    pub const GET_LIST_STATUS: u16 = 0x000A;
    /// Read the beam position.
    pub const GET_POSITION_XY: u16 = 0x000C;
    /// Move the galvos immediately. Args: x, y, angle, distance.
    pub const GOTO_XY: u16 = 0x000D;
    /// Laser signal off.
    pub const LASER_SIGNAL_OFF: u16 = 0x000E;
    /// Laser signal on. Never sent by this driver.
    pub const LASER_SIGNAL_ON: u16 = 0x000F;
    /// One line of the correction table.
    pub const WRITE_COR_LINE: u16 = 0x0010;
    /// Clear the list buffer.
    pub const RESET_LIST: u16 = 0x0012;
    /// Resume a paused list.
    pub const RESTART_LIST: u16 = 0x0013;
    /// Start (1) or blank (0) the correction table.
    pub const WRITE_COR_TABLE: u16 = 0x0015;
    /// Control mode.
    pub const SET_CONTROL_MODE: u16 = 0x0016;
    /// Delay mode.
    pub const SET_DELAY_MODE: u16 = 0x0017;
    /// Marks the end of the uploaded list data.
    pub const SET_END_OF_LIST: u16 = 0x0019;
    /// First pulse killer.
    pub const SET_FIRST_PULSE_KILLER: u16 = 0x001A;
    /// Laser mode.
    pub const SET_LASER_MODE: u16 = 0x001B;
    /// Timing mode.
    pub const SET_TIMING: u16 = 0x001C;
    /// Standby parameters.
    pub const SET_STANDBY: u16 = 0x001D;
    /// PWM half period.
    pub const SET_PWM_HALF_PERIOD: u16 = 0x001E;
    /// Abort the running list.
    pub const STOP_EXECUTE: u16 = 0x001F;
    /// Pause the running list.
    pub const STOP_LIST: u16 = 0x0020;
    /// Set the digital output port bits.
    pub const WRITE_PORT: u16 = 0x0021;
    /// Analog port 1.
    pub const WRITE_ANALOG_PORT_1: u16 = 0x0022;
    /// Read the input port.
    pub const READ_PORT: u16 = 0x0025;
    /// First pulse killer parameters.
    pub const SET_FPK_PARAM_2: u16 = 0x002E;
    /// On-the-fly resolution.
    pub const SET_FLY_RES: u16 = 0x0032;
    /// Fiber master oscillator: 1 opens it (the source can fire), 0 closes it.
    pub const FIBER_SET_MO: u16 = 0x0033;
    /// Disable Z.
    pub const DISABLE_Z: u16 = 0x0039;
    /// Enable Z.
    pub const ENABLE_Z: u16 = 0x003A;
    /// Reset the board.
    pub const RESET: u16 = 0x0040;
    /// Mark time of the last list.
    pub const GET_MARK_TIME: u16 = 0x0041;
}

/// List commands.
pub mod list {
    /// Move with the beam off. Args: x, y, angle, distance.
    pub const JUMP_TO: u16 = 0x8001;
    /// End of list; also the padding of every list packet.
    pub const END_OF_LIST: u16 = 0x8002;
    /// Fire in place for a dwell time. Never sent by this driver.
    pub const LASER_ON_POINT: u16 = 0x8003;
    /// Wait, in units of 10 µs.
    pub const DELAY_TIME: u16 = 0x8004;
    /// Move with the beam on. Args: x, y, angle, distance.
    pub const MARK_TO: u16 = 0x8005;
    /// Jump speed, galvo units per ms.
    pub const JUMP_SPEED: u16 = 0x8006;
    /// Laser on delay, µs. Args: delay, sign (0x8000 = negative).
    pub const LASER_ON_DELAY: u16 = 0x8007;
    /// Laser off delay, µs. Args: delay, sign.
    pub const LASER_OFF_DELAY: u16 = 0x8008;
    /// Mark speed, galvo units per ms.
    pub const MARK_SPEED: u16 = 0x800C;
    /// Jump delay, µs. Args: delay, sign.
    pub const JUMP_DELAY: u16 = 0x800D;
    /// Polygon delay, 10 µs units.
    pub const POLYGON_DELAY: u16 = 0x800F;
    /// Set the output port bits from inside a list.
    pub const WRITE_PORT: u16 = 0x8011;
    /// Fiber power: 0..0xFFF for 0..100 %.
    pub const MARK_CURRENT: u16 = 0x8012;
    /// Q-switch period: 20000 / kHz.
    pub const QSWITCH_PERIOD: u16 = 0x801B;
    /// MOPA pulse width (YLPM).
    pub const FIBER_YLPM_PULSE_WIDTH: u16 = 0x8026;
    /// Start of every new list.
    pub const READY_MARK: u16 = 0x8051;
}

/// Status bit (reply word 3): the board is executing a list.
pub const STATUS_BUSY: u16 = 0x04;
/// Status bit: the board can take another list packet.
pub const STATUS_READY: u16 = 0x20;
/// Status bit: an axis is moving.
pub const STATUS_AXIS: u16 = 0x40;

/// Encode an opcode and up to five arguments.
pub fn command(op: u16, args: [u16; 5]) -> Command {
    let mut out = [0u8; 12];
    out[..2].copy_from_slice(&op.to_le_bytes());
    for (i, a) in args.iter().enumerate() {
        out[2 + i * 2..4 + i * 2].copy_from_slice(&a.to_le_bytes());
    }
    out
}

/// Decode a command into its opcode and arguments.
pub fn decode(c: &[u8]) -> (u16, [u16; 5]) {
    let w = |i: usize| u16::from_le_bytes([c[i * 2], c[i * 2 + 1]]);
    (w(0), [w(1), w(2), w(3), w(4), w(5)])
}

/// The padding command used to fill list packets.
pub const NOP: Command = [0x02, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// Pack list commands into 3072-byte packets, padding the last one.
pub fn list_packets(cmds: &[Command]) -> Vec<Vec<u8>> {
    cmds.chunks(LIST_LEN)
        .map(|chunk| {
            let mut p = Vec::with_capacity(LIST_PACKET_BYTES);
            for c in chunk {
                p.extend_from_slice(c);
            }
            while p.len() < LIST_PACKET_BYTES {
                p.extend_from_slice(&NOP);
            }
            p
        })
        .collect()
}

/// Reply words from an 8-byte reply.
pub fn reply_words(r: &[u8; 8]) -> [u16; 4] {
    [
        u16::from_le_bytes([r[0], r[1]]),
        u16::from_le_bytes([r[2], r[3]]),
        u16::from_le_bytes([r[4], r[5]]),
        u16::from_le_bytes([r[6], r[7]]),
    ]
}

/// Fiber power in percent to the `MARK_CURRENT` value.
pub fn power_to_current(pct: f64) -> u16 {
    (pct.clamp(0.0, 100.0) * f64::from(0xFFFu16) / 100.0).round() as u16
}

/// `MARK_CURRENT` value back to percent.
pub fn current_to_power(v: u16) -> f64 {
    f64::from(v.min(0xFFF)) * 100.0 / f64::from(0xFFFu16)
}

/// Pulse frequency in kHz to the fiber Q-switch period.
pub fn qswitch_period(freq_khz: f64) -> u16 {
    if freq_khz <= 0.0 || !freq_khz.is_finite() {
        return 0xFFFF;
    }
    (20000.0 / freq_khz).round().clamp(0.0, 65535.0) as u16
}

/// A short description of a command for the raw log.
pub fn describe(c: &[u8]) -> String {
    let (op, a) = decode(c);
    let name = match op {
        single::DISABLE_LASER => "DisableLaser",
        single::ENABLE_LASER => "EnableLaser",
        single::EXECUTE_LIST => "ExecuteList",
        single::GET_VERSION => "GetVersion",
        single::GET_SERIAL_NO => "GetSerialNo",
        single::GET_POSITION_XY => "GetPositionXY",
        single::GOTO_XY => "GotoXY",
        single::RESET_LIST => "ResetList",
        single::RESTART_LIST => "RestartList",
        single::WRITE_COR_TABLE => "WriteCorTable",
        single::SET_END_OF_LIST => "SetEndOfList",
        single::STOP_EXECUTE => "StopExecute",
        single::STOP_LIST => "StopList",
        single::WRITE_PORT => "WritePort",
        single::FIBER_SET_MO => "FiberSetMo",
        single::RESET => "Reset",
        list::JUMP_TO => "listJumpTo",
        list::MARK_TO => "listMarkTo",
        list::WRITE_PORT => "listWritePort",
        list::MARK_CURRENT => "listMarkCurrent",
        list::READY_MARK => "listReadyMark",
        list::END_OF_LIST => "listEndOfList",
        _ => "",
    };
    let mut args = a.to_vec();
    while args.last() == Some(&0) {
        args.pop();
    }
    if name.is_empty() {
        format!("{op:#06x} {args:?}")
    } else {
        format!("{name} {args:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_command_bytes() {
        // GetVersion: 07 00 then ten zero bytes.
        assert_eq!(
            command(single::GET_VERSION, [0; 5]),
            [0x07, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        // listJumpTo 0x8000, 0x8000 (centre), distance 0x1234.
        assert_eq!(
            command(list::JUMP_TO, [0x8000, 0x8000, 0, 0x1234, 0]),
            [0x01, 0x80, 0x00, 0x80, 0x00, 0x80, 0, 0, 0x34, 0x12, 0, 0]
        );
        // WritePort with only bit 8 (the usual guide light) set.
        assert_eq!(
            command(single::WRITE_PORT, [1 << 8, 0, 0, 0, 0]),
            [0x21, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(command(list::END_OF_LIST, [0; 5]), NOP);
    }

    #[test]
    fn list_packets_are_padded_to_3072_bytes() {
        let cmds = vec![command(list::READY_MARK, [0; 5]); 300];
        let packets = list_packets(&cmds);
        assert_eq!(packets.len(), 2);
        assert!(packets.iter().all(|p| p.len() == LIST_PACKET_BYTES));
        assert_eq!(&packets[1][44 * 12..45 * 12], &NOP);
    }

    #[test]
    fn unit_conversions() {
        assert_eq!(power_to_current(100.0), 0xFFF);
        assert_eq!(power_to_current(50.0), 0x800);
        assert_eq!(power_to_current(250.0), 0xFFF);
        assert_eq!(qswitch_period(30.0), 667);
        assert_eq!(qswitch_period(0.0), 0xFFFF);
        assert!((current_to_power(0x800) - 50.0).abs() < 0.05);
    }

    #[test]
    fn decode_round_trips() {
        let c = command(list::MARK_TO, [1, 2, 3, 4, 5]);
        assert_eq!(decode(&c), (list::MARK_TO, [1, 2, 3, 4, 5]));
    }
}
