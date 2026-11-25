// SPDX-License-Identifier: AGPL-3.0-or-later
//! Links to a Ruida controller and the session rules on top of them.
//!
//! - **UDP** (Ethernet): each packet is a big-endian 16-bit checksum followed
//!   by scrambled commands, sent to port 50200. The controller answers every
//!   packet with a one-byte ACK or NAK (scrambled) and, for memory reads, a
//!   second packet with the value.
//! - **USB serial**: the same scrambled commands with no checksum and no ACK.
//!   Memory reads are answered with a 9-byte reply.
//!
//! The session behaviour follows MeerK40t's `ruida/ruidasession.py`
//! (Copyright (c) 2021 meerk40t, MIT License).

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use libreburn_core::{Error, ErrorCode, RawExchange, Result};

use crate::codec;
use crate::protocol::{self, UDP_COMMAND_PORT, UDP_REPLY_PORT};

/// The kind of link, which decides checksum and ACK handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// Ethernet, UDP ports 50200 / 40200.
    Udp,
    /// USB serial (FTDI), no checksum and no ACK.
    Serial,
}

/// Moves raw bytes to and from the controller.
pub trait Transport: Send {
    /// Which link this is.
    fn link(&self) -> Link;
    /// Send bytes exactly as given.
    fn send(&mut self, bytes: &[u8]) -> Result<()>;
    /// Wait up to `timeout` for the next datagram (UDP) or available bytes
    /// (serial). `Ok(None)` means nothing arrived.
    fn recv(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>>;
}

pub(crate) fn unreachable(why: impl Into<String>) -> Error {
    Error::new(
        ErrorCode::DeviceUnreachable,
        "The Ruida controller did not answer",
        why,
        "check the cable or network, the controller's IP address and that it is powered on",
    )
}

fn violation(why: impl Into<String>) -> Error {
    Error::new(
        ErrorCode::ProtocolViolation,
        "The Ruida controller sent an unexpected reply",
        why,
        "check the scramble key (connection.magic) in the profile and record a trace for a bug report",
    )
}

/// Length of a memory-read reply: `DA 01`, address(2), value(5).
pub const MEM_REPLY_LEN: usize = 9;

/// Session state on top of a [`Transport`]: scrambling, checksums, ACKs,
/// retries and the raw exchange log.
#[derive(Debug)]
pub struct Session<T: Transport> {
    transport: T,
    magic: u8,
    /// Wait this long for each ACK or reply.
    pub timeout: Duration,
    /// Resend a packet this many times after a NAK or a timeout.
    pub retries: u32,
    log: Vec<RawExchange>,
}

/// How many raw exchanges the session keeps.
const LOG_LIMIT: usize = 512;

impl<T: Transport> Session<T> {
    /// A session with the board's scramble key.
    pub fn new(transport: T, magic: u8) -> Self {
        Self {
            transport,
            magic,
            timeout: Duration::from_millis(250),
            retries: 4,
            log: Vec::new(),
        }
    }

    /// The transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The transport, mutably.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Recent exchanges, oldest first.
    pub fn log(&self) -> &[RawExchange] {
        &self.log
    }

    fn record(&mut self, sent: Vec<u8>, received: Vec<u8>, decoded: String) {
        if self.log.len() == LOG_LIMIT {
            self.log.remove(0);
        }
        self.log.push(RawExchange {
            sent,
            received,
            decoded,
        });
    }

    /// Send one packet of commands (unscrambled payload) and wait for it to
    /// be accepted. Returns the unscrambled reply for memory reads.
    pub fn send(&mut self, payload: &[u8], decoded: &str) -> Result<Option<Vec<u8>>> {
        let expects_reply = payload.starts_with(&codec::GET_SETTING);
        match self.transport.link() {
            Link::Udp => self.send_udp(payload, decoded, expects_reply),
            Link::Serial => self.send_serial(payload, decoded, expects_reply),
        }
    }

    fn send_udp(
        &mut self,
        payload: &[u8],
        decoded: &str,
        expects_reply: bool,
    ) -> Result<Option<Vec<u8>>> {
        let packet = protocol::encode_packet(payload, self.magic);
        let mut received = Vec::new();
        let mut attempts = 0;
        self.transport.send(&packet)?;
        loop {
            let Some(bytes) = self.transport.recv(self.timeout)? else {
                attempts += 1;
                if attempts > self.retries {
                    self.record(packet, received, format!("{decoded} (no ACK)"));
                    return Err(unreachable(format!(
                        "no ACK after {attempts} tries ({decoded})"
                    )));
                }
                self.transport.send(&packet)?;
                continue;
            };
            received.extend_from_slice(&bytes);
            let plain = protocol::unscramble_all(&bytes, self.magic);
            match plain.as_slice() {
                [codec::ACK] => break,
                [codec::NAK] => {
                    attempts += 1;
                    if attempts > self.retries {
                        self.record(packet, received, format!("{decoded} (NAK)"));
                        return Err(violation(format!(
                            "the controller rejected the packet {attempts} times ({decoded})"
                        )));
                    }
                    self.transport.send(&packet)?;
                }
                [codec::ERR] => {
                    self.record(packet, received, format!("{decoded} (error)"));
                    return Err(violation(format!(
                        "the controller replied ERR to {decoded}"
                    )));
                }
                // Stray keep-alives and late replies are ignored.
                _ => {}
            }
        }
        let reply = if expects_reply {
            match self.transport.recv(self.timeout)? {
                Some(bytes) => {
                    received.extend_from_slice(&bytes);
                    Some(protocol::unscramble_all(&bytes, self.magic))
                }
                None => {
                    self.record(packet, received, format!("{decoded} (no reply)"));
                    return Err(unreachable(format!("no reply to {decoded}")));
                }
            }
        } else {
            None
        };
        self.record(packet, received, decoded.to_owned());
        Ok(reply)
    }

    fn send_serial(
        &mut self,
        payload: &[u8],
        decoded: &str,
        expects_reply: bool,
    ) -> Result<Option<Vec<u8>>> {
        let packet = protocol::scramble_all(payload, self.magic);
        self.transport.send(&packet)?;
        if !expects_reply {
            self.record(packet, Vec::new(), decoded.to_owned());
            return Ok(None);
        }
        let mut received = Vec::new();
        let mut waits = 0;
        while received.len() < MEM_REPLY_LEN {
            match self.transport.recv(self.timeout)? {
                Some(bytes) => received.extend_from_slice(&bytes),
                None => {
                    waits += 1;
                    if waits > self.retries {
                        self.record(packet, received, format!("{decoded} (no reply)"));
                        return Err(unreachable(format!("no reply to {decoded}")));
                    }
                }
            }
        }
        let plain = protocol::unscramble_all(&received[..MEM_REPLY_LEN], self.magic);
        self.record(packet, received, decoded.to_owned());
        Ok(Some(plain))
    }

    /// Read a controller memory word.
    pub fn read_mem(&mut self, addr: u16) -> Result<u32> {
        let req = codec::cmd(&[&codec::GET_SETTING, &codec::encode_mem(addr)]);
        let reply = self
            .send(&req, &format!("read mem {addr:#06x}"))?
            .unwrap_or_default();
        if reply.len() < MEM_REPLY_LEN
            || reply[..2] != codec::SET_SETTING
            || codec::decode_mem(&reply[2..4]) != addr
        {
            return Err(violation(format!(
                "expected a reply for mem {addr:#06x}, got {reply:02x?}"
            )));
        }
        Ok((codec::decode_u35(&reply[4..9]) & 0xFFFF_FFFF) as u32)
    }
}

/// Ethernet link over UDP.
#[derive(Debug)]
pub struct UdpTransport {
    socket: UdpSocket,
    target: SocketAddr,
}

impl UdpTransport {
    /// Connect to a controller at `host` on the standard ports. Binds the
    /// reply port 40200, which the controller always answers to.
    pub fn connect(host: &str) -> Result<Self> {
        Self::with_ports(host, UDP_COMMAND_PORT, UDP_REPLY_PORT)
    }

    /// Connect with explicit ports (tests and simulators).
    pub fn with_ports(host: &str, command_port: u16, bind_port: u16) -> Result<Self> {
        let target = (host, command_port)
            .to_socket_addrs()
            .map_err(|e| unreachable(format!("cannot resolve {host}: {e}")))?
            .next()
            .ok_or_else(|| unreachable(format!("{host} has no address")))?;
        let bind: SocketAddr = if target.is_ipv4() {
            ([0, 0, 0, 0], bind_port).into()
        } else {
            (std::net::Ipv6Addr::UNSPECIFIED, bind_port).into()
        };
        let socket = UdpSocket::bind(bind).map_err(|e| {
            unreachable(format!(
                "cannot listen on UDP port {bind_port}: {e} (is another laser program running?)"
            ))
        })?;
        Ok(Self { socket, target })
    }
}

impl Transport for UdpTransport {
    fn link(&self) -> Link {
        Link::Udp
    }

    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.socket
            .send_to(bytes, self.target)
            .map_err(|e| unreachable(format!("UDP send to {} failed: {e}", self.target)))?;
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        self.socket
            .set_read_timeout(Some(timeout.max(Duration::from_millis(1))))
            .map_err(|e| unreachable(e.to_string()))?;
        let mut buf = [0u8; 2048];
        loop {
            match self.socket.recv_from(&mut buf) {
                // Only accept datagrams from the controller.
                Ok((n, from)) if from.ip() == self.target.ip() => {
                    return Ok(Some(buf[..n].to_vec()));
                }
                Ok(_) => continue,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(unreachable(format!("UDP receive failed: {e}"))),
            }
        }
    }
}
