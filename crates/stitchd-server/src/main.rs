//! Stitchd server binary entry point.
use anyhow::{Context, Result};
use stitchd_server::{build_router, telemetry};
use tracing::info;

const SERVICE_NAME: &str = "stitchd-server";
const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<()> {
    // Metrics must be installed before tracing so the metrics layer can record
    // any spans emitted during subscriber setup.
    let metrics_handle = telemetry::init_metrics()
        .context("failed to initialise metrics")?;

    let tracer_provider = telemetry::init_tracing(SERVICE_NAME, SERVICE_VERSION)
        .context("failed to initialise tracing")?;

    info!(
        service = SERVICE_NAME,
        version = SERVICE_VERSION,
        "stitchd-server starting"
    );

    let http_port: u16 = std::env::var("HTTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], http_port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind to {addr}"))?;

    info!(address = %addr, "HTTP server listening");

    let app = build_router(metrics_handle);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server error")?;

    info!("shutting down");
    telemetry::shutdown_tracing(tracer_provider);

    Ok(())
}

/// Resolves when SIGTERM or Ctrl-C is received.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = sigterm => {},
    }

    info!("shutdown signal received");
}
