// SPDX-License-Identifier: AGPL-3.0-or-later
//! Ruida controller driver (OmTech K40 and other CO2 gantry machines).
//!
//! Commands are encoded as an `.rd` stream ([`rd`]), scrambled with the
//! board's key ("magic", 0x88 on RDC644x) and sent over UDP or USB serial
//! ([`transport`]). [`sim`] is a software controller used by the tests.
//!
//! Protocol knowledge comes from MeerK40t (Copyright (c) 2021 meerk40t, MIT
//! License) and public notes; see `docs/protocols.md`. No MeerK40t GUI code
//! is used.
//!
//! Safety:
//! - Power is clamped to `limits.max_power_pct`; a zero cap (HIL profiles)
//!   means no cut opcode is ever sent.
//! - Framing only supports laser-off tracing: the frame job contains no cut
//!   opcodes and zero power. Main-source framing is refused.
//! - Board parameter writes are refused until an allowlist of safe
//!   addresses has been reviewed.

pub mod codec;
pub mod protocol;
pub mod rd;
pub mod sim;
pub mod transport;

use libreburn_core::{
    Driver, DriverStatus, Error, ErrorCode, FrameRequest, FramingSource, Job, MachineProfile,
    MachineState, RawExchange, Result,
};

pub use transport::{Link, SerialTransport, Session, Transport, UdpTransport, open_from_profile};

use crate::codec::mem;
use crate::rd::Geometry;

/// Speed for laser-off framing, mm/s, before the profile's speed limit.
pub const FRAME_SPEED_MM_S: f64 = 100.0;

/// The scramble key from the profile's `[connection] magic`, or the RDC644x default.
pub fn magic_from_profile(profile: &MachineProfile) -> Result<u8> {
    match profile.connection.get("magic") {
        None => Ok(protocol::MAGIC_644X),
        Some(v) => v
            .as_integer()
            .and_then(|i| u8::try_from(i).ok())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ProfileInvalid,
                    "Invalid Ruida scramble key",
                    format!("connection.magic is {v}"),
                    "use a byte value such as 0x88 (RDC644x), 0x11 or 0x38",
                )
            }),
    }
}

/// The Ruida driver.
#[derive(Debug)]
pub struct RuidaDriver<T: Transport> {
    profile: MachineProfile,
    session: Session<T>,
    geometry: Geometry,
    state: MachineState,
}

impl<T: Transport> RuidaDriver<T> {
    /// Create a driver. `magic` is the board's scramble key (0x88 on RDC644x boards).
    pub fn new(profile: MachineProfile, transport: T, magic: u8) -> Self {
        let geometry = Geometry::from_profile(&profile);
        Self {
            profile,
            session: Session::new(transport, magic),
            geometry,
            state: MachineState::Disconnected,
        }
    }

    /// Create a driver with the scramble key from the profile.
    pub fn from_profile(profile: MachineProfile, transport: T) -> Result<Self> {
        let magic = magic_from_profile(&profile)?;
        Ok(Self::new(profile, transport, magic))
    }

    /// The underlying transport.
    pub fn transport(&self) -> &T {
        self.session.transport()
    }

    /// The session (timeouts, retries).
    pub fn session_mut(&mut self) -> &mut Session<T> {
        &mut self.session
    }

    fn require_connected(&self) -> Result<()> {
        if self.state == MachineState::Disconnected {
            return Err(Error::new(
                ErrorCode::DeviceUnreachable,
                "The Ruida controller is not connected",
                "connect() has not succeeded yet",
                "connect to the machine first",
            ));
        }
        Ok(())
    }

    fn send_commands(&mut self, cmds: &[Vec<u8>], what: &str) -> Result<()> {
        let packets = rd::chunk(cmds);
        let n = packets.len();
        for (i, p) in packets.iter().enumerate() {
            self.session.send(
                p,
                &format!("{what} packet {}/{n} ({} bytes)", i + 1, p.len()),
            )?;
        }
        Ok(())
    }

    fn simple(&mut self, payload: &[u8]) -> Result<()> {
        self.session.send(payload, &codec::describe(payload))?;
        Ok(())
    }

    /// Current head position in job coordinates (mm).
    pub fn position(&mut self) -> Result<(f64, f64)> {
        let x = self.session.read_mem(mem::CURRENT_X)? as i32;
        let y = self.session.read_mem(mem::CURRENT_Y)? as i32;
        Ok(self.geometry.from_machine(x, y))
    }

    /// Home X and Y against the end stops.
    pub fn home(&mut self) -> Result<()> {
        self.require_connected()?;
        self.simple(&codec::HOME_XY)
    }
}

impl<T: Transport> Driver for RuidaDriver<T> {
    fn profile(&self) -> &MachineProfile {
        &self.profile
    }

    fn connect(&mut self) -> Result<()> {
        match self.session.transport().link() {
            // UDP: an enquiry is answered with an ACK.
            Link::Udp => {
                self.session.send(&[codec::ENQ], "enquiry")?;
            }
            // Serial has no ACKs, so read the status word instead.
            Link::Serial => {
                self.session.read_mem(mem::MACHINE_STATUS)?;
            }
        }
        self.state = MachineState::Idle;
        Ok(())
    }

    fn status(&mut self) -> Result<DriverStatus> {
        if self.state == MachineState::Disconnected {
            return Ok(DriverStatus {
                state: self.state,
                position: None,
            });
        }
        let bits = self.session.read_mem(mem::MACHINE_STATUS)?;
        let position = Some(self.position()?);
        let busy =
            bits & (mem::STATUS_JOB_RUNNING | mem::STATUS_MOVING | mem::STATUS_PART_END) != 0;
        self.state = match (self.state, busy) {
            (MachineState::Paused, _) => MachineState::Paused,
            (MachineState::Framing, true) => MachineState::Framing,
            (_, true) => MachineState::Running,
            (_, false) => MachineState::Idle,
        };
        Ok(DriverStatus {
            state: self.state,
            position,
        })
    }

    fn read_param(&mut self, name: &str) -> Result<Vec<u8>> {
        self.require_connected()?;
        let addr = mem::by_name(name).ok_or_else(|| {
            Error::new(
                ErrorCode::Unsupported,
                format!("Unknown Ruida parameter \"{name}\""),
                "it is not a known name or a 0x-prefixed address",
                "use x, y, z, u, bed_x, bed_y, card_id, machine_status or an address like 0x0421",
            )
        })?;
        Ok(self.session.read_mem(addr)?.to_be_bytes().to_vec())
    }

    fn write_param(&mut self, name: &str, _value: &[u8]) -> Result<()> {
        Err(Error::new(
            ErrorCode::Unsupported,
            format!("Writing Ruida parameter \"{name}\""),
            "board parameter writes are disabled until a reviewed allowlist exists, \
             because some addresses change laser power limits",
            "change board settings on the controller's panel for now",
        ))
    }

    fn run_job(&mut self, job: &Job) -> Result<()> {
        self.require_connected()?;
        let cmds = rd::encode_job(job, &self.profile)?;
        self.send_commands(&cmds, "job")?;
        self.state = MachineState::Running;
        Ok(())
    }

    fn jog(&mut self, dx: f64, dy: f64) -> Result<()> {
        self.require_connected()?;
        let (x, y) = self.position()?;
        let (tx, ty) = (x + dx, y + dy);
        self.geometry.check(tx, ty)?;
        let (mx, my) = self.geometry.to_machine(tx, ty);
        let payload = codec::cmd(&[
            &codec::RAPID_MOVE_XY,
            &[codec::RAPID_OPTION_ORIGIN],
            &codec::encode_coord(mx),
            &codec::encode_coord(my),
        ]);
        self.simple(&payload)
    }

    fn trace_frame(&mut self, request: &FrameRequest) -> Result<()> {
        self.require_connected()?;
        match request.source {
            FramingSource::LaserOff => {
                let cmds = rd::encode_frame(&request.outline, &self.profile, FRAME_SPEED_MM_S)?;
                self.send_commands(&cmds, "frame")?;
                self.state = MachineState::Framing;
                Ok(())
            }
            FramingSource::GuideLight => Err(Error::new(
                ErrorCode::Unsupported,
                "Ruida framing with a guide light",
                "this driver does not drive a red-dot pointer",
                "frame with the laser off",
            )),
            FramingSource::MainSourceLowPower => Err(Error::new(
                ErrorCode::SafetyBlocked,
                "Ruida framing with the laser tube",
                "low-power tube framing is not implemented, so it is refused",
                "frame with the laser off",
            )),
        }
    }

    fn pause(&mut self) -> Result<()> {
        self.require_connected()?;
        self.simple(&codec::PAUSE_PROCESS)?;
        self.state = MachineState::Paused;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        self.require_connected()?;
        self.simple(&codec::RESTORE_PROCESS)?;
        self.state = MachineState::Running;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.simple(&codec::STOP_PROCESS)?;
        if self.state != MachineState::Disconnected {
            self.state = MachineState::Idle;
        }
        Ok(())
    }

    fn raw_log(&self) -> &[RawExchange] {
        self.session.log()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{RuidaSim, SimTransport};
    use libreburn_core::{Layer, LayerMode, Move, MoveKind, SafetyPolicy};
    use std::time::Duration;

    fn profile_text(max_power: f64) -> String {
        include_str!("../../../profiles/omtech-k40-ruida.toml").replace(
            "max_power_pct = 70",
            &format!("max_power_pct = {max_power}"),
        )
    }

    fn profile() -> MachineProfile {
        MachineProfile::from_toml(&profile_text(70.0)).unwrap()
    }

    fn driver_with(p: MachineProfile, link: Link) -> RuidaDriver<SimTransport> {
        let sim = RuidaSim::new(protocol::MAGIC_644X, link, (300.0, 200.0));
        let mut d = RuidaDriver::from_profile(p, SimTransport::new(sim)).unwrap();
        d.connect().unwrap();
        d
    }

    fn driver(link: Link) -> RuidaDriver<SimTransport> {
        driver_with(profile(), link)
    }

    fn sim(d: &RuidaDriver<SimTransport>) -> RuidaSim {
        d.transport().sim.lock().unwrap().clone()
    }

    fn mv(kind: MoveKind, x: f64, y: f64) -> Move {
        match kind {
            MoveKind::Travel => Move::travel(x, y, 0),
            MoveKind::Burn => Move::burn(x, y, 0),
        }
    }

    fn job(power: f64) -> Job {
        Job {
            layers: vec![Layer {
                name: "cut".into(),
                mode: LayerMode::Cut,
                speed_mm_s: 15.0,
                power_pct: power,
                ..Default::default()
            }],
            moves: vec![
                mv(MoveKind::Travel, 10.0, 10.0),
                mv(MoveKind::Burn, 15.0, 10.0),
                mv(MoveKind::Burn, 15.0, 12.0),
                mv(MoveKind::Travel, 100.0, 100.0),
                mv(MoveKind::Burn, 110.0, 101.5),
            ],
        }
    }

    #[test]
    fn connect_sends_one_scrambled_enquiry() {
        let d = driver(Link::Udp);
        let s = sim(&d);
        assert_eq!(s.commands, vec![vec![codec::ENQ]]);
        assert_eq!(d.raw_log().len(), 1);
        // On the wire: checksum 00 C8, then 0xCE scrambled to 0xC8; reply is ACK 0xC6.
        assert_eq!(d.raw_log()[0].sent, vec![0x00, 0xC8, 0xC8]);
        assert_eq!(d.raw_log()[0].received, vec![0xC6]);
    }

    #[test]
    fn job_runs_on_the_simulator_with_exact_geometry() {
        for link in [Link::Udp, Link::Serial] {
            let mut d = driver(link);
            d.run_job(&job(40.0)).unwrap();
            let s = sim(&d);
            assert!(s.errors.is_empty(), "{link:?}: {:?}", s.errors);
            assert_eq!(s.completed_jobs, 1);
            let segs: Vec<_> = s.burns.iter().map(|b| (b.from, b.to)).collect();
            assert_eq!(
                segs,
                vec![
                    ((10_000, 10_000), (15_000, 10_000)),
                    ((15_000, 10_000), (15_000, 12_000)),
                    ((100_000, 100_000), (110_000, 101_500)),
                ]
            );
            assert!((s.max_burn_power() - 40.0).abs() < 0.01);
        }
    }

    #[test]
    fn power_never_exceeds_the_profile_cap() {
        let mut d = driver(Link::Udp);
        d.run_job(&job(100.0)).unwrap();
        let s = sim(&d);
        assert!(s.max_burn_power() <= 70.0 + 0.01);
        assert!(!s.burns.is_empty());
    }

    #[test]
    fn hil_profile_never_fires() {
        let text = include_str!("../../../hil/profiles/hil-omtech-k40-ruida.toml");
        let mut d = driver_with(MachineProfile::from_toml(text).unwrap(), Link::Udp);
        d.run_job(&job(100.0)).unwrap();
        let s = sim(&d);
        assert!(s.burns.is_empty());
        assert!(s.commands.iter().all(|c| !(0xA8..=0xAB).contains(&c[0])));
        assert_eq!(s.pos, (110_000, 101_500));
    }

    #[test]
    fn frame_moves_the_head_without_firing() {
        let mut d = driver(Link::Udp);
        let req = FrameRequest {
            outline: vec![(10.0, 10.0), (60.0, 10.0), (60.0, 40.0), (10.0, 40.0)],
            source: FramingSource::LaserOff,
            continuous: false,
        };
        d.frame(&req, &SafetyPolicy::default()).unwrap();
        let s = sim(&d);
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert!(s.burns.is_empty());
        assert_eq!(
            s.travels,
            vec![
                (10_000, 10_000),
                (60_000, 10_000),
                (60_000, 40_000),
                (10_000, 40_000),
                (10_000, 10_000)
            ]
        );
    }

    #[test]
    fn main_source_framing_is_refused_even_with_opt_in() {
        let mut d = driver(Link::Udp);
        let before = sim(&d).commands.len();
        let req = FrameRequest {
            outline: vec![(10.0, 10.0), (20.0, 20.0)],
            source: FramingSource::MainSourceLowPower,
            continuous: false,
        };
        let policy = SafetyPolicy {
            main_source_framing_opt_in: true,
        };
        assert!(d.frame(&req, &policy).is_err());
        assert!(d.frame(&req, &SafetyPolicy::default()).is_err());
        assert_eq!(sim(&d).commands.len(), before);
    }

    #[test]
    fn status_and_jog_use_memory_reads() {
        for link in [Link::Udp, Link::Serial] {
            let mut d = driver(link);
            d.jog(12.5, 7.0).unwrap();
            assert_eq!(sim(&d).pos, (12_500, 7_000));
            let st = d.status().unwrap();
            assert_eq!(st.state, MachineState::Idle);
            assert_eq!(st.position, Some((12.5, 7.0)));
            assert!(
                d.jog(-20.0, 0.0).is_err(),
                "jog off the bed must be refused"
            );
            assert_eq!(
                d.read_param("card_id").unwrap(),
                sim::SIM_CARD_ID.to_be_bytes().to_vec()
            );
        }
    }

    #[test]
    fn pause_resume_stop() {
        let mut d = driver(Link::Udp);
        d.run_job(&job(20.0)).unwrap();
        d.pause().unwrap();
        assert!(sim(&d).paused);
        assert_eq!(d.status().unwrap().state, MachineState::Paused);
        d.resume().unwrap();
        assert!(!sim(&d).paused);
        d.stop().unwrap();
        assert_eq!(d.status().unwrap().state, MachineState::Idle);
    }

    #[test]
    fn nak_is_retried_and_timeouts_report_unreachable() {
        let mut d = driver(Link::Udp);
        d.transport().sim.lock().unwrap().nak_next = 2;
        d.stop().unwrap();
        assert_eq!(sim(&d).naks, 2);

        d.transport().sim.lock().unwrap().drop_next = 100;
        let err = d.connect().unwrap_err();
        assert_eq!(err.code, ErrorCode::DeviceUnreachable);
    }

    #[test]
    fn wrong_magic_is_detected() {
        let sim = RuidaSim::new(0x11, Link::Udp, (300.0, 200.0));
        let mut d = RuidaDriver::new(profile(), SimTransport::new(sim), protocol::MAGIC_644X);
        d.session_mut().retries = 1;
        assert!(d.connect().is_err());
    }

    #[test]
    fn param_writes_are_refused() {
        let mut d = driver(Link::Udp);
        assert_eq!(
            d.write_param("0x0026", &[1, 2, 3, 4]).unwrap_err().code,
            ErrorCode::Unsupported
        );
        assert!(sim(&d).written.is_empty());
    }

    #[test]
    fn real_udp_socket_against_the_simulator() {
        let sim = RuidaSim::new(protocol::MAGIC_644X, Link::Udp, (300.0, 200.0));
        let shared = std::sync::Arc::new(std::sync::Mutex::new(sim));
        let (addr, _h) = sim::spawn_udp(shared.clone(), Duration::from_secs(5)).unwrap();
        let t = UdpTransport::with_ports("127.0.0.1", addr.port(), 0).unwrap();
        let mut d = RuidaDriver::new(profile(), t, protocol::MAGIC_644X);
        d.session_mut().timeout = Duration::from_millis(500);
        d.connect().unwrap();
        d.run_job(&job(30.0)).unwrap();
        let st = d.status().unwrap();
        assert_eq!(st.position, Some((110.0, 101.5)));
        let s = shared.lock().unwrap();
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.completed_jobs, 1);
        assert_eq!(s.burns.len(), 3);
    }

    #[test]
    fn magic_comes_from_the_profile() {
        let p =
            MachineProfile::from_toml(&profile_text(70.0).replace("magic = 0x88", "magic = 0x11"))
                .unwrap();
        assert_eq!(magic_from_profile(&p).unwrap(), 0x11);
        let p =
            MachineProfile::from_toml(&profile_text(70.0).replace("magic = 0x88", "magic = 300"))
                .unwrap();
        assert!(magic_from_profile(&p).is_err());
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
