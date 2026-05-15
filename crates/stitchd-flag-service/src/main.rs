//! Entry point for the `stitchd-flag-service` gRPC microservice.
//!
//! The server listens on `FLAG_SERVICE_PORT` (default `50052`) and exposes
//! the [`FlagService`] gRPC API backed by a PostgreSQL database.
//!
//! # Environment variables
//!
//! | Variable             | Default                                    | Description                     |
//! |----------------------|--------------------------------------------|---------------------------------|
//! | `FLAG_SERVICE_PORT`  | `50052`                                    | gRPC server listen port         |
//! | `DATABASE_URL`       | *required*                                 | PostgreSQL connection string    |
//! | `CLICKHOUSE_URL`     | `http://localhost:8123`                    | ClickHouse HTTP endpoint        |
//! | `CLICKHOUSE_DB`      | `stitchd`                                  | ClickHouse database name        |

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context as _;
use clickhouse::Client as ChClient;
use metrics_exporter_prometheus::PrometheusBuilder;
use stitchd_db::{PgFlagRepository, PgSdkKeyRepository, PgSegmentRepository, PgVariantRepository};
use stitchd_flag_service::service::FlagServiceImpl;
use stitchd_proto::flags::v1::flag_service_server::FlagServiceServer;
use tonic::transport::Server;
use tonic_health::server::health_reporter;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Observability ──────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stitchd_flag_service=info,tonic=warn".parse().unwrap()),
        )
        .json()
        .init();

    // ── Metrics ────────────────────────────────────────────────────────────────
    let metrics_port: u16 = std::env::var("FLAG_METRICS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9052);
    let metrics_addr: SocketAddr = format!("0.0.0.0:{metrics_port}").parse().unwrap();
    PrometheusBuilder::new()
        .with_http_listener(metrics_addr)
        .install()
        .context("failed to install Prometheus recorder")?;

    // ── Database ───────────────────────────────────────────────────────────────
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .context("failed to connect to database")?;

    let audit_raw = Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(pool.clone()));
    let flag_repo = Arc::new(PgFlagRepository::new(pool.clone(), audit_raw.clone()));
    let variant_repo = Arc::new(PgVariantRepository::new(pool.clone(), audit_raw.clone()));
    let sdk_key_repo = Arc::new(PgSdkKeyRepository::new(pool.clone(), audit_raw.clone()));
    let segment_repo = Arc::new(PgSegmentRepository::new(pool, audit_raw.clone()));

    // ── ClickHouse (optional — evaluation telemetry) ───────────────────────────
    let ch_url = std::env::var("CLICKHOUSE_URL")
        .unwrap_or_else(|_| "http://localhost:8123".to_string());
    let ch_db = std::env::var("CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());
    let ch_client = Arc::new(ChClient::default().with_url(ch_url).with_database(ch_db));

    // ── gRPC Server ────────────────────────────────────────────────────────────
    let port: u16 = std::env::var("FLAG_SERVICE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(50052);

    let addr: SocketAddr = format!("0.0.0.0:{port}").parse().unwrap();
    let svc = FlagServiceImpl::new(flag_repo, variant_repo, sdk_key_repo, segment_repo)
        .with_clickhouse(ch_client);

    let (health_reporter, health_service) = health_reporter();
    health_reporter
        .set_serving::<FlagServiceServer<FlagServiceImpl>>()
        .await;

    info!("stitchd-flag-service listening on {addr}");

    Server::builder()
        .add_service(health_service)
        .add_service(FlagServiceServer::new(svc))
        .serve_with_shutdown(addr, shutdown_signal())
        .await
        .context("gRPC server error")?;

    Ok(())
}

/// Waits for SIGINT or SIGTERM and returns when either is received.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
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

    info!("shutdown signal received, stopping flag service");
}
