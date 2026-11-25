// SPDX-License-Identifier: AGPL-3.0-or-later
//! Ruida controller driver.
//!
//! Port 50200 carries scrambled `.rd` command packets, each prefixed with a
//! 16-bit checksum and answered with an ACK. Port 50207 is a simpler jog and
//! control channel. The scramble key ("magic") varies by board model.
//!
//! Protocol knowledge comes from MeerK40t (MIT) and public notes; see
//! `docs/protocols.md`.

pub mod protocol;

use libreburn_core::{
    Driver, DriverStatus, Error, ErrorCode, FrameRequest, Job, MachineProfile, MachineState,
    RawExchange, Result,
};

/// Moves bytes to and from the controller. Real UDP and serial transports
/// implement this; [`MockTransport`] replays fixtures in tests (NF-7).
pub trait Transport: Send {
    /// Send one packet and return the reply.
    fn exchange(&mut self, packet: &[u8]) -> Result<Vec<u8>>;
}

/// A transport that ACKs every packet and records what it was sent.
#[derive(Debug, Default)]
pub struct MockTransport {
    /// Every packet sent, in order.
    pub sent: Vec<Vec<u8>>,
}

impl Transport for MockTransport {
    fn exchange(&mut self, packet: &[u8]) -> Result<Vec<u8>> {
        self.sent.push(packet.to_vec());
        Ok(vec![protocol::ACK])
    }
}

/// The Ruida driver.
#[derive(Debug)]
pub struct RuidaDriver<T: Transport> {
    profile: MachineProfile,
    transport: T,
    magic: u8,
    state: MachineState,
    log: Vec<RawExchange>,
}

impl<T: Transport> RuidaDriver<T> {
    /// Create a driver. `magic` is the board's scramble key (0x88 on RDC644x boards).
    pub fn new(profile: MachineProfile, transport: T, magic: u8) -> Self {
        Self {
            profile,
            transport,
            magic,
            state: MachineState::Disconnected,
            log: Vec::new(),
        }
    }

    /// The underlying transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    fn send(&mut self, payload: &[u8], decoded: &str) -> Result<Vec<u8>> {
        let packet = protocol::encode_packet(payload, self.magic);
        let reply = self.transport.exchange(&packet)?;
        self.log.push(RawExchange {
            sent: packet,
            received: reply.clone(),
            decoded: decoded.to_owned(),
        });
        Ok(reply)
    }

    fn not_yet(what: &str) -> Error {
        Error::new(
            ErrorCode::Unsupported,
            format!("Ruida {what}"),
            "not implemented in this early build",
            "track progress in the repository issues",
        )
    }
}

impl<T: Transport> Driver for RuidaDriver<T> {
    fn profile(&self) -> &MachineProfile {
        &self.profile
    }

    fn connect(&mut self) -> Result<()> {
        self.send(&protocol::CMD_PING, "ping")?;
        self.state = MachineState::Idle;
        Ok(())
    }

    fn status(&mut self) -> Result<DriverStatus> {
        Ok(DriverStatus {
            state: self.state,
            position: None,
        })
    }

    fn read_param(&mut self, name: &str) -> Result<Vec<u8>> {
        Err(Self::not_yet(&format!("parameter read ({name})")))
    }

    fn write_param(&mut self, name: &str, _value: &[u8]) -> Result<()> {
        Err(Self::not_yet(&format!("parameter write ({name})")))
    }

    fn run_job(&mut self, _job: &Job) -> Result<()> {
        Err(Self::not_yet("job streaming"))
    }

    fn jog(&mut self, _dx: f64, _dy: f64) -> Result<()> {
        Err(Self::not_yet("jog"))
    }

    fn trace_frame(&mut self, _request: &FrameRequest) -> Result<()> {
        Err(Self::not_yet("framing"))
    }

    fn pause(&mut self) -> Result<()> {
        Err(Self::not_yet("pause"))
    }

    fn resume(&mut self) -> Result<()> {
        Err(Self::not_yet("resume"))
    }

    fn stop(&mut self) -> Result<()> {
        self.send(&protocol::CMD_STOP, "stop")?;
        self.state = MachineState::Idle;
        Ok(())
    }

    fn raw_log(&self) -> &[RawExchange] {
        &self.log
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> MachineProfile {
        MachineProfile::from_toml(include_str!("../../../profiles/omtech-k40-ruida.toml")).unwrap()
    }

    #[test]
    fn connect_sends_one_scrambled_ping() {
        let mut d = RuidaDriver::new(profile(), MockTransport::default(), protocol::MAGIC_644X);
        d.connect().unwrap();
        assert_eq!(d.status().unwrap().state, MachineState::Idle);
        let sent = &d.transport().sent;
        assert_eq!(sent.len(), 1);
        let payload = protocol::decode_packet(&sent[0], protocol::MAGIC_644X).unwrap();
        assert_eq!(payload, protocol::CMD_PING);
        assert_eq!(d.raw_log().len(), 1);
    }
}

/// Hardware-in-the-loop tests. Run with `hil/run.sh`, never in normal CI.
#[cfg(test)]
mod hil {
    use libreburn_core::MachineProfile;

    fn hil_profile() -> MachineProfile {
        let path = std::env::var("LIBREBURN_HIL_PROFILE").expect("set LIBREBURN_HIL_PROFILE");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path);
        let p = MachineProfile::from_toml(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(
            p.machine.hil,
            "refusing to run hardware tests with a non-HIL profile"
        );
        assert_eq!(
            p.limits.max_power_pct, 0.0,
            "HIL profiles must cap power at zero"
        );
        p
    }

    #[test]
    #[ignore = "needs hardware; run with hil/run.sh"]
    fn hil_profile_is_safe() {
        hil_profile();
    }
}
