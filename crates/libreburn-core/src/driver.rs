// SPDX-License-Identifier: AGPL-3.0-or-later
//! The driver interface (requirements HW-2 and HW-4).
//!
//! Adding a board means writing one implementation of [`Driver`] plus a
//! profile file. Nothing outside the driver crate should need to change.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::job::Job;
use crate::profile::MachineProfile;
use crate::safety::{FramingSource, SafetyPolicy, check_framing};

/// Coarse machine state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    /// Not connected.
    Disconnected,
    /// Connected and idle.
    Idle,
    /// Running a job.
    Running,
    /// Job paused.
    Paused,
    /// Tracing a frame.
    Framing,
    /// The controller reports an alarm.
    Alarm,
}

/// A status snapshot from the driver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriverStatus {
    /// Current state.
    pub state: MachineState,
    /// Head or beam position in mm, if known.
    pub position: Option<(f64, f64)>,
}

/// One raw exchange with the controller, kept for the device panel and traces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawExchange {
    /// Bytes sent.
    pub sent: Vec<u8>,
    /// Bytes received.
    pub received: Vec<u8>,
    /// Human-readable decoding.
    pub decoded: String,
}

/// A framing request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameRequest {
    /// Closed outline to trace, in mm.
    pub outline: Vec<(f64, f64)>,
    /// What traces it.
    pub source: FramingSource,
    /// Keep tracing until stopped (galvo red-light preview).
    pub continuous: bool,
}

/// What every controller driver implements.
pub trait Driver: Send {
    /// The profile this driver was opened with.
    fn profile(&self) -> &MachineProfile;
    /// Open the connection.
    fn connect(&mut self) -> Result<()>;
    /// Query the controller's state.
    fn status(&mut self) -> Result<DriverStatus>;
    /// Read a board parameter by name.
    fn read_param(&mut self, name: &str) -> Result<Vec<u8>>;
    /// Write a board parameter by name.
    fn write_param(&mut self, name: &str, value: &[u8]) -> Result<()>;
    /// Stream a planned job.
    fn run_job(&mut self, job: &Job) -> Result<()>;
    /// Jog by a relative offset in mm.
    fn jog(&mut self, dx: f64, dy: f64) -> Result<()>;
    /// Trace an outline. Called only through [`Driver::frame`], after safety checks.
    fn trace_frame(&mut self, request: &FrameRequest) -> Result<()>;
    /// Pause the running job.
    fn pause(&mut self) -> Result<()>;
    /// Resume a paused job.
    fn resume(&mut self) -> Result<()>;
    /// Stop immediately.
    fn stop(&mut self) -> Result<()>;
    /// Recent raw exchanges, oldest first.
    fn raw_log(&self) -> &[RawExchange];

    /// Frame the job after checking the safety policy. Drivers should not override this.
    fn frame(&mut self, request: &FrameRequest, policy: &SafetyPolicy) -> Result<()> {
        check_framing(self.profile(), request.source, policy)?;
        self.trace_frame(request)
    }
}

pub mod mock {
    //! A hardware-free [`Driver`] that records every call (requirement NF-7).
    //!
    //! The service and its tests use it to exercise the driver interface
    //! without a controller. Each protocol driver still ships its own mock
    //! transport for byte-level fixtures.

    use std::collections::BTreeMap;

    use super::{Driver, DriverStatus, FrameRequest, MachineState, RawExchange};
    use crate::error::{Error, ErrorCode, Result};
    use crate::job::Job;
    use crate::profile::MachineProfile;

    /// One call the mock received.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Call {
        /// [`Driver::connect`].
        Connect,
        /// [`Driver::run_job`], with the number of moves.
        RunJob(usize),
        /// [`Driver::jog`].
        Jog(f64, f64),
        /// [`Driver::trace_frame`].
        TraceFrame(FrameRequest),
        /// [`Driver::pause`].
        Pause,
        /// [`Driver::resume`].
        Resume,
        /// [`Driver::stop`].
        Stop,
        /// [`Driver::write_param`].
        WriteParam(String, Vec<u8>),
    }

    /// Records calls and keeps a fake head position.
    #[derive(Debug)]
    pub struct RecordingDriver {
        profile: MachineProfile,
        state: MachineState,
        position: (f64, f64),
        params: BTreeMap<String, Vec<u8>>,
        log: Vec<RawExchange>,
        /// Every call, in order.
        pub calls: Vec<Call>,
    }

    impl RecordingDriver {
        /// A disconnected mock for `profile`.
        pub fn new(profile: MachineProfile) -> Self {
            Self {
                profile,
                state: MachineState::Disconnected,
                position: (0.0, 0.0),
                params: BTreeMap::new(),
                log: Vec::new(),
                calls: Vec::new(),
            }
        }

        fn connected(&self) -> Result<()> {
            if self.state == MachineState::Disconnected {
                return Err(Error::new(
                    ErrorCode::DeviceUnreachable,
                    "Mock driver is not connected",
                    "connect() has not been called",
                    "call connect() first",
                ));
            }
            Ok(())
        }

        fn record(&mut self, call: Call) {
            self.log.push(RawExchange {
                sent: Vec::new(),
                received: Vec::new(),
                decoded: format!("{call:?}"),
            });
            self.calls.push(call);
        }
    }

    impl Driver for RecordingDriver {
        fn profile(&self) -> &MachineProfile {
            &self.profile
        }

        fn connect(&mut self) -> Result<()> {
            self.state = MachineState::Idle;
            self.record(Call::Connect);
            Ok(())
        }

        fn status(&mut self) -> Result<DriverStatus> {
            Ok(DriverStatus {
                state: self.state,
                position: (self.state != MachineState::Disconnected).then_some(self.position),
            })
        }

        fn read_param(&mut self, name: &str) -> Result<Vec<u8>> {
            self.connected()?;
            self.params.get(name).cloned().ok_or_else(|| {
                Error::new(
                    ErrorCode::Unsupported,
                    format!("Mock has no parameter \"{name}\""),
                    "it was never written",
                    "write it first with write_param()",
                )
            })
        }

        fn write_param(&mut self, name: &str, value: &[u8]) -> Result<()> {
            self.connected()?;
            self.params.insert(name.to_owned(), value.to_vec());
            self.record(Call::WriteParam(name.to_owned(), value.to_vec()));
            Ok(())
        }

        fn run_job(&mut self, job: &Job) -> Result<()> {
            self.connected()?;
            if let Some(last) = job.moves.last() {
                self.position = (last.x, last.y);
            }
            self.record(Call::RunJob(job.moves.len()));
            Ok(())
        }

        fn jog(&mut self, dx: f64, dy: f64) -> Result<()> {
            self.connected()?;
            self.position = (self.position.0 + dx, self.position.1 + dy);
            self.record(Call::Jog(dx, dy));
            Ok(())
        }

        fn trace_frame(&mut self, request: &FrameRequest) -> Result<()> {
            self.connected()?;
            self.record(Call::TraceFrame(request.clone()));
            Ok(())
        }

        fn pause(&mut self) -> Result<()> {
            self.connected()?;
            self.record(Call::Pause);
            Ok(())
        }

        fn resume(&mut self) -> Result<()> {
            self.connected()?;
            self.record(Call::Resume);
            Ok(())
        }

        fn stop(&mut self) -> Result<()> {
            self.record(Call::Stop);
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
        use crate::safety::{FramingSource, SafetyPolicy};

        fn profile() -> MachineProfile {
            MachineProfile::from_toml(
                "[machine]\nname='m'\ndriver='mock'\nlaser='fiber'\n\
                 [work_area]\nwidth_mm=110\nheight_mm=110\n\
                 [limits]\nmax_power_pct=100\nmax_speed_mm_s=4000\n\
                 [framing]\ndefault_source='guide_light'\nguide_light_port=8\n",
            )
            .unwrap()
        }

        fn request(source: FramingSource) -> FrameRequest {
            FrameRequest {
                outline: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
                source,
                continuous: true,
            }
        }

        #[test]
        fn frame_with_main_source_never_reaches_driver_without_opt_in() {
            let mut d = RecordingDriver::new(profile());
            d.connect().unwrap();
            let err = d
                .frame(
                    &request(FramingSource::MainSourceLowPower),
                    &SafetyPolicy::default(),
                )
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::SafetyBlocked);
            assert!(
                !d.calls.iter().any(|c| matches!(c, Call::TraceFrame(_))),
                "trace_frame must not run when the safety check fails"
            );
        }

        #[test]
        fn guide_light_frame_is_traced() {
            let mut d = RecordingDriver::new(profile());
            d.connect().unwrap();
            d.frame(
                &request(FramingSource::GuideLight),
                &SafetyPolicy::default(),
            )
            .unwrap();
            assert!(matches!(d.calls.last(), Some(Call::TraceFrame(_))));
        }

        #[test]
        fn calls_before_connect_fail() {
            let mut d = RecordingDriver::new(profile());
            assert_eq!(
                d.jog(1.0, 1.0).unwrap_err().code,
                ErrorCode::DeviceUnreachable
            );
        }
    }
}
