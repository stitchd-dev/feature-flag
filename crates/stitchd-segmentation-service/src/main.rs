//! Segmentation Service binary entrypoint.
//!
//! Reads configuration from environment variables and starts the tonic gRPC server
//! with a Prometheus metrics endpoint.
//!
//! ## Environment Variables
//! - `SEGMENTATION_SERVICE_PORT` — gRPC listen port (default: `50053`)
//! - `SEGMENTATION_METRICS_PORT` — Prometheus metrics port (default: `9053`)
//! - `DATABASE_URL` — PostgreSQL connection string (required)
//! - `RUST_LOG` — log filter directive (default: `info`)

use std::sync::Arc;

use anyhow::Context;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::postgres::PgPoolOptions;
use tonic::transport::Server;
use tonic_health::server::health_reporter;
use tracing_subscriber::{EnvFilter, fmt};

use stitchd_db::{PgAuditLogger, PgSegmentRepository, SegmentRepository};
use stitchd_proto::segments::v1::segmentation_service_server::SegmentationServiceServer;
use stitchd_segmentation_service::grpc::service::{AppState, SegmentationServiceImpl};

/// Default gRPC listen port.
const DEFAULT_PORT: u16 = 50053;
/// Default Prometheus metrics port.
const DEFAULT_METRICS_PORT: u16 = 9053;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Logging ───────────────────────────────────────────────────────────────
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    // ── Configuration ─────────────────────────────────────────────────────────
    let port: u16 = std::env::var("SEGMENTATION_SERVICE_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let metrics_port: u16 = std::env::var("SEGMENTATION_METRICS_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_METRICS_PORT);

    let database_url =
        std::env::var("DATABASE_URL").context("DATABASE_URL environment variable is required")?;

    // ── Prometheus metrics ────────────────────────────────────────────────────
    let metrics_addr: std::net::SocketAddr = format!("0.0.0.0:{metrics_port}")
        .parse()
        .context("invalid metrics address")?;

    PrometheusBuilder::new()
        .with_http_listener(metrics_addr)
        .install()
        .context("failed to install Prometheus metrics exporter")?;

    tracing::info!(%metrics_addr, "prometheus metrics endpoint started");

    // ── Database ──────────────────────────────────────────────────────────────
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .context("failed to connect to PostgreSQL")?;

    let audit_logger = Arc::new(PgAuditLogger::new(pool.clone()));
    let segment_repo: Arc<dyn SegmentRepository> =
        Arc::new(PgSegmentRepository::new(pool.clone(), audit_logger));

    // ── gRPC server ───────────────────────────────────────────────────────────
    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}")
        .parse()
        .context("invalid gRPC listen address")?;

    let state = AppState { segment_repo };
    let service = SegmentationServiceImpl::new(state);
    let svc = SegmentationServiceServer::new(service);

    tracing::info!(%addr, "starting segmentation service gRPC server");

    let (health_reporter, health_service) = health_reporter();
    health_reporter
        .set_serving::<SegmentationServiceServer<SegmentationServiceImpl>>()
        .await;

    Server::builder()
        .add_service(health_service)
        .add_service(svc)
        .serve_with_shutdown(addr, shutdown_signal())
        .await
        .context("gRPC server error")?;

    tracing::info!("segmentation service shut down cleanly");

    Ok(())
}

/// Resolves when SIGTERM or Ctrl-C is received.
async fn shutdown_signal() {
    use tokio::signal;

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
        () = ctrl_c => tracing::info!("received Ctrl-C"),
        () = terminate => tracing::info!("received SIGTERM"),
    }
}
