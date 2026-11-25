// SPDX-License-Identifier: AGPL-3.0-or-later
//! JCZ / BJJCZ EZCAD2 LMC galvo driver (USB).
//!
//! [`protocol`] encodes commands, [`lists`] builds job and frame lists, and
//! [`sim`] is a software board used by the tests. Protocol knowledge comes
//! from MeerK40t's `balormk` driver (Copyright (c) 2021 meerk40t, MIT
//! License), which descends from the balor project; see `docs/protocols.md`.
//! No MeerK40t GUI code is used.
//!
//! Safety:
//! - Framing uses the red guide light only: jumps with only the guide-light
//!   bit set, the master oscillator (MO) closed first, and no mark commands.
//!   A guide-light port equal to the laser-enable port is refused.
//! - Low-power fiber framing is refused, even after the session opt-in,
//!   until it has been implemented and tested.
//! - Job power is clamped to `limits.max_power_pct`. A job that fires
//!   nothing (HIL profiles cap power at zero) never opens the MO or sets the
//!   laser-enable bit.
//! - `stop()` aborts the list, closes the MO and clears every output.

pub mod cor;
pub mod lists;
pub mod protocol;
pub mod sim;
pub mod usb;

pub use usb::UsbTransport;

use libreburn_core::{
    Driver, DriverStatus, Error, ErrorCode, FrameRequest, FramingSource, Job, MachineProfile,
    MachineState, RawExchange, Result,
};

use crate::lists::Settings;
use crate::protocol::{STATUS_BUSY, STATUS_READY, command, list_packets, reply_words, single};

/// Moves LMC packets over USB: 12-byte single commands (answered with 8
/// bytes) and 3072-byte list packets (not answered).
pub trait Transport: Send {
    /// Bulk-write one packet.
    fn write(&mut self, packet: &[u8]) -> Result<()>;
    /// Read one 8-byte reply.
    fn read(&mut self) -> Result<[u8; 8]>;
}

/// Outline passes for a one-shot red-light frame.
pub const FRAME_PASSES: u32 = 10;
/// Outline passes per list for a continuous red-light preview. The service
/// re-sends the frame until it calls `stop()`.
pub const FRAME_PASSES_CONTINUOUS: u32 = 100;
/// How many status polls to wait for the board to take another list packet.
const READY_POLLS: usize = 2000;
/// How many raw exchanges are kept.
const LOG_LIMIT: usize = 512;

/// The LMC driver.
#[derive(Debug)]
pub struct LmcDriver<T: Transport> {
    profile: MachineProfile,
    settings: Settings,
    transport: T,
    state: MachineState,
    pos: (u16, u16),
    log: Vec<RawExchange>,
}

impl<T: Transport> LmcDriver<T> {
    /// Create a driver. Fails if the profile's connection settings are invalid.
    pub fn new(profile: MachineProfile, transport: T) -> Result<Self> {
        let settings = Settings::from_profile(&profile)?;
        Ok(Self {
            profile,
            settings,
            transport,
            state: MachineState::Disconnected,
            pos: (0x8000, 0x8000),
            log: Vec::new(),
        })
    }

    /// The underlying transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Field and timing settings.
    pub fn settings(&self) -> &Settings {
        &self.settings
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

    /// Send a single command whose reply is not read (correction lines).
    fn single_no_reply(&mut self, op: u16, args: [u16; 5]) -> Result<()> {
        let cmd = command(op, args);
        self.transport.write(&cmd)?;
        self.record(cmd.to_vec(), Vec::new(), protocol::describe(&cmd));
        Ok(())
    }

    /// Send the lens correction table, or a blank one.
    fn write_correction(&mut self) -> Result<()> {
        let Some(path) = self.settings.correction_file.clone() else {
            self.single(single::WRITE_COR_TABLE, [0; 5])?;
            return Ok(());
        };
        let bytes = std::fs::read(&path).map_err(|e| {
            Error::new(
                ErrorCode::ProfileInvalid,
                "Could not read the lens correction file",
                format!("{path}: {e}"),
                "check connection.correction_file, or leave it empty for no correction",
            )
        })?;
        let table = cor::parse(&bytes)?;
        self.single(single::WRITE_COR_TABLE, [1, 0, 0, 0, 0])?;
        // MeerK40t sends the lines without reading replies.
        for (i, (dx, dy)) in table.into_iter().enumerate() {
            self.single_no_reply(single::WRITE_COR_LINE, [dx, dy, u16::from(i > 0), 0, 0])?;
        }
        Ok(())
    }

    fn single(&mut self, op: u16, args: [u16; 5]) -> Result<[u16; 4]> {
        let cmd = command(op, args);
        self.transport.write(&cmd)?;
        let reply = self.transport.read()?;
        self.record(cmd.to_vec(), reply.to_vec(), protocol::describe(&cmd));
        Ok(reply_words(&reply))
    }

    fn require_connected(&self) -> Result<()> {
        if self.state == MachineState::Disconnected {
            return Err(Error::new(
                ErrorCode::DeviceUnreachable,
                "The LMC board is not connected",
                "connect() has not succeeded yet",
                "connect to the machine first",
            ));
        }
        Ok(())
    }

    fn wait_ready(&mut self) -> Result<()> {
        for _ in 0..READY_POLLS {
            let st = self.single(single::GET_VERSION, [0; 5])?[3];
            if st & STATUS_READY != 0 {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err(Error::new(
            ErrorCode::DeviceUnreachable,
            "The LMC board stayed busy",
            "it did not report READY for 10 seconds",
            "stop the job, then power-cycle the board if it persists",
        ))
    }

    /// Upload list commands and start them.
    fn run_list(&mut self, cmds: &[protocol::Command], what: &str) -> Result<()> {
        self.single(single::RESET_LIST, [0; 5])?;
        let packets = list_packets(cmds);
        let n = packets.len();
        let mut executing = false;
        for (i, p) in packets.into_iter().enumerate() {
            self.wait_ready()?;
            self.transport.write(&p)?;
            self.record(p, Vec::new(), format!("{what} list packet {}/{n}", i + 1));
            self.single(single::SET_END_OF_LIST, [0; 5])?;
            // Start once a few packets are buffered, as EZCAD does.
            if i >= 2 && !executing {
                self.single(single::EXECUTE_LIST, [0; 5])?;
                executing = true;
            }
        }
        if !executing {
            self.single(single::EXECUTE_LIST, [0; 5])?;
        }
        Ok(())
    }

    /// Close the MO and clear every output.
    fn safe_outputs(&mut self) -> Result<()> {
        self.single(single::FIBER_SET_MO, [0; 5])?;
        self.single(single::WRITE_PORT, [0; 5])?;
        Ok(())
    }

    fn init(&mut self) -> Result<()> {
        // The init sequence and default values follow MeerK40t's
        // GalvoController.init_laser. Unconfirmed on our boards.
        self.single(single::GET_SERIAL_NO, [0; 5])?;
        self.single(single::GET_VERSION, [0; 5])?;
        self.single(single::RESET, [0; 5])?;
        self.write_correction()?;
        self.safe_outputs()?;
        self.single(single::ENABLE_LASER, [0; 5])?;
        self.single(single::SET_CONTROL_MODE, [0; 5])?;
        self.single(single::SET_LASER_MODE, [1, 0, 0, 0, 0])?;
        self.single(single::SET_DELAY_MODE, [1, 0, 0, 0, 0])?;
        self.single(single::SET_TIMING, [1, 0, 0, 0, 0])?;
        self.single(single::SET_STANDBY, [2000, 20, 0, 0, 0])?;
        self.single(single::SET_FIRST_PULSE_KILLER, [200, 0, 0, 0, 0])?;
        self.single(single::SET_PWM_HALF_PERIOD, [125, 0, 0, 0, 0])?;
        self.single(single::SET_PWM_PULSE_WIDTH, [125, 0, 0, 0, 0])?;
        self.single(single::FIBER_SET_MO, [0; 5])?;
        self.single(single::SET_FPK_PARAM_2, [0xFFB, 1, 409, 409, 0])?;
        self.single(single::SET_FLY_RES, [0, 99, 1000, 25, 0])?;
        self.single(single::ENABLE_Z, [0; 5])?;
        self.single(single::WRITE_ANALOG_PORT_1, [0x7FF, 0, 0, 0, 0])?;
        self.single(single::ENABLE_Z, [0; 5])?;
        Ok(())
    }
}

impl<T: Transport> Driver for LmcDriver<T> {
    fn profile(&self) -> &MachineProfile {
        &self.profile
    }

    fn connect(&mut self) -> Result<()> {
        self.init()?;
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
        let st = self.single(single::GET_VERSION, [0; 5])?[3];
        let busy = st & STATUS_BUSY != 0;
        self.state = match (self.state, busy) {
            (MachineState::Paused, _) => MachineState::Paused,
            (s, true) => s,
            (MachineState::Running | MachineState::Framing, false) => {
                // The list finished: close the MO and clear outputs.
                self.safe_outputs()?;
                MachineState::Idle
            }
            (s, false) => s,
        };
        Ok(DriverStatus {
            state: self.state,
            position: Some(self.settings.from_galvo(self.pos.0, self.pos.1)),
        })
    }

    fn read_param(&mut self, name: &str) -> Result<Vec<u8>> {
        self.require_connected()?;
        let op = match name {
            "version" | "status" => single::GET_VERSION,
            "serial" => single::GET_SERIAL_NO,
            "list_status" => single::GET_LIST_STATUS,
            "position" => single::GET_POSITION_XY,
            "input_port" => single::READ_PORT,
            "mark_time" => single::GET_MARK_TIME,
            _ => {
                return Err(Error::new(
                    ErrorCode::Unsupported,
                    format!("Unknown LMC parameter \"{name}\""),
                    "it is not a readable board value",
                    "use version, serial, list_status, position, input_port or mark_time",
                ));
            }
        };
        let arg = if op == single::GET_MARK_TIME { 3 } else { 0 };
        let w = self.single(op, [arg, 0, 0, 0, 0])?;
        Ok(w.iter().flat_map(|x| x.to_le_bytes()).collect())
    }

    fn write_param(&mut self, name: &str, _value: &[u8]) -> Result<()> {
        Err(Error::new(
            ErrorCode::Unsupported,
            format!("Writing LMC parameter \"{name}\""),
            "board parameter writes are disabled until a reviewed allowlist exists",
            "set timing and laser parameters in the profile instead",
        ))
    }

    fn run_job(&mut self, job: &Job) -> Result<()> {
        self.require_connected()?;
        let built = lists::job_list(job, &self.profile, &self.settings, self.pos)?;
        if built.fires {
            let enable = self.settings.laser_enable_mask();
            self.single(single::WRITE_PORT, [enable, 0, 0, 0, 0])?;
            self.single(single::FIBER_SET_MO, [1, 0, 0, 0, 0])?;
        } else {
            self.safe_outputs()?;
        }
        if let Err(e) = self.run_list(&built.commands, "job") {
            // Never leave the source armed after a failed upload.
            let _ = self.single(single::STOP_EXECUTE, [0; 5]);
            let _ = self.safe_outputs();
            return Err(e);
        }
        self.pos = built.end;
        self.state = MachineState::Running;
        Ok(())
    }

    fn jog(&mut self, dx: f64, dy: f64) -> Result<()> {
        self.require_connected()?;
        let (x, y) = self.settings.from_galvo(self.pos.0, self.pos.1);
        let (tx, ty) = (x + dx, y + dy);
        self.settings.check(tx, ty)?;
        let to = self.settings.to_galvo(tx, ty);
        self.single(single::GOTO_XY, [to.0, to.1, 0, 0xFFFF, 0])?;
        self.pos = to;
        Ok(())
    }

    fn trace_frame(&mut self, request: &FrameRequest) -> Result<()> {
        self.require_connected()?;
        match request.source {
            FramingSource::GuideLight => {
                let passes = if request.continuous {
                    FRAME_PASSES_CONTINUOUS
                } else {
                    FRAME_PASSES
                };
                let (cmds, end) =
                    lists::frame_list(&request.outline, &self.settings, self.pos, passes)?;
                // The fiber source stays disarmed for the whole frame.
                self.safe_outputs()?;
                self.run_list(&cmds, "frame")?;
                self.pos = end;
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
        self.require_connected()?;
        self.single(single::STOP_LIST, [0; 5])?;
        self.state = MachineState::Paused;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        self.require_connected()?;
        self.single(single::RESTART_LIST, [0; 5])?;
        self.state = MachineState::Running;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.single(single::STOP_EXECUTE, [0; 5])?;
        self.single(single::RESET_LIST, [0; 5])?;
        self.safe_outputs()?;
        if self.state != MachineState::Disconnected {
            self.state = MachineState::Idle;
        }
        Ok(())
    }

    fn raw_log(&self) -> &[RawExchange] {
        &self.log
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{decode, list};
    use crate::sim::{LmcSim, SimTransport};
    use libreburn_core::{Layer, LayerMode, Move, SafetyPolicy};

    const PROFILE: &str = include_str!("../../../profiles/jcz-lmc-fiber.toml");
    const HIL: &str = include_str!("../../../hil/profiles/hil-jcz-lmc-fiber.toml");

    fn driver_from(text: &str) -> LmcDriver<SimTransport> {
        let p = MachineProfile::from_toml(text).unwrap();
        let light = p.framing.guide_light_port.unwrap();
        let mut d = LmcDriver::new(p, SimTransport::new(LmcSim::new(0, light))).unwrap();
        d.connect().unwrap();
        d
    }

    fn driver() -> LmcDriver<SimTransport> {
        driver_from(PROFILE)
    }

    fn sim(d: &LmcDriver<SimTransport>) -> LmcSim {
        d.transport().sim.lock().unwrap().clone()
    }

    fn frame(source: FramingSource, continuous: bool) -> FrameRequest {
        FrameRequest {
            outline: vec![(10.0, 10.0), (60.0, 10.0), (60.0, 40.0), (10.0, 40.0)],
            source,
            continuous,
        }
    }

    fn job(power: f64) -> Job {
        Job {
            layers: vec![Layer {
                name: "mark".into(),
                mode: LayerMode::Cut,
                speed_mm_s: 800.0,
                power_pct: power,
                frequency_khz: Some(30.0),
                ..Default::default()
            }],
            moves: vec![
                Move::travel(10.0, 10.0, 0),
                Move::burn(20.0, 10.0, 0),
                Move::burn(20.0, 20.0, 0).with_power(95.0),
                Move::travel(50.0, 50.0, 0),
            ],
        }
    }

    #[test]
    fn connect_leaves_the_source_disarmed() {
        let d = driver();
        let s = sim(&d);
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert!(s.laser_enabled);
        assert!(!s.mo_ever_opened);
        assert_eq!(s.port, 0);
        assert!(!d.raw_log().is_empty());
    }

    #[test]
    fn guide_light_framing_traces_the_outline_without_firing() {
        let mut d = driver();
        d.frame(
            &frame(FramingSource::GuideLight, false),
            &SafetyPolicy::default(),
        )
        .unwrap();
        let s = sim(&d);
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.mark_attempts, 0);
        assert!(s.burns.is_empty());
        assert!(!s.mo_ever_opened);
        assert!(!s.laser_signal_on);
        // Only the guide-light bit (8) is ever set; it ends switched off.
        assert!(s.port_history.iter().all(|&p| p == 0 || p == 1 << 8));
        assert!(s.port_history.contains(&(1 << 8)));
        assert_eq!(s.port, 0);
        // 4 sides traced FRAME_PASSES times with the light on.
        assert_eq!(s.light_trace.len(), 4 * FRAME_PASSES as usize);
        let set = d.settings().clone();
        assert_eq!(s.light_trace[0].0, set.to_galvo(10.0, 10.0));
        assert_eq!(s.light_trace[0].1, set.to_galvo(60.0, 10.0));
        assert_eq!(d.status().unwrap().state, MachineState::Idle);
    }

    #[test]
    fn continuous_framing_uses_more_passes_and_stop_clears_everything() {
        let mut d = driver();
        d.frame(
            &frame(FramingSource::GuideLight, true),
            &SafetyPolicy::default(),
        )
        .unwrap();
        assert_eq!(
            sim(&d).light_trace.len(),
            4 * FRAME_PASSES_CONTINUOUS as usize
        );
        assert!(sim(&d).list_packets > 1);
        d.stop().unwrap();
        let s = sim(&d);
        assert_eq!(s.port, 0);
        assert!(!s.mo_open);
        assert_eq!(s.mark_attempts, 0);
    }

    #[test]
    fn framing_after_a_job_closes_the_mo_first() {
        let mut d = driver();
        d.run_job(&job(30.0)).unwrap();
        assert!(sim(&d).mo_open);
        d.frame(
            &frame(FramingSource::GuideLight, false),
            &SafetyPolicy::default(),
        )
        .unwrap();
        let s = sim(&d);
        assert!(!s.mo_open);
        let marks_before = s.burns.len();
        assert_eq!(marks_before, 2, "only the job's two burns");
    }

    #[test]
    fn light_port_on_the_enable_bit_sends_nothing() {
        let text = PROFILE.replace("guide_light_port = 8", "guide_light_port = 0");
        let mut d = driver_from(&text);
        let before = sim(&d).singles.len();
        let err = d
            .frame(
                &frame(FramingSource::GuideLight, false),
                &SafetyPolicy::default(),
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::SafetyBlocked);
        assert_eq!(sim(&d).singles.len(), before);
    }

    #[test]
    fn fiber_framing_is_refused_with_and_without_opt_in() {
        let mut d = driver();
        let before = sim(&d).singles.len();
        for opt_in in [false, true] {
            let policy = SafetyPolicy {
                main_source_framing_opt_in: opt_in,
            };
            let err = d
                .frame(&frame(FramingSource::MainSourceLowPower, false), &policy)
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::SafetyBlocked);
        }
        assert_eq!(sim(&d).singles.len(), before);
        assert!(!sim(&d).mo_ever_opened);
    }

    #[test]
    fn job_fires_with_capped_power_and_status_disarms_after() {
        let text = PROFILE.replace("max_power_pct = 100", "max_power_pct = 60");
        let mut d = driver_from(&text);
        d.run_job(&job(50.0)).unwrap();
        let s = sim(&d);
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.burns.len(), 2);
        assert!((s.burns[0].power_pct - 50.0).abs() < 0.05);
        // The 95 % override is capped at 60 %.
        assert!((s.burns[1].power_pct - 60.0).abs() < 0.05);
        assert!(s.max_burn_power() <= 60.0 + 0.05);
        let set = d.settings().clone();
        assert_eq!(s.burns[0].from, set.to_galvo(10.0, 10.0));
        assert_eq!(s.burns[1].to, set.to_galvo(20.0, 20.0));
        assert_eq!(d.status().unwrap().state, MachineState::Idle);
        let s = sim(&d);
        assert!(!s.mo_open);
        assert_eq!(s.port, 0);
    }

    #[test]
    fn hil_profile_never_arms_the_source() {
        let mut d = driver_from(HIL);
        d.run_job(&job(100.0)).unwrap();
        let s = sim(&d);
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.mark_attempts, 0);
        assert!(s.burns.is_empty());
        assert!(!s.mo_ever_opened);
        assert!(s.port_history.iter().all(|&p| p & 1 == 0));
        // Motion still happens, so HIL can check geometry.
        assert_eq!(s.pos, d.settings().to_galvo(50.0, 50.0));
    }

    #[test]
    fn pause_resume_jog_and_params() {
        let mut d = driver();
        d.run_job(&job(10.0)).unwrap();
        d.pause().unwrap();
        assert!(sim(&d).paused);
        assert_eq!(d.status().unwrap().state, MachineState::Paused);
        d.resume().unwrap();
        assert!(!sim(&d).paused);
        d.stop().unwrap();
        d.jog(5.0, -10.0).unwrap();
        let expect = d.settings().to_galvo(55.0, 40.0);
        assert_eq!(sim(&d).pos, expect);
        assert!(d.jog(100.0, 0.0).is_err());
        let serial = d.read_param("serial").unwrap();
        assert_eq!(serial.len(), 8);
        assert!(d.write_param("power", &[1]).is_err());
    }

    #[test]
    fn correction_file_is_uploaded_on_connect() {
        let dir = std::env::temp_dir().join(format!("libreburn-cor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lens.cor");
        std::fs::write(&path, cor::tests::int_file()).unwrap();
        let text = PROFILE.replace(
            "correction_file = \"\"",
            &format!("correction_file = {:?}", path.to_str().unwrap()),
        );
        let d = driver_from(&text);
        let s = sim(&d);
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        let lines: Vec<_> = s
            .singles
            .iter()
            .map(|c| decode(c))
            .filter(|(op, _)| *op == single::WRITE_COR_LINE)
            .collect();
        assert_eq!(lines.len(), cor::ENTRIES);
        assert_eq!(lines[0].1[2], 0);
        assert_eq!(lines[5].1, [5, 0x8005, 1, 0, 0]);
        std::fs::remove_dir_all(dir).unwrap();

        let bad = PROFILE.replace(
            "correction_file = \"\"",
            "correction_file = \"/nonexistent.cor\"",
        );
        let p = MachineProfile::from_toml(&bad).unwrap();
        let mut d = LmcDriver::new(p, SimTransport::new(LmcSim::new(0, 8))).unwrap();
        assert_eq!(d.connect().unwrap_err().code, ErrorCode::ProfileInvalid);
    }

    #[test]
    fn lists_only_contain_known_commands() {
        let mut d = driver();
        d.run_job(&job(20.0)).unwrap();
        let s = sim(&d);
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        let first = decode(&s.executed[0]).0;
        assert_eq!(first, list::READY_MARK);
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
