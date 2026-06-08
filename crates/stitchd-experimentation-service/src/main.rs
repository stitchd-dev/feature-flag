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

use stitchd_core::util::grpc_connect::connect_with_retry_default;
use stitchd_db::{
    PgAuditLogger, PgExperimentRepository, PgStatsScheduleRepository,
    repository::pg::{PgExclusionGroupRepository, PgFlagRepository},
};
use stitchd_experimentation_service::{
    analytics_client::AnalyticsClient, exposure_reader::ClickHouseExposureReader,
    flag_client::FlagClient, service::ExperimentationServiceImpl,
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
    let experiment_repo = Arc::new(PgExperimentRepository::new(pool.clone(), audit.clone()));
    let exclusion_group_repo =
        Arc::new(PgExclusionGroupRepository::new(pool.clone(), audit.clone()));
    let flag_repo = Arc::new(PgFlagRepository::new(pool.clone(), audit));
    let schedule_repo = Arc::new(PgStatsScheduleRepository::new(pool.clone()));

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

    // Start-time prerequisite gate (Phase 5 + Phase 10): repo over
    // `experiment_start_prerequisites` + a resolver. `experiment_done` is backed
    // by the experiment repo; `flag_variant` resolves the live served variant via
    // the Flag Service (exact UUID comparison) when a Flag Service connection is
    // available — falling back to fail-closed only when it is not.
    let start_prereq_repo = Arc::new(
        stitchd_experimentation_service::start_prerequisites::PgStartPrerequisiteRepository::new(
            pool.clone(),
        ),
    );
    let served_variant_lookup: Arc<
        dyn stitchd_experimentation_service::start_prerequisites::ServedVariantLookup,
    > = match flag_client.clone() {
        Some(fc) => Arc::new(
            stitchd_experimentation_service::start_prerequisites::FlagServiceServedVariantLookup::new(
                flag_repo.clone(),
                fc,
            ),
        ),
        None => Arc::new(
            stitchd_experimentation_service::start_prerequisites::UnavailableServedVariantLookup,
        ),
    };
    let start_prereq_resolver = Arc::new(
        stitchd_experimentation_service::start_prerequisites::ServiceStartPrerequisiteResolver::new(
            experiment_repo.clone(),
            served_variant_lookup,
        ),
    );

    // ── Analytics Service gRPC client ─────────────────────────────────────────
    let analytics_addr = std::env::var("STITCHD_ANALYTICS_SERVICE_GRPC_URL")
        .unwrap_or_else(|_| "http://localhost:50054".to_string());

    let analytics_client = connect_with_retry_default("Analytics Service", &analytics_addr, || {
        let addr = analytics_addr.clone();
        async move { AnalyticsClient::connect(addr).await }
    })
    .await
    .context("connect to Analytics Service")?;
    tracing::info!(addr = %analytics_addr, "Connected to Analytics Service");

    // ── gRPC server ───────────────────────────────────────────────────────────
    let port: u16 = std::env::var("STITCHD_EXPERIMENTATION_SERVICE_GRPC_PORT")
        .unwrap_or_else(|_| "50055".to_string())
        .parse()
        .context("STITCHD_EXPERIMENTATION_SERVICE_GRPC_PORT must be a valid port number")?;

    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
    tracing::info!(addr = %addr, "Experimentation Service listening");

    // ── ClickHouse client (for exposure reads) ───────────────────────────────
    let ch_url = std::env::var("STITCHD_CLICKHOUSE_URL")
        .unwrap_or_else(|_| "http://localhost:8123".to_string());
    let ch_db = std::env::var("STITCHD_CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());
    let ch_user =
        std::env::var("STITCHD_CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_string());
    let ch_password = std::env::var("STITCHD_CLICKHOUSE_PASSWORD").unwrap_or_default();
    let ch_client = Arc::new(
        clickhouse::Client::default()
            .with_url(&ch_url)
            .with_database(&ch_db)
            .with_user(ch_user)
            .with_password(ch_password),
    );
    let exposure_reader = Arc::new(ClickHouseExposureReader::new(ch_client.clone()));
    let interactions_reader = Arc::new(
        stitchd_experimentation_service::interactions_reader::ClickHouseInteractionsReader::new(
            ch_client,
        ),
    );
    tracing::info!(url = %ch_url, db = %ch_db, "ClickHouse exposure + interactions readers configured");

    let svc = ExperimentationServiceImpl::new(
        experiment_repo,
        Arc::new(analytics_client),
        schedule_repo,
        flag_client,
    )
    .with_exposure_reader(exposure_reader)
    .with_interactions_reader(interactions_reader)
    .with_exclusion_groups(exclusion_group_repo, flag_repo)
    .with_start_prerequisites(start_prereq_repo, start_prereq_resolver);

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
