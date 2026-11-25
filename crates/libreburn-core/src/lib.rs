// SPDX-License-Identifier: AGPL-3.0-or-later
//! Core of libreburn: the device-neutral job model, machine profiles, the
//! driver interface every controller implements, and safety checks.
//!
//! Nothing in this crate talks to hardware. Drivers live in their own crates
//! and the service (`libreburnd`) is the only process that opens a device.

pub mod driver;
pub mod error;
pub mod geometry;
pub mod import;
pub mod job;
pub mod profile;
pub mod project;
pub mod raster;
pub mod safety;

pub use driver::{Driver, DriverStatus, FrameRequest, MachineState, RawExchange};
pub use error::{Error, ErrorCode, Result};
pub use import::{FileFormat, ImportOptions};
pub use job::{Job, Layer, LayerMode, Move, MoveKind, effective_power_pct};
pub use profile::{LaserType, MachineProfile};
pub use project::{HatchSettings, Object, ObjectKind, Project, ProjectLayer, RasterSettings};
pub use raster::{Dither, GrayImage};
pub use safety::{FramingSource, SafetyPolicy};
