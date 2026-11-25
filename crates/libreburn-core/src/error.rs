// SPDX-License-Identifier: AGPL-3.0-or-later
//! Errors with stable codes (requirement NF-1).
//!
//! Every error says what failed, why, and what to try next. The code is stable
//! across releases so docs and bug reports can refer to it.

use std::fmt;

/// A stable, documented error code such as `LB-SAFETY-001`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// A machine profile could not be parsed or failed validation.
    ProfileInvalid,
    /// A safety rule blocked the action.
    SafetyBlocked,
    /// The driver could not reach the controller.
    DeviceUnreachable,
    /// The controller replied with something the driver did not expect.
    ProtocolViolation,
    /// The operation is not supported by this driver.
    Unsupported,
    /// A file could not be read or converted.
    ImportFailed,
    /// The file type is not one libreburn imports.
    ImportUnsupported,
    /// The project could not be turned into a job.
    PlanFailed,
}

impl ErrorCode {
    /// The stable string form of the code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProfileInvalid => "LB-PROFILE-001",
            Self::SafetyBlocked => "LB-SAFETY-001",
            Self::DeviceUnreachable => "LB-DEVICE-001",
            Self::ProtocolViolation => "LB-PROTO-001",
            Self::Unsupported => "LB-DRIVER-001",
            Self::ImportFailed => "LB-IMPORT-001",
            Self::ImportUnsupported => "LB-IMPORT-002",
            Self::PlanFailed => "LB-PLAN-001",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error type shared by core and drivers.
#[derive(Debug, thiserror::Error)]
#[error("[{code}] {what}: {why}. Try: {hint}")]
pub struct Error {
    /// Stable code.
    pub code: ErrorCode,
    /// What failed.
    pub what: String,
    /// Why it failed.
    pub why: String,
    /// What to try next.
    pub hint: String,
}

impl Error {
    /// Build an error from its parts.
    pub fn new(
        code: ErrorCode,
        what: impl Into<String>,
        why: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            code,
            what: what.into(),
            why: why.into(),
            hint: hint.into(),
        }
    }
}

/// Result alias for libreburn.
pub type Result<T> = std::result::Result<T, Error>;
