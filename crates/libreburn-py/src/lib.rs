// SPDX-License-Identifier: AGPL-3.0-or-later
//! PyO3 bindings exposed to Python as `libreburn._core`.
//!
//! Keep `python/libreburn/src/libreburn/_core.pyi` in sync with this file;
//! `tools/check_stubs.py` fails CI when a function here has no stub.

use libreburn_core::{FrameShape, ImportOptions, MachineProfile, PlanOptions, Project};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use serde::de::DeserializeOwned;

fn err(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(e.to_string())
}

/// Parse JSON input, naming the argument in the error. Empty text means defaults.
fn from_json<T: DeserializeOwned + Default>(text: &str, what: &str) -> PyResult<T> {
    if text.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(text).map_err(|e| {
        PyValueError::new_err(format!(
            "[LB-API-001] Could not read {what}: {e}. Try: pass a JSON object"
        ))
    })
}

fn to_json(value: &impl serde::Serialize) -> PyResult<String> {
    serde_json::to_string(value).map_err(err)
}

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

/// Import a file into a project, offline. Takes and returns project JSON
/// (empty text for a new project); also returns the import report as JSON.
#[pyfunction]
#[pyo3(signature = (project_json, data, name, options_json = ""))]
fn project_import(
    py: Python<'_>,
    project_json: &str,
    data: &[u8],
    name: &str,
    options_json: &str,
) -> PyResult<(String, String)> {
    let mut project: Project = from_json(project_json, "the project")?;
    let options: ImportOptions = from_json(options_json, "the import options")?;
    let report = py
        .detach(|| project.import(data, name, &options))
        .map_err(err)?;
    Ok((to_json(&project)?, to_json(&report)?))
}

/// Plan a project for a machine profile, offline. Returns the plan as JSON:
/// `{"job": ..., "estimate": ..., "warnings": [...]}`.
#[pyfunction]
#[pyo3(signature = (project_json, profile_toml, options_json = ""))]
fn plan_project(
    py: Python<'_>,
    project_json: &str,
    profile_toml: &str,
    options_json: &str,
) -> PyResult<String> {
    let project: Project = from_json(project_json, "the project")?;
    let options: PlanOptions = from_json(options_json, "the plan options")?;
    let profile = MachineProfile::from_toml(profile_toml).map_err(err)?;
    let plan = py
        .detach(|| libreburn_core::plan(&project, &profile, &options))
        .map_err(err)?;
    to_json(&plan)
}

/// The framing outline of a job (JSON), as `bounding_box` or `hull`.
#[pyfunction]
#[pyo3(signature = (job_json, shape = "bounding_box"))]
fn frame_outline(job_json: &str, shape: &str) -> PyResult<Vec<(f64, f64)>> {
    let job: libreburn_core::Job = from_json(job_json, "the job")?;
    let shape: FrameShape = serde_json::from_value(serde_json::Value::String(shape.into()))
        .map_err(|_| {
            PyValueError::new_err(format!(
                "[LB-API-001] Unknown frame shape \"{shape}\". Try: \"bounding_box\" or \"hull\""
            ))
        })?;
    Ok(libreburn_core::frame_outline(&job, shape))
}

/// Dither 8-bit grayscale pixels (0 black, 255 white). `mode_json` is a
/// [`libreburn_core::Dither`] such as `{"kind": "floyd_steinberg"}`.
/// Returns one byte per pixel: 1 burn, 0 skip.
#[pyfunction]
#[pyo3(signature = (width, height, pixels, mode_json = ""))]
fn dither(width: u32, height: u32, pixels: &[u8], mode_json: &str) -> PyResult<Vec<u8>> {
    let image = libreburn_core::GrayImage::new(width, height, pixels.to_vec()).map_err(err)?;
    let mode: libreburn_core::Dither = from_json(mode_json, "the dither mode")?;
    Ok(libreburn_core::raster::dither(&image, mode)
        .dots
        .into_iter()
        .map(u8::from)
        .collect())
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(core_version, m)?)?;
    m.add_function(wrap_pyfunction!(validate_profile, m)?)?;
    m.add_function(wrap_pyfunction!(ruida_scramble, m)?)?;
    m.add_function(wrap_pyfunction!(ruida_unscramble, m)?)?;
    m.add_function(wrap_pyfunction!(project_import, m)?)?;
    m.add_function(wrap_pyfunction!(plan_project, m)?)?;
    m.add_function(wrap_pyfunction!(frame_outline, m)?)?;
    m.add_function(wrap_pyfunction!(dither, m)?)?;
    Ok(())
}
