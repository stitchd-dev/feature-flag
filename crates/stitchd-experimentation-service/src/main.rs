//! Stitchd Experimentation Service binary entry point.
//!
//! Starts a tonic gRPC server exposing `ExperimentationService`.
//!
//! Environment variables:
//! - `STITCHD_DATABASE_URL` — PostgreSQL connection string (required)
//! - `STITCHD_EXPERIMENTATION_SERVICE_GRPC_PORT` — gRPC listen port (default: `50055`)
//! - `STITCHD_FLAG_SERVICE_ADDR` — Flag Service gRPC address (default: `http://localhost:50052`)
//! - `STITCHD_ANALYTICS_SERVICE_GRPC_URL` — Analytics Service gRPC address (default: `http://localhost:50054`)

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context as _;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::postgres::PgPoolOptions;
use tonic::transport::Server;
use tonic_health::server::health_reporter;
use tracing_subscriber::{EnvFilter, fmt};

use stitchd_db::{PgAuditLogger, PgExperimentRepository, PgStatsScheduleRepository};
use stitchd_experimentation_service::{
    analytics_client::AnalyticsClient, flag_client::FlagClient, service::ExperimentationServiceImpl,
};
use stitchd_proto::experiments::v1::experimentation_service_server::ExperimentationServiceServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Tracing ─────────────────────────────────────────────────────────────
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    // ── Metrics ─────────────────────────────────────────────────────────────
    let metrics_port: u16 = std::env::var("STITCHD_EXPERIMENTATION_SERVICE_METRICS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9055);
    let metrics_addr: SocketAddr = format!("0.0.0.0:{metrics_port}").parse().unwrap();
    PrometheusBuilder::new()
        .with_http_listener(metrics_addr)
        .install()
        .context("install Prometheus metrics recorder")?;

    // ── Database ─────────────────────────────────────────────────────────────
    let database_url =
        std::env::var("STITCHD_DATABASE_URL").context("STITCHD_DATABASE_URL must be set")?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .context("connect to PostgreSQL")?;

    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let experiment_repo = Arc::new(PgExperimentRepository::new(pool.clone(), audit));
    let schedule_repo = Arc::new(PgStatsScheduleRepository::new(pool));

    // ── Analytics Service gRPC client ─────────────────────────────────────────
    let analytics_addr = std::env::var("STITCHD_ANALYTICS_SERVICE_GRPC_URL")
        .unwrap_or_else(|_| "http://localhost:50054".to_string());

    let analytics_client = AnalyticsClient::connect(analytics_addr.clone())
        .await
        .context("connect to Analytics Service")?;
    tracing::info!(addr = %analytics_addr, "Connected to Analytics Service");

    // ── Flag Service client ───────────────────────────────────────────────────
    let flag_service_addr = std::env::var("STITCHD_FLAG_SERVICE_ADDR")
        .unwrap_or_else(|_| "http://localhost:50052".to_string());

    let flag_client = match FlagClient::connect(flag_service_addr.clone()).await {
        Ok(fc) => {
            tracing::info!(addr = %flag_service_addr, "Connected to Flag Service");
            Some(fc)
        }
        Err(e) => {
            tracing::warn!(
                addr = %flag_service_addr,
                error = ?e,
                "Could not connect to Flag Service — flag verification will be skipped"
            );
            None
        }
    };

    // ── gRPC server ───────────────────────────────────────────────────────────
    let port: u16 = std::env::var("STITCHD_EXPERIMENTATION_SERVICE_GRPC_PORT")
        .unwrap_or_else(|_| "50055".to_string())
        .parse()
        .context("STITCHD_EXPERIMENTATION_SERVICE_GRPC_PORT must be a valid port number")?;

    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
    tracing::info!(addr = %addr, "Experimentation Service listening");

    let svc = ExperimentationServiceImpl::new(
        experiment_repo,
        Arc::new(analytics_client),
        schedule_repo,
        flag_client,
    );

    let (health_reporter, health_service) = health_reporter();
    health_reporter
        .set_serving::<ExperimentationServiceServer<ExperimentationServiceImpl>>()
        .await;

    Server::builder()
        .add_service(health_service)
        .add_service(ExperimentationServiceServer::new(svc))
        .serve_with_shutdown(addr, async {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to install CTRL+C signal handler");
            tracing::info!("Received shutdown signal — draining connections");
        })
        .await
        .context("gRPC server error")?;

    Ok(())
}
