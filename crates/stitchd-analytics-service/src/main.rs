//! `stitchd-analytics-service` — Analytics & Event Ingestion gRPC Service.
//!
//! Listens on `ANALYTICS_GRPC_PORT` / `EVENT_SERVICE_PORT` (default: 50054).
//! Serves all ClickHouse analytics RPCs and context-registry operations.
//! Prometheus metrics on `METRICS_PORT` (default: 9104).

use std::sync::Arc;

use anyhow::Context as _;
use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::signal;
use tonic::transport::Server;
use tonic_health::server::health_reporter;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use stitchd_analytics_service::{
    config::Config,
    grpc::{
        experiment_results::{ExperimentResultsRepository, RepoError, ResultRow, WriteResultInput},
        service::{AnalyticsServiceImpl, ServiceState},
    },
};
use stitchd_proto::analytics::v1::analytics_service_server::AnalyticsServiceServer;

// ---------------------------------------------------------------------------
// Temporary stub — remove once Worker 3's ClickHouse impl lands.
// TODO(merge): delete this struct and wire ChExperimentResultsRepository.
// ---------------------------------------------------------------------------

struct UnimplementedExperimentResultsRepo;

#[async_trait::async_trait]
impl ExperimentResultsRepository for UnimplementedExperimentResultsRepo {
    async fn write(&self, _input: &WriteResultInput) -> Result<ResultRow, RepoError> {
        Err(RepoError::Internal("experiment_results backend not yet wired".into()))
    }

    async fn list(
        &self,
        _experiment_id: uuid::Uuid,
        _iteration_id: Option<uuid::Uuid>,
    ) -> Result<Vec<ResultRow>, RepoError> {
        Err(RepoError::Internal("experiment_results backend not yet wired".into()))
    }

    async fn get(
        &self,
        _experiment_id: uuid::Uuid,
        _iteration_id: uuid::Uuid,
        _metric_key: &str,
    ) -> Result<ResultRow, RepoError> {
        Err(RepoError::Internal("experiment_results backend not yet wired".into()))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env()?;

    PrometheusBuilder::new()
        .with_http_listener(cfg.metrics_addr)
        .install()
        .context("Failed to install Prometheus metrics exporter")?;

    info!("Prometheus metrics listening on {}", cfg.metrics_addr);

    let mut ch_client = clickhouse::Client::default()
        .with_url(&cfg.clickhouse_url)
        .with_database(&cfg.clickhouse_db);

    if let Some(user) = &cfg.clickhouse_user {
        ch_client = ch_client.with_user(user);
    }
    if let Some(password) = &cfg.clickhouse_password {
        ch_client = ch_client.with_password(password);
    }

    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&cfg.database_url)
        .await
        .context("Failed to connect to PostgreSQL")?;

    let audit_logger = Arc::new(stitchd_db::PgAuditLogger::new(pg_pool.clone()));
    let event_def_repo: Arc<dyn stitchd_db::EventDefinitionRepository> = Arc::new(
        stitchd_db::PgEventDefinitionRepository::new(pg_pool.clone(), audit_logger.clone()),
    );
    let sdk_key_repo: Arc<dyn stitchd_db::SdkKeyRepository> = Arc::new(
        stitchd_db::PgSdkKeyRepository::new(pg_pool.clone(), audit_logger.clone()),
    );
    let context_registry: Arc<dyn stitchd_db::ContextRegistryRepository> =
        Arc::new(stitchd_db::PgContextRegistryRepository::new(pg_pool.clone()));
    let event_writer = stitchd_events::writer::EventWriter::new(ch_client.clone());

    // TODO(merge): Replace with the real ClickHouse-backed
    // `ChExperimentResultsRepository` once Worker 3's impl lands.
    // This stub satisfies the compiler during parallel development.
    let experiment_results_repo: Arc<dyn ExperimentResultsRepository> =
        Arc::new(UnimplementedExperimentResultsRepo);

    let state = ServiceState {
        pg_pool: Arc::new(pg_pool),
        ch_client: Arc::new(ch_client),
        event_def_repo,
        sdk_key_repo,
        event_writer,
        context_registry,
        experiment_results_repo,
    };
    let svc = AnalyticsServiceImpl::new(state);

    let (health_reporter, health_service) = health_reporter();
    health_reporter
        .set_serving::<AnalyticsServiceServer<AnalyticsServiceImpl>>()
        .await;

    info!("AnalyticsService gRPC listening on {}", cfg.grpc_addr);

    Server::builder()
        .add_service(health_service)
        .add_service(AnalyticsServiceServer::new(svc))
        .serve_with_shutdown(cfg.grpc_addr, shutdown_signal())
        .await
        .context("gRPC server failed")?;

    info!("AnalyticsService shut down gracefully");
    Ok(())
}

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

#[cfg(test)]
mod tests {
    #[test]
    fn config_requires_database_url() {
        // SAFETY: test-only; single-threaded test runner assumed.
        unsafe { std::env::remove_var("DATABASE_URL") };
        assert!(stitchd_analytics_service::config::Config::from_env().is_err());
    }
}
