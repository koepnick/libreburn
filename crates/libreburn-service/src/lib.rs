// SPDX-License-Identifier: AGPL-3.0-or-later
//! The libreburn service library: API routes and startup checks.
//!
//! The service is the only process that talks to a controller (AR-1). It
//! listens on localhost unless remote access is enabled with a token (AR-4).

use std::net::SocketAddr;

use axum::{Json, Router, routing::get};
use serde::Serialize;

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

/// Build the API router.
pub fn router() -> Router {
    Router::new()
        .route("/api/v1/health", get(|| async { "ok" }))
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
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let res = router()
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }

    #[test]
    fn localhost_is_always_allowed() {
        assert!(check_bind("127.0.0.1:7878".parse().unwrap(), false, None).is_ok());
    }

    #[test]
    fn remote_needs_opt_in_and_token() {
        let addr = "0.0.0.0:7878".parse().unwrap();
        assert!(check_bind(addr, false, Some("0123456789abcdef")).is_err());
        assert!(check_bind(addr, true, None).is_err());
        assert!(check_bind(addr, true, Some("short")).is_err());
        assert!(check_bind(addr, true, Some("0123456789abcdef")).is_ok());
    }
}
