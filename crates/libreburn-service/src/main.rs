// SPDX-License-Identifier: AGPL-3.0-or-later
//! `libreburnd`: the libreburn service.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// libreburn service: owns machines and jobs and serves the API.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Address to listen on. Non-loopback addresses need --allow-remote and a token.
    #[arg(long, env = "LIBREBURN_LISTEN", default_value = "127.0.0.1:7878")]
    listen: SocketAddr,
    /// Allow listening on a non-loopback address.
    #[arg(long, env = "LIBREBURN_ALLOW_REMOTE")]
    allow_remote: bool,
    /// Machine profile to load at startup (TOML).
    #[arg(long, env = "LIBREBURN_PROFILE")]
    profile: Option<PathBuf>,
    /// Write logs as JSON lines (NF-3).
    #[arg(long)]
    json_logs: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    if args.json_logs {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let token = std::env::var("LIBREBURN_TOKEN").ok();
    if let Err(msg) =
        libreburn_service::check_bind(args.listen, args.allow_remote, token.as_deref())
    {
        tracing::error!("{msg}");
        return ExitCode::FAILURE;
    }

    let profile = match &args.profile {
        None => None,
        Some(path) => match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|t| libreburn_core::MachineProfile::from_toml(&t).map_err(|e| e.to_string()))
        {
            Ok(p) => {
                tracing::info!(profile = %p.machine.name, "loaded machine profile");
                Some(p)
            }
            Err(e) => {
                tracing::error!(
                    "[LB-SERVICE-004] Could not load profile {}: {e}",
                    path.display()
                );
                return ExitCode::FAILURE;
            }
        },
    };
    let state = libreburn_service::AppState::new(token, profile);

    let listener = match tokio::net::TcpListener::bind(args.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(
                "[LB-SERVICE-003] Could not listen on {}: {e}. Try: pick a free port with --listen",
                args.listen
            );
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(addr = %args.listen, "libreburnd listening");
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    if let Err(e) = axum::serve(listener, libreburn_service::router(state))
        .with_graceful_shutdown(shutdown)
        .await
    {
        tracing::error!("server error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
