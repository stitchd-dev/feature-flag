//! `stitchd-stats-service` — Scheduled Statistics Processing Service.
//!
//! Runs a periodic scheduler computing experiment results for all running experiments.
//! Exposes health and Prometheus metrics on `STATS_HTTP_PORT` (default: 9200).

use std::net::SocketAddr;

use anyhow::Context as _;
use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::signal;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use stitchd_stats_service::config::StatsConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Telemetry ────────────────────────────────────────────────────────────
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    // ── Config ───────────────────────────────────────────────────────────────
    let config = StatsConfig::from_env().context("Failed to load configuration")?;

    // ── Prometheus ───────────────────────────────────────────────────────────
    let prometheus_handle = PrometheusBuilder::new()
        .install_recorder()
        .context("Failed to install Prometheus metrics recorder")?;

    // ── Axum HTTP server (health + metrics) ──────────────────────────────────
    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(prometheus_handle);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.http_port));
    info!("Stats service HTTP listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("Failed to bind HTTP listener")?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")?;

    info!("Stats service shut down gracefully");
    Ok(())
}

async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn metrics_handler(
    State(handle): State<metrics_exporter_prometheus::PrometheusHandle>,
) -> impl IntoResponse {
    handle.render()
}

/// Resolves when SIGINT or SIGTERM is received.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    info!("Shutdown signal received");
}
