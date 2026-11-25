// SPDX-License-Identifier: AGPL-3.0-or-later
//! The libreburn service library: API routes and startup checks.
//!
//! The service is the only process that talks to a controller (AR-1). It
//! listens on localhost unless remote access is enabled with a token (AR-4).
//! Endpoints are listed in `docs/api.md`.

pub mod api;

use std::net::SocketAddr;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::{Json, Router, routing::get};
use serde::Serialize;

pub use api::AppState;

/// Version and build information returned by `GET /api/v1/version`.
#[derive(Debug, Serialize)]
pub struct VersionInfo {
    /// Crate version.
    pub version: &'static str,
    /// API version.
    pub api: &'static str,
    /// Target architecture.
    pub arch: &'static str,
}

/// Build the API router around `state`.
pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route(
            "/api/v1/version",
            get(|| async {
                Json(VersionInfo {
                    version: env!("CARGO_PKG_VERSION"),
                    api: "v1",
                    arch: std::env::consts::ARCH,
                })
            }),
        )
        .merge(api::routes())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state);
    Router::new()
        .route("/api/v1/health", get(|| async { "ok" }))
        .merge(protected)
        .layer(DefaultBodyLimit::max(api::MAX_UPLOAD_BYTES))
}

/// Compare without leaking the length of the matching prefix through timing.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(token) = state.token() else {
        return next.run(req).await;
    };
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    // Browsers cannot set headers on WebSocket upgrades, so allow ?token= too.
    let query = req
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")));
    if bearer
        .or(query)
        .is_some_and(|t| same(t.as_bytes(), token.as_bytes()))
    {
        return next.run(req).await;
    }
    api::error_response(
        StatusCode::UNAUTHORIZED,
        "Request was not authorized",
        "remote access is on and the request did not carry the service token",
        "send `Authorization: Bearer <LIBREBURN_TOKEN>`",
    )
}

/// Check the listen address against the remote-access setting (AR-4).
pub fn check_bind(addr: SocketAddr, allow_remote: bool, token: Option<&str>) -> Result<(), String> {
    if addr.ip().is_loopback() {
        return Ok(());
    }
    if !allow_remote {
        return Err(format!(
            "[LB-SERVICE-001] Refusing to listen on {addr}: remote access is off. \
             Try: listen on 127.0.0.1, or pass --allow-remote with LIBREBURN_TOKEN set"
        ));
    }
    match token {
        Some(t) if t.len() >= 16 => Ok(()),
        _ => Err(format!(
            "[LB-SERVICE-002] Refusing to listen on {addr}: remote access needs a token. \
             Try: set LIBREBURN_TOKEN to a random string of at least 16 characters"
        )),
    }
}

#[cfg(test)]
mod tests;
