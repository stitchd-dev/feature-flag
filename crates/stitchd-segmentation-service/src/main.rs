//! Segmentation Service binary entrypoint.
//!
//! Reads configuration from environment variables and starts the tonic gRPC server
//! with a Prometheus metrics endpoint.
//!
//! ## Environment Variables
//! - `STITCHD_SEGMENTATION_SERVICE_GRPC_PORT` — gRPC listen port (default: `50053`)
//! - `STITCHD_SEGMENTATION_SERVICE_METRICS_PORT` — Prometheus metrics port (default: `9053`)
//! - `STITCHD_DATABASE_URL` — PostgreSQL connection string (required)
//! - `STITCHD_SCYLLA_URI` — ScyllaDB contact point (default: `127.0.0.1:9042`)
//! - `STITCHD_SCYLLA_KEYSPACE` — ScyllaDB keyspace (default: `stitchd`)
//! - `RUST_LOG` — log filter directive (default: `info`)

use std::sync::Arc;

use anyhow::Context;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::postgres::PgPoolOptions;
use tonic::transport::Server;
use tonic_health::server::health_reporter;
use tracing_subscriber::{EnvFilter, fmt};

use stitchd_db::scylla::{ScyllaConfig, migrate as scylla_migrate, segment::ScyllaSegmentStore};
use stitchd_db::{
    CompositeSegmentRepository, PgAuditLogger, PgSegmentRepository, SegmentRepository,
};
use stitchd_proto::sdk::v1::segmentation_sdk_backend_service_server::SegmentationSdkBackendServiceServer;
use stitchd_proto::segments::v1::segmentation_service_server::SegmentationServiceServer;
use stitchd_segmentation_service::grpc::sdk_backend::SegmentationSdkBackendServiceImpl;
use stitchd_segmentation_service::grpc::service::{AppState, SegmentationServiceImpl};
use stitchd_segmentation_service::sweeper::GenerationSweeper;

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
    let port: u16 = std::env::var("STITCHD_SEGMENTATION_SERVICE_GRPC_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let metrics_port: u16 = std::env::var("STITCHD_SEGMENTATION_SERVICE_METRICS_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_METRICS_PORT);

    let database_url =
        std::env::var("STITCHD_DATABASE_URL").context("STITCHD_DATABASE_URL environment variable is required")?;

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
    let pg_segment_repo = Arc::new(PgSegmentRepository::new(pool.clone(), audit_logger));

    // ── ScyllaDB ──────────────────────────────────────────────────────────────
    let scylla_config = ScyllaConfig::from_env();
    tracing::info!(uri = %scylla_config.uri, keyspace = %scylla_config.keyspace, "connecting to ScyllaDB");

    let scylla_client = stitchd_db::scylla::ScyllaClient::connect(&scylla_config)
        .await
        .context("failed to connect to ScyllaDB")?;

    let migrations_dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/stitchd-db/scylla-migrations"
    );
    scylla_migrate::run(&scylla_client, migrations_dir)
        .await
        .context("ScyllaDB migrations failed")?;
    tracing::info!("ScyllaDB migrations applied");

    // Clone the client before moving into ScyllaSegmentStore — the clone is
    // cheap because `ScyllaClient` wraps an `Arc<Session>` internally.
    let metrics_poll_client = scylla_client.clone();
    let scylla_store = Arc::new(ScyllaSegmentStore::new(scylla_client));

    // ── Scylla metrics polling ────────────────────────────────────────────────
    // Polls the Scylla driver's internal metrics every 15 s and emits them as
    // Prometheus gauges via the `metrics` crate.
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
        loop {
            interval.tick().await;
            metrics_poll_client.record_metrics().await;
        }
    });
    tracing::info!("scylla metrics polling started (interval=15s)");

    // ── Generation sweeper ────────────────────────────────────────────────────
    // Reads retention/interval from environment with sensible defaults.
    let sweeper_retention_secs: u64 = std::env::var("STITCHD_SEGMENTATION_SWEEPER_RETENTION_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(24 * 3600); // default: 24 hours
    let sweeper_interval_secs: u64 = std::env::var("STITCHD_SEGMENTATION_SWEEPER_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3600); // default: run every hour

    // Clone is cheap — ScyllaSegmentStore wraps an Arc<Session> internally.
    let sweeper_store = (*scylla_store).clone();
    tokio::spawn(async move {
        let sweeper = GenerationSweeper::new(
            sweeper_store,
            std::time::Duration::from_secs(sweeper_retention_secs),
        );
        sweeper
            .run(std::time::Duration::from_secs(sweeper_interval_secs))
            .await;
    });
    tracing::info!(
        retention_secs = sweeper_retention_secs,
        interval_secs = sweeper_interval_secs,
        "generation sweeper started"
    );

    // Composite repository: PG for metadata, Scylla for list-entry operations.
    let segment_repo: Arc<dyn SegmentRepository> = Arc::new(CompositeSegmentRepository::new(
        pg_segment_repo.clone(),
        scylla_store,
    ));

    // ── gRPC server ───────────────────────────────────────────────────────────
    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}")
        .parse()
        .context("invalid gRPC listen address")?;

    let state = AppState {
        segment_repo: Arc::clone(&segment_repo),
    };
    let service = SegmentationServiceImpl::new(state);
    let svc = SegmentationServiceServer::new(service);

    let sdk_backend_svc = SegmentationSdkBackendServiceServer::new(
        SegmentationSdkBackendServiceImpl::new(segment_repo),
    );

    tracing::info!(%addr, "starting segmentation service gRPC server");

    let (health_reporter, health_service) = health_reporter();
    health_reporter
        .set_serving::<SegmentationServiceServer<SegmentationServiceImpl>>()
        .await;

    Server::builder()
        .add_service(health_service)
        .add_service(svc)
        .add_service(sdk_backend_svc)
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
