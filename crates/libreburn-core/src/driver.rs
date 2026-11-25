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
