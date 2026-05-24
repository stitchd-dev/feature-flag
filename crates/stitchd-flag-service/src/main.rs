//! Entry point for the `stitchd-flag-service` gRPC microservice.
//!
//! The server listens on `STITCHD_FLAG_SERVICE_GRPC_PORT` (default `50052`) and exposes
//! the [`FlagService`] gRPC API backed by a PostgreSQL database.
//!
//! # Environment variables
//!
//! | Variable             | Default                                    | Description                     |
//! |----------------------|--------------------------------------------|---------------------------------|
//! | `STITCHD_FLAG_SERVICE_GRPC_PORT`  | `50052`                                    | gRPC server listen port         |
//! | `STITCHD_DATABASE_URL`       | *required*                                 | PostgreSQL connection string    |
//! | `STITCHD_CLICKHOUSE_URL`     | `http://localhost:8123`                    | ClickHouse HTTP endpoint        |
//! | `STITCHD_CLICKHOUSE_DB`      | `stitchd`                                  | ClickHouse database name        |
//! | `STITCHD_CLICKHOUSE_USER`    | `default`                                  | ClickHouse username             |
//! | `STITCHD_CLICKHOUSE_PASSWORD`| *(empty)*                                  | ClickHouse password             |

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context as _;
use clickhouse::Client as ChClient;
use metrics_exporter_prometheus::PrometheusBuilder;
use stitchd_db::{
    CompositeSegmentRepository, PgEventDefinitionRepository, PgExperimentRepository,
    PgFlagRepository, PgSdkKeyRepository, PgSegmentRepository, PgVariantRepository,
    scylla::{ScyllaConfig, segment::ScyllaSegmentStore},
};
use stitchd_flag_service::sdk_backend::FlagSdkBackendServiceImpl;
use stitchd_flag_service::service::FlagServiceImpl;
use stitchd_proto::flags::v1::flag_service_server::FlagServiceServer;
use stitchd_proto::sdk::v1::flag_sdk_backend_service_server::FlagSdkBackendServiceServer;
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
    let metrics_port: u16 = std::env::var("STITCHD_FLAG_SERVICE_METRICS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9052);
    let metrics_addr: SocketAddr = format!("0.0.0.0:{metrics_port}").parse().unwrap();
    PrometheusBuilder::new()
        .with_http_listener(metrics_addr)
        .install()
        .context("failed to install Prometheus recorder")?;

    // ── Database ───────────────────────────────────────────────────────────────
    let database_url =
        std::env::var("STITCHD_DATABASE_URL").context("STITCHD_DATABASE_URL is required")?;
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .context("failed to connect to database")?;

    let audit_raw = Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(pool.clone()));
    let flag_repo: Arc<dyn stitchd_db::FlagRepository> =
        Arc::new(PgFlagRepository::new(pool.clone(), audit_raw.clone()));
    let variant_repo: Arc<dyn stitchd_db::VariantRepository> =
        Arc::new(PgVariantRepository::new(pool.clone(), audit_raw.clone()));
    let sdk_key_repo: Arc<dyn stitchd_db::SdkKeyRepository> =
        Arc::new(PgSdkKeyRepository::new(pool.clone(), audit_raw.clone()));
    // Experiment repo backs the whole-flag lock helper (`is_flag_locked`).
    let experiment_repo: Arc<dyn stitchd_db::ExperimentRepository> =
        Arc::new(PgExperimentRepository::new(pool.clone(), audit_raw.clone()));
    // ── ScyllaDB (list-segment membership reads) ──────────────────────────────
    let scylla_config = ScyllaConfig::from_env();
    tracing::info!(
        uri = %scylla_config.uri,
        keyspace = %scylla_config.keyspace,
        "connecting to ScyllaDB for list-segment membership"
    );
    let scylla_client = stitchd_db::scylla::ScyllaClient::connect(&scylla_config)
        .await
        .context("failed to connect to ScyllaDB")?;
    let scylla_store = Arc::new(ScyllaSegmentStore::new(scylla_client));

    let pg_segment_repo = Arc::new(PgSegmentRepository::new(pool.clone(), audit_raw.clone()));
    let segment_repo: Arc<dyn stitchd_db::SegmentRepository> = Arc::new(
        CompositeSegmentRepository::new(pg_segment_repo, scylla_store),
    );

    // Event-definition repo — supplies the SDK's track() validation cache
    // via SyncDefinitions.
    let event_definition_repo: Arc<dyn stitchd_db::EventDefinitionRepository> =
        Arc::new(PgEventDefinitionRepository::new(pool, audit_raw.clone()));

    // ── ClickHouse (optional — evaluation telemetry) ───────────────────────────
    let ch_url = std::env::var("STITCHD_CLICKHOUSE_URL")
        .unwrap_or_else(|_| "http://localhost:8123".to_string());
    let ch_db = std::env::var("STITCHD_CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());
    let ch_user =
        std::env::var("STITCHD_CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_string());
    let ch_password = std::env::var("STITCHD_CLICKHOUSE_PASSWORD").unwrap_or_default();
    let ch_client = Arc::new(
        ChClient::default()
            .with_url(ch_url)
            .with_database(ch_db)
            .with_user(ch_user)
            .with_password(ch_password),
    );

    stitchd_event_writer::migrations::run(&ch_client)
        .await
        .expect("Failed to run ClickHouse migrations");

    // ── gRPC Server ────────────────────────────────────────────────────────────
    let port: u16 = std::env::var("STITCHD_FLAG_SERVICE_GRPC_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(50052);

    let addr: SocketAddr = format!("0.0.0.0:{port}").parse().unwrap();
    let svc = FlagServiceImpl::new(
        Arc::clone(&flag_repo),
        Arc::clone(&variant_repo),
        sdk_key_repo,
        Arc::clone(&segment_repo),
    )
    .with_clickhouse(Arc::clone(&ch_client))
    .with_experiment_repo(Arc::clone(&experiment_repo));

    let sdk_backend_svc = FlagSdkBackendServiceImpl::new(
        Arc::clone(&flag_repo),
        Arc::clone(&variant_repo),
        Arc::clone(&segment_repo),
    )
    .with_clickhouse(ch_client)
    .with_event_definition_repo(Arc::clone(&event_definition_repo));

    let (health_reporter, health_service) = health_reporter();
    health_reporter
        .set_serving::<FlagServiceServer<FlagServiceImpl>>()
        .await;

    info!("stitchd-flag-service listening on {addr}");

    Server::builder()
        .add_service(health_service)
        .add_service(FlagServiceServer::new(svc))
        .add_service(FlagSdkBackendServiceServer::new(sdk_backend_svc))
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
