// SPDX-License-Identifier: AGPL-3.0-or-later
//! A software LMC board for tests (NF-7).
//!
//! [`LmcSim`] takes the same 12-byte commands and 3072-byte list packets a
//! real board would, keeps the output port bits and the fiber master
//! oscillator (MO) state, runs lists when told to, and records:
//! - every segment where the fiber source would fire (`MARK_TO` with the MO
//!   open and the laser-enable bit set), with its power;
//! - every mark attempt at all, whatever the enable state;
//! - every jump traced with the guide light on.
//!
//! Tests assert on those records rather than on encoder internals.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use libreburn_core::{Error, ErrorCode, Result};

use crate::protocol::{
    self, LIST_LEN, LIST_PACKET_BYTES, STATUS_BUSY, STATUS_READY, current_to_power, list, single,
};

/// One stretch of travel with the fiber source firing.
#[derive(Debug, Clone, PartialEq)]
pub struct Burn {
    /// Start, galvo units.
    pub from: (u16, u16),
    /// End, galvo units.
    pub to: (u16, u16),
    /// Power, percent.
    pub power_pct: f64,
}

/// What the simulated board has seen.
#[derive(Debug, Clone, PartialEq)]
pub struct LmcSim {
    /// Which output bit enables the fiber source.
    pub laser_enable_bit: u8,
    /// Which output bit drives the guide light.
    pub guide_light_bit: u8,
    /// Current output port bits.
    pub port: u16,
    /// Every value written to the port, single or list.
    pub port_history: Vec<u16>,
    /// The master oscillator is open.
    pub mo_open: bool,
    /// `MO` was ever opened.
    pub mo_ever_opened: bool,
    /// `ENABLE_LASER` was received (and not followed by `DISABLE_LASER`).
    pub laser_enabled: bool,
    /// Beam position, galvo units.
    pub pos: (u16, u16),
    /// Fiber power from `MARK_CURRENT`, percent.
    pub power_pct: f64,
    /// A list is running.
    pub executing: bool,
    /// The list is paused.
    pub paused: bool,
    /// Segments where the fiber source fired.
    pub burns: Vec<Burn>,
    /// Every `MARK_TO`/`LASER_ON_POINT`, fired or not.
    pub mark_attempts: usize,
    /// `LASER_SIGNAL_ON` was ever sent.
    pub laser_signal_on: bool,
    /// Jumps made with the guide light on.
    pub light_trace: Vec<((u16, u16), (u16, u16))>,
    /// Every single command received.
    pub singles: Vec<protocol::Command>,
    /// Every list command executed.
    pub executed: Vec<protocol::Command>,
    /// List packets received.
    pub list_packets: usize,
    /// Problems found in what was sent.
    pub errors: Vec<String>,
    queue: Vec<protocol::Command>,
}

impl LmcSim {
    /// A board with the given enable and guide-light bits.
    pub fn new(laser_enable_bit: u8, guide_light_bit: u8) -> Self {
        Self {
            laser_enable_bit,
            guide_light_bit,
            port: 0,
            port_history: Vec::new(),
            mo_open: false,
            mo_ever_opened: false,
            laser_enabled: false,
            pos: (0x8000, 0x8000),
            power_pct: 0.0,
            executing: false,
            paused: false,
            burns: Vec::new(),
            mark_attempts: 0,
            laser_signal_on: false,
            light_trace: Vec::new(),
            singles: Vec::new(),
            executed: Vec::new(),
            list_packets: 0,
            errors: Vec::new(),
            queue: Vec::new(),
        }
    }

    /// Highest power any burn used.
    pub fn max_burn_power(&self) -> f64 {
        self.burns.iter().fold(0.0, |m, b| m.max(b.power_pct))
    }

    fn set_port(&mut self, v: u16) {
        self.port = v;
        self.port_history.push(v);
    }

    fn fires(&self) -> bool {
        self.mo_open && self.laser_enabled && self.port & (1 << self.laser_enable_bit) != 0
    }

    fn light_on(&self) -> bool {
        self.port & (1 << self.guide_light_bit) != 0
    }

    fn run(&mut self, c: protocol::Command) {
        let (op, a) = protocol::decode(&c);
        self.executed.push(c);
        match op {
            list::JUMP_TO => {
                let to = (a[0], a[1]);
                if self.light_on() {
                    self.light_trace.push((self.pos, to));
                }
                self.pos = to;
            }
            list::MARK_TO => {
                self.mark_attempts += 1;
                let to = (a[0], a[1]);
                if self.fires() {
                    self.burns.push(Burn {
                        from: self.pos,
                        to,
                        power_pct: self.power_pct,
                    });
                }
                self.pos = to;
            }
            list::LASER_ON_POINT => self.mark_attempts += 1,
            list::WRITE_PORT => self.set_port(a[0]),
            list::MARK_CURRENT => self.power_pct = current_to_power(a[0]),
            list::END_OF_LIST
            | list::READY_MARK
            | list::DELAY_TIME
            | list::JUMP_SPEED
            | list::MARK_SPEED
            | list::LASER_ON_DELAY
            | list::LASER_OFF_DELAY
            | list::JUMP_DELAY
            | list::POLYGON_DELAY
            | list::QSWITCH_PERIOD
            | list::FIBER_YLPM_PULSE_WIDTH => {}
            _ => self.errors.push(format!("unknown list command {op:#06x}")),
        }
    }

    fn drain(&mut self) {
        if self.executing && !self.paused {
            let q = std::mem::take(&mut self.queue);
            for c in q {
                self.run(c);
            }
        }
    }

    fn status(&self) -> u16 {
        let busy = self.executing && (self.paused || !self.queue.is_empty());
        STATUS_READY | if busy { STATUS_BUSY } else { 0 }
    }

    /// Feed one USB bulk write. Returns the 8-byte reply for single commands.
    pub fn write(&mut self, packet: &[u8]) -> Option<[u8; 8]> {
        match packet.len() {
            12 => {
                let c: protocol::Command = packet.try_into().expect("12 bytes");
                self.singles.push(c);
                let (op, a) = protocol::decode(&c);
                // Correction lines are sent without reading a reply.
                if op == single::WRITE_COR_LINE {
                    return None;
                }
                let mut reply = [0u16; 4];
                match op {
                    single::ENABLE_LASER => self.laser_enabled = true,
                    single::DISABLE_LASER => self.laser_enabled = false,
                    single::FIBER_SET_MO => {
                        self.mo_open = a[0] != 0;
                        self.mo_ever_opened |= self.mo_open;
                    }
                    single::WRITE_PORT => self.set_port(a[0]),
                    single::RESET_LIST => {
                        self.queue.clear();
                        self.executing = false;
                    }
                    single::EXECUTE_LIST => {
                        self.executing = true;
                        self.paused = false;
                    }
                    single::STOP_EXECUTE => {
                        self.queue.clear();
                        self.executing = false;
                        self.paused = false;
                    }
                    single::STOP_LIST => self.paused = true,
                    single::RESTART_LIST => self.paused = false,
                    single::GOTO_XY => self.pos = (a[0], a[1]),
                    single::LASER_SIGNAL_ON => self.laser_signal_on = true,
                    single::GET_SERIAL_NO => reply = [0x4C42, 0x5349, 0x4D00, 0],
                    single::GET_POSITION_XY => reply = [0, self.pos.0, self.pos.1, 0],
                    single::GET_VERSION
                    | single::GET_LIST_STATUS
                    | single::LASER_SIGNAL_OFF
                    | single::WRITE_COR_TABLE
                    | single::SET_CONTROL_MODE
                    | single::SET_DELAY_MODE
                    | single::SET_END_OF_LIST
                    | single::SET_FIRST_PULSE_KILLER
                    | single::SET_LASER_MODE
                    | single::SET_TIMING
                    | single::SET_STANDBY
                    | single::SET_PWM_HALF_PERIOD
                    | single::SET_PWM_PULSE_WIDTH
                    | single::WRITE_ANALOG_PORT_1
                    | single::READ_PORT
                    | single::SET_FPK_PARAM_2
                    | single::SET_FLY_RES
                    | single::ENABLE_Z
                    | single::DISABLE_Z
                    | single::RESET
                    | single::GET_MARK_TIME => {}
                    _ => self
                        .errors
                        .push(format!("unknown single command {op:#06x}")),
                }
                self.drain();
                reply[3] |= self.status();
                let mut out = [0u8; 8];
                for (i, w) in reply.iter().enumerate() {
                    out[i * 2..i * 2 + 2].copy_from_slice(&w.to_le_bytes());
                }
                Some(out)
            }
            LIST_PACKET_BYTES => {
                self.list_packets += 1;
                for c in packet.chunks_exact(12).take(LIST_LEN) {
                    let c: protocol::Command = c.try_into().expect("12 bytes");
                    if protocol::decode(&c).0 < 0x8000 {
                        self.errors
                            .push(format!("single command {c:02x?} inside a list packet"));
                    }
                    self.queue.push(c);
                }
                self.drain();
                None
            }
            n => {
                self.errors.push(format!("bad packet length {n}"));
                None
            }
        }
    }
}

/// A [`Transport`](crate::Transport) wired straight into an [`LmcSim`].
#[derive(Debug, Clone)]
pub struct SimTransport {
    /// The simulator, shared so tests can inspect it after handing the
    /// transport to a driver.
    pub sim: Arc<Mutex<LmcSim>>,
    pending: VecDeque<[u8; 8]>,
}

impl SimTransport {
    /// Wrap a simulator.
    pub fn new(sim: LmcSim) -> Self {
        Self {
            sim: Arc::new(Mutex::new(sim)),
            pending: VecDeque::new(),
        }
    }
}

impl crate::Transport for SimTransport {
    fn write(&mut self, packet: &[u8]) -> Result<()> {
        if let Some(r) = self.sim.lock().unwrap().write(packet) {
            self.pending.push_back(r);
        }
        Ok(())
    }

    fn read(&mut self) -> Result<[u8; 8]> {
        self.pending.pop_front().ok_or_else(|| {
            Error::new(
                ErrorCode::DeviceUnreachable,
                "The simulated LMC board sent no reply",
                "a read was made without a single command before it",
                "this is a driver bug",
            )
        })
    }
}
