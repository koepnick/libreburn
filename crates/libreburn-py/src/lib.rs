// SPDX-License-Identifier: AGPL-3.0-or-later
//! PyO3 bindings exposed to Python as `libreburn._core`.
//!
//! Keep `python/libreburn/src/libreburn/_core.pyi` in sync with this file;
//! `tools/check_stubs.py` fails CI when a function here has no stub.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// Version of the Rust core.
#[pyfunction]
fn core_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Parse and validate a machine profile, returning its display name.
#[pyfunction]
fn validate_profile(text: &str) -> PyResult<String> {
    libreburn_core::MachineProfile::from_toml(text)
        .map(|p| p.machine.name)
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Scramble bytes the way a Ruida controller expects.
#[pyfunction]
fn ruida_scramble(data: &[u8], magic: u8) -> Vec<u8> {
    data.iter()
        .map(|&b| libreburn_driver_ruida::protocol::scramble(b, magic))
        .collect()
}

/// Reverse `ruida_scramble`.
#[pyfunction]
fn ruida_unscramble(data: &[u8], magic: u8) -> Vec<u8> {
    data.iter()
        .map(|&b| libreburn_driver_ruida::protocol::unscramble(b, magic))
        .collect()
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(core_version, m)?)?;
    m.add_function(wrap_pyfunction!(validate_profile, m)?)?;
    m.add_function(wrap_pyfunction!(ruida_scramble, m)?)?;
    m.add_function(wrap_pyfunction!(ruida_unscramble, m)?)?;
    Ok(())
}
