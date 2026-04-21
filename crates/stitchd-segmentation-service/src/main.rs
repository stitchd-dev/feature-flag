//! Segmentation Service binary entrypoint.
//!
//! Reads configuration from environment variables and starts the tonic gRPC server.

use anyhow::Context;
use tracing_subscriber::{EnvFilter, fmt};

/// Port env var (default 50053).
const DEFAULT_PORT: u16 = 50053;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialise structured logging.
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    let port: u16 = std::env::var("SEGMENTATION_SERVICE_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}")
        .parse()
        .context("invalid listen address")?;

    tracing::info!(%addr, "starting segmentation service");

    tracing::warn!("segmentation service started in scaffold mode; no DB connection configured");

    // Graceful shutdown signal.
    tokio::signal::ctrl_c()
        .await
        .context("failed to listen for ctrl-c")?;

    tracing::info!("shutting down segmentation service");

    Ok(())
}
