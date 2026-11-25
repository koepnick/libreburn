// SPDX-License-Identifier: AGPL-3.0-or-later
//! JCZ EZCAD2 LMC galvo driver (USB).
//!
//! Protocol knowledge comes from the balor project, galvoplotter and MeerK40t
//! (all MIT); see `docs/protocols.md`.
//!
//! Safety: framing and red-light preview use the guide light. This driver
//! refuses to fire the fiber source for framing, even after the session
//! opt-in, until low-power framing has been implemented and tested.

use libreburn_core::{
    Driver, DriverStatus, Error, ErrorCode, FrameRequest, FramingSource, Job, MachineProfile,
    MachineState, RawExchange, Result,
};

/// Moves 12-byte LMC command packets over USB. [`MockTransport`] is used in tests.
pub trait Transport: Send {
    /// Send one command and return the controller's reply.
    fn exchange(&mut self, command: &[u8; 12]) -> Result<Vec<u8>>;
}

/// A transport that records commands and replies with an idle status.
#[derive(Debug, Default)]
pub struct MockTransport {
    /// Every command sent, in order.
    pub sent: Vec<[u8; 12]>,
}

impl Transport for MockTransport {
    fn exchange(&mut self, command: &[u8; 12]) -> Result<Vec<u8>> {
        self.sent.push(*command);
        Ok(vec![0; 8])
    }
}

/// LMC opcodes used so far. Placeholders until checked against recorded fixtures.
pub mod opcode {
    /// Read the board version.
    pub const GET_VERSION: u16 = 0x0007;
    /// Set the output port mask (drives the guide light).
    pub const WRITE_PORT: u16 = 0x0021;
}

fn command(op: u16, args: [u16; 5]) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[..2].copy_from_slice(&op.to_le_bytes());
    for (i, a) in args.iter().enumerate() {
        out[2 + i * 2..4 + i * 2].copy_from_slice(&a.to_le_bytes());
    }
    out
}

/// The LMC driver.
#[derive(Debug)]
pub struct LmcDriver<T: Transport> {
    profile: MachineProfile,
    transport: T,
    state: MachineState,
    log: Vec<RawExchange>,
}

impl<T: Transport> LmcDriver<T> {
    /// Create a driver.
    pub fn new(profile: MachineProfile, transport: T) -> Self {
        Self {
            profile,
            transport,
            state: MachineState::Disconnected,
            log: Vec::new(),
        }
    }

    /// The underlying transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    fn send(&mut self, op: u16, args: [u16; 5], decoded: &str) -> Result<Vec<u8>> {
        let cmd = command(op, args);
        let reply = self.transport.exchange(&cmd)?;
        self.log.push(RawExchange {
            sent: cmd.to_vec(),
            received: reply.clone(),
            decoded: decoded.to_owned(),
        });
        Ok(reply)
    }

    fn not_yet(what: &str) -> Error {
        Error::new(
            ErrorCode::Unsupported,
            format!("LMC {what}"),
            "not implemented in this early build",
            "track progress in the repository issues",
        )
    }
}

impl<T: Transport> Driver for LmcDriver<T> {
    fn profile(&self) -> &MachineProfile {
        &self.profile
    }

    fn connect(&mut self) -> Result<()> {
        self.send(opcode::GET_VERSION, [0; 5], "get version")?;
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

    fn trace_frame(&mut self, request: &FrameRequest) -> Result<()> {
        match request.source {
            FramingSource::GuideLight => {
                let port = self.profile.framing.guide_light_port.ok_or_else(|| {
                    Error::new(
                        ErrorCode::SafetyBlocked,
                        "Galvo framing was blocked",
                        "the profile has no framing.guide_light_port",
                        "set the guide light's output port in the machine profile",
                    )
                })?;
                self.send(
                    opcode::WRITE_PORT,
                    [1 << port, 0, 0, 0, 0],
                    "guide light on",
                )?;
                self.state = MachineState::Framing;
                Ok(())
            }
            FramingSource::LaserOff => Err(Error::new(
                ErrorCode::Unsupported,
                "Galvo framing with the laser off",
                "a galvo has no visible head to move",
                "frame with the guide light",
            )),
            FramingSource::MainSourceLowPower => Err(Error::new(
                ErrorCode::SafetyBlocked,
                "Galvo framing with the fiber source",
                "low-power fiber framing is not implemented, so it is refused",
                "frame with the guide light",
            )),
        }
    }

    fn pause(&mut self) -> Result<()> {
        Err(Self::not_yet("pause"))
    }

    fn resume(&mut self) -> Result<()> {
        Err(Self::not_yet("resume"))
    }

    fn stop(&mut self) -> Result<()> {
        self.send(opcode::WRITE_PORT, [0; 5], "all outputs off")?;
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
    use libreburn_core::SafetyPolicy;

    fn driver() -> LmcDriver<MockTransport> {
        let p = MachineProfile::from_toml(include_str!("../../../profiles/jcz-lmc-fiber.toml"))
            .unwrap();
        LmcDriver::new(p, MockTransport::default())
    }

    fn frame(source: FramingSource) -> FrameRequest {
        FrameRequest {
            outline: vec![(0.0, 0.0), (10.0, 10.0)],
            source,
            continuous: false,
        }
    }

    #[test]
    fn guide_light_framing_writes_only_the_guide_port() {
        let mut d = driver();
        d.frame(&frame(FramingSource::GuideLight), &SafetyPolicy::default())
            .unwrap();
        let cmd = d.transport().sent.last().unwrap();
        assert_eq!(u16::from_le_bytes([cmd[0], cmd[1]]), opcode::WRITE_PORT);
        let port = d.profile().framing.guide_light_port.unwrap();
        assert_eq!(u16::from_le_bytes([cmd[2], cmd[3]]), 1 << port);
    }

    #[test]
    fn fiber_framing_without_opt_in_sends_nothing() {
        let mut d = driver();
        let err = d
            .frame(
                &frame(FramingSource::MainSourceLowPower),
                &SafetyPolicy::default(),
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::SafetyBlocked);
        assert!(d.transport().sent.is_empty());
    }

    #[test]
    fn fiber_framing_is_refused_even_with_opt_in() {
        let mut d = driver();
        let policy = SafetyPolicy {
            main_source_framing_opt_in: true,
        };
        assert!(
            d.frame(&frame(FramingSource::MainSourceLowPower), &policy)
                .is_err()
        );
        assert!(d.transport().sent.is_empty());
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
