//! `stitchd-event-service` — Experimentation Event gRPC Service.
//!
//! Listens on `EVENT_SERVICE_PORT` (default: 50054).
//! Exposes `EventIngestionService` over gRPC (tonic).
//! Prometheus metrics are served on `METRICS_PORT` (default: 9104).

use std::{net::SocketAddr, sync::Arc};

use anyhow::Context as _;
use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::signal;
use tonic::transport::Server;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use stitchd_event_service::grpc::event_ingestion::{EventIngestionServiceImpl, ServiceState};
use stitchd_proto::events::v1::event_ingestion_service_server::EventIngestionServiceServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Telemetry ────────────────────────────────────────────────────────────
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    // ── Prometheus ───────────────────────────────────────────────────────────
    let metrics_port: u16 = std::env::var("METRICS_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9104);

    PrometheusBuilder::new()
        .with_http_listener(SocketAddr::from(([0, 0, 0, 0], metrics_port)))
        .install()
        .context("Failed to install Prometheus metrics exporter")?;

    info!("Prometheus metrics listening on :{metrics_port}");

    // ── Database connections ─────────────────────────────────────────────────
    let database_url =
        std::env::var("DATABASE_URL").context("DATABASE_URL environment variable is required")?;

    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .context("Failed to connect to PostgreSQL")?;

    let clickhouse_url = std::env::var("CLICKHOUSE_URL")
        .unwrap_or_else(|_| "http://localhost:8123".to_string());

    let ch_client = clickhouse::Client::default()
        .with_url(&clickhouse_url)
        .with_database("stitchd");

    let event_writer = stitchd_events::writer::EventWriter::new(ch_client);

    // ── Repositories ─────────────────────────────────────────────────────────
    let audit_logger = Arc::new(stitchd_db::PgAuditLogger::new(pg_pool.clone()));
    let event_def_repo: Arc<dyn stitchd_db::EventDefinitionRepository> = Arc::new(
        stitchd_db::PgEventDefinitionRepository::new(pg_pool.clone(), audit_logger.clone()),
    );
    let sdk_key_repo: Arc<dyn stitchd_db::SdkKeyRepository> =
        Arc::new(stitchd_db::PgSdkKeyRepository::new(pg_pool.clone(), audit_logger.clone()));

    let state = ServiceState {
        event_def_repo,
        sdk_key_repo,
        event_writer,
    };

    // ── gRPC server ──────────────────────────────────────────────────────────
    let service_port: u16 = std::env::var("EVENT_SERVICE_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50054);

    let addr = SocketAddr::from(([0, 0, 0, 0], service_port));
    info!("EventIngestionService listening on {addr}");

    Server::builder()
        .add_service(EventIngestionServiceServer::new(
            EventIngestionServiceImpl::new(state),
        ))
        .serve_with_shutdown(addr, shutdown_signal())
        .await
        .context("gRPC server failed")?;

    info!("EventIngestionService shut down gracefully");
    Ok(())
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
