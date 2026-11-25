// SPDX-License-Identifier: AGPL-3.0-or-later
//! A software Ruida controller for tests (NF-7).
//!
//! [`RuidaSim`] parses the same bytes a real board would receive, checks
//! checksums, argument lengths and the job file sum, tracks head position, and
//! records every segment where the laser would fire and at what power. Tests
//! assert on that record rather than on encoder internals.
//!
//! [`SimTransport`] plugs it straight into a driver; [`spawn_udp`] serves it
//! on a localhost UDP socket so the real [`UdpTransport`](crate::UdpTransport)
//! is exercised too.

use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use libreburn_core::Result;

use crate::codec::{self, decode_power, decode_u35, decode14, decode32, mem};
use crate::protocol;
use crate::transport::{Link, Transport};

/// One stretch of travel with the laser firing.
#[derive(Debug, Clone, PartialEq)]
pub struct Burn {
    /// Start, µm.
    pub from: (i32, i32),
    /// End, µm.
    pub to: (i32, i32),
    /// Power, percent.
    pub power_pct: f64,
}

/// What the simulated board has seen.
#[derive(Debug, Clone, PartialEq)]
pub struct RuidaSim {
    /// Scramble key.
    pub magic: u8,
    /// Link behaviour (checksums and ACKs only on UDP).
    pub link: Link,
    /// Head position, µm.
    pub pos: (i32, i32),
    /// Bed size, µm.
    pub bed: (i32, i32),
    /// A job is running.
    pub running: bool,
    /// The job is paused.
    pub paused: bool,
    /// Power of the current layer, percent (max of min/max power settings).
    pub power_pct: f64,
    /// Every laser-on segment.
    pub burns: Vec<Burn>,
    /// Every rapid or laser-off move target.
    pub travels: Vec<(i32, i32)>,
    /// Every command received, unscrambled.
    pub commands: Vec<Vec<u8>>,
    /// Commands the simulator does not know, or with the wrong length.
    pub errors: Vec<String>,
    /// Jobs that ended with a correct file sum.
    pub completed_jobs: usize,
    /// Memory words written with `DA 01`.
    pub written: HashMap<u16, u32>,
    /// Packets rejected for a bad checksum.
    pub naks: usize,
    /// Fault injection: drop the reply to the next N packets.
    pub drop_next: usize,
    /// Fault injection: NAK the next N packets even if the checksum is good.
    pub nak_next: usize,
    file_sum: u64,
    in_file: bool,
}

/// The card ID the simulator reports (an RDC6442S, per MeerK40t).
pub const SIM_CARD_ID: u32 = 0x6510_6510;

impl RuidaSim {
    /// A simulator for a bed of `bed_mm`.
    pub fn new(magic: u8, link: Link, bed_mm: (f64, f64)) -> Self {
        Self {
            magic,
            link,
            pos: (0, 0),
            bed: (
                (bed_mm.0 * 1000.0).round() as i32,
                (bed_mm.1 * 1000.0).round() as i32,
            ),
            running: false,
            paused: false,
            power_pct: 0.0,
            burns: Vec::new(),
            travels: Vec::new(),
            commands: Vec::new(),
            errors: Vec::new(),
            completed_jobs: 0,
            written: HashMap::new(),
            naks: 0,
            drop_next: 0,
            nak_next: 0,
            file_sum: 0,
            in_file: false,
        }
    }

    /// Highest power any burn used.
    pub fn max_burn_power(&self) -> f64 {
        self.burns.iter().fold(0.0, |m, b| m.max(b.power_pct))
    }

    /// Feed one packet as it arrives on the wire. Returns packets to send back.
    pub fn handle(&mut self, packet: &[u8]) -> Vec<Vec<u8>> {
        let payload = match self.link {
            Link::Udp => {
                if self.nak_next > 0 {
                    self.nak_next -= 1;
                    self.naks += 1;
                    return vec![vec![protocol::scramble(codec::NAK, self.magic)]];
                }
                match protocol::decode_packet(packet, self.magic) {
                    Some(p) => p,
                    None => {
                        self.naks += 1;
                        return vec![vec![protocol::scramble(codec::NAK, self.magic)]];
                    }
                }
            }
            Link::Serial => protocol::unscramble_all(packet, self.magic),
        };
        let mut replies = Vec::new();
        if self.link == Link::Udp {
            replies.push(vec![protocol::scramble(codec::ACK, self.magic)]);
        }
        for c in codec::split_commands(&payload) {
            if let Some(r) = self.execute(c) {
                replies.push(protocol::scramble_all(&r, self.magic));
            }
        }
        if self.drop_next > 0 {
            self.drop_next -= 1;
            return Vec::new();
        }
        replies
    }

    fn expected_len(c: &[u8]) -> Option<usize> {
        Some(match c {
            [0x88 | 0xA8, ..] => 11,
            [0x89 | 0xA9, ..] => 5,
            [0x8A | 0x8B | 0xAA | 0xAB, ..] => 3,
            [0xC7 | 0xC8, ..] => 3,
            [0xC6, 0x01 | 0x02 | 0x21 | 0x22, ..] => 4,
            [0xC6, 0x12 | 0x13, ..] => 7,
            [0xC6, 0x31 | 0x32 | 0x41 | 0x42, ..] => 5,
            [0xC9, 0x02, ..] => 7,
            [0xC9, 0x04, ..] => 8,
            [0xCA, 0x01, ..] => 3,
            [0xCA, 0x02 | 0x03 | 0x22, ..] => 3,
            [0xCA, 0x06, ..] => 8,
            [0xCA, 0x41, ..] => 4,
            [0xCC | 0xCD | 0xCE | 0xCF | 0xD7 | 0xF0] => 1,
            [0xD8, ..] => 2,
            [0xD9, 0x10, ..] => 13,
            [0xDA, 0x00, ..] => 4,
            [0xDA, 0x01, ..] => 14,
            [0xE5, 0x05, ..] => 7,
            [0xE6, 0x01, ..] => 2,
            [0xE7, 0x00, ..] => 2,
            [0xE7, 0x03 | 0x07 | 0x50 | 0x51, ..] => 12,
            [0xE7, 0x52 | 0x53, ..] => 13,
            [0xF1, 0x02, ..] => 3,
            _ => return None,
        })
    }

    fn go(&mut self, to: (i32, i32), laser: bool) {
        if laser {
            self.burns.push(Burn {
                from: self.pos,
                to,
                power_pct: self.power_pct,
            });
        } else {
            self.travels.push(to);
        }
        self.pos = to;
    }

    fn mem_value(&self, addr: u16) -> u32 {
        match addr {
            mem::MACHINE_STATUS => u32::from(self.running) * mem::STATUS_JOB_RUNNING,
            mem::CURRENT_X => self.pos.0 as u32,
            mem::CURRENT_Y => self.pos.1 as u32,
            mem::BED_SIZE_X => self.bed.0 as u32,
            mem::BED_SIZE_Y => self.bed.1 as u32,
            mem::CARD_ID => SIM_CARD_ID,
            a => self.written.get(&a).copied().unwrap_or(0),
        }
    }

    /// Run one unscrambled command. Returns an unscrambled reply, if any.
    pub fn execute(&mut self, c: &[u8]) -> Option<Vec<u8>> {
        self.commands.push(c.to_vec());
        match Self::expected_len(c) {
            None => {
                self.errors.push(format!("unknown command {c:02x?}"));
                return None;
            }
            Some(n) if n != c.len() => {
                self.errors.push(format!(
                    "command {c:02x?} has {} bytes, expected {n}",
                    c.len()
                ));
                return None;
            }
            Some(_) => {}
        }
        // File sum: every byte from the start of a file up to SET_FILE_SUM.
        if c == codec::REF_POINT_2 {
            self.in_file = true;
            self.file_sum = 0;
        }
        if self.in_file && c[..2.min(c.len())] != codec::SET_FILE_SUM {
            self.file_sum += c.iter().map(|&b| u64::from(b)).sum::<u64>();
        }
        let p = self.pos;
        match c {
            [0x88, ..] => self.go((decode32(&c[1..6]), decode32(&c[6..11])), false),
            [0xA8, ..] => self.go((decode32(&c[1..6]), decode32(&c[6..11])), true),
            [0x89, ..] => self.go((p.0 + decode14(&c[1..3]), p.1 + decode14(&c[3..5])), false),
            [0xA9, ..] => self.go((p.0 + decode14(&c[1..3]), p.1 + decode14(&c[3..5])), true),
            [0x8A, ..] => self.go((p.0 + decode14(&c[1..3]), p.1), false),
            [0xAA, ..] => self.go((p.0 + decode14(&c[1..3]), p.1), true),
            [0x8B, ..] => self.go((p.0, p.1 + decode14(&c[1..3])), false),
            [0xAB, ..] => self.go((p.0, p.1 + decode14(&c[1..3])), true),
            [0xC6, 0x01 | 0x02, ..] => {
                // Track the larger of min and max power for laser 1.
                let v = decode_power(&c[2..4]);
                if c[1] == 0x01 {
                    self.power_pct = v;
                } else {
                    self.power_pct = self.power_pct.max(v);
                }
            }
            [0xD8, 0x00] => self.running = true,
            [0xD8, 0x01] => {
                self.running = false;
                self.paused = false;
                self.in_file = false;
            }
            [0xD8, 0x02] => self.paused = true,
            [0xD8, 0x03] => self.paused = false,
            [0xD9, 0x10, ..] => self.go((decode32(&c[3..8]), decode32(&c[8..13])), false),
            [0xDA, 0x00, a, b] => {
                let addr = codec::decode_mem(&[*a, *b]);
                let v = self.mem_value(addr);
                return Some(codec::cmd(&[
                    &codec::SET_SETTING,
                    &[*a, *b],
                    &codec::encode35(i64::from(v)),
                ]));
            }
            [0xDA, 0x01, a, b, ..] => {
                let addr = codec::decode_mem(&[*a, *b]);
                self.written
                    .insert(addr, (decode_u35(&c[4..9]) & 0xFFFF_FFFF) as u32);
            }
            [0xE5, 0x05, ..] => {
                let expected = decode_u35(&c[2..7]);
                let actual = self.file_sum + u64::from(codec::END_OF_FILE);
                if expected != actual {
                    self.errors
                        .push(format!("file sum {expected} does not match {actual}"));
                }
            }
            [0xD7] => {
                if self.in_file {
                    self.completed_jobs += 1;
                }
                self.in_file = false;
                self.running = false;
                self.power_pct = 0.0;
            }
            _ => {}
        }
        None
    }
}

/// A [`Transport`] wired straight into a [`RuidaSim`].
#[derive(Debug, Clone)]
pub struct SimTransport {
    /// The simulator, shared so tests can inspect it after handing the
    /// transport to a driver.
    pub sim: Arc<Mutex<RuidaSim>>,
    pending: VecDeque<Vec<u8>>,
}

impl SimTransport {
    /// Wrap a simulator.
    pub fn new(sim: RuidaSim) -> Self {
        Self {
            sim: Arc::new(Mutex::new(sim)),
            pending: VecDeque::new(),
        }
    }
}

impl Transport for SimTransport {
    fn link(&self) -> Link {
        self.sim.lock().unwrap().link
    }

    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let replies = self.sim.lock().unwrap().handle(bytes);
        self.pending.extend(replies);
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>> {
        Ok(self.pending.pop_front())
    }
}

/// Serve a UDP simulator on `127.0.0.1` with an OS-chosen port.
///
/// Returns the command address and a handle; the thread stops after
/// `idle` with no traffic.
pub fn spawn_udp(
    sim: Arc<Mutex<RuidaSim>>,
    idle: Duration,
) -> std::io::Result<(SocketAddr, JoinHandle<()>)> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.set_read_timeout(Some(idle))?;
    let addr = socket.local_addr()?;
    let handle = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok((n, from)) = socket.recv_from(&mut buf) {
            let replies = sim.lock().unwrap().handle(&buf[..n]);
            for r in replies {
                let _ = socket.send_to(&r, from);
            }
        }
    });
    Ok((addr, handle))
}
