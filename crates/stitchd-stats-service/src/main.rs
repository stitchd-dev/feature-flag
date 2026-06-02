//! `stitchd-stats-service` — Scheduled Statistics Processing Service.
//!
//! Runs a periodic scheduler computing experiment results for all running experiments.
//! Exposes health and Prometheus metrics on `STITCHD_STATS_SERVICE_HTTP_PORT` (default: 9200).
//! Exposes gRPC `StatsService` on `STITCHD_STATS_SERVICE_GRPC_PORT` (default: 50056).
//!
//! ## gRPC Clients
//! - `STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL` — experimentation-service endpoint
//!   (default: `http://localhost:50054`).
//! - `STITCHD_ANALYTICS_SERVICE_GRPC_URL` — analytics-service endpoint
//!   (default: `http://localhost:50055`).

use std::net::SocketAddr;

use std::sync::Arc;

use anyhow::Context as _;
use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use chrono::Duration;
use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::signal;
use tokio::sync::Mutex;
use tonic::transport::{Channel, Server};
use tonic_health::server::health_reporter;
use tracing::{error, info, warn};

use stitchd_core::util::grpc_connect::connect_with_retry_default;
use stitchd_db::PgContextRegistryRepository;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use stitchd_proto::analytics::v1::analytics_service_client::AnalyticsServiceClient;
use stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient;
use stitchd_proto::stats::v1::stats_service_server::StatsServiceServer;
use stitchd_stats_service::{
    config::StatsConfig,
    context_refresher::{ClickHouseEvalLogSource, ContextRegistryRefresher},
    grpc::service::StatsServiceImpl,
    results_writer::write_results,
    schedule_updater::update_schedule_after_run,
    scheduler::fetch_running_experiments,
};

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

    // ── Database connections ──────────────────────────────────────────────────
    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .context("Failed to connect to PostgreSQL")?;

    // ── ClickHouse client ─────────────────────────────────────────────────────
    let ch_user =
        std::env::var("STITCHD_CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_string());
    let ch_password = std::env::var("STITCHD_CLICKHOUSE_PASSWORD").unwrap_or_default();
    let ch_client = clickhouse::Client::default()
        .with_url(&config.clickhouse_url)
        .with_database(&config.clickhouse_db)
        .with_user(ch_user)
        .with_password(ch_password);

    // ── gRPC clients ──────────────────────────────────────────────────────────
    info!(
        url = %config.experimentation_service_grpc_url,
        "Connecting to experimentation-service gRPC"
    );
    let exp_endpoint = Channel::from_shared(config.experimentation_service_grpc_url.clone())
        .context("Invalid STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL")?;
    let exp_channel = connect_with_retry_default(
        "experimentation-service",
        &config.experimentation_service_grpc_url,
        || {
            let endpoint = exp_endpoint.clone();
            async move { endpoint.connect().await }
        },
    )
    .await
    .context("Failed to connect to experimentation-service gRPC")?;
    let exp_client = Arc::new(Mutex::new(ExperimentationServiceClient::new(exp_channel)));

    info!(
        url = %config.analytics_service_grpc_url,
        "Connecting to analytics-service gRPC"
    );
    let analytics_endpoint = Channel::from_shared(config.analytics_service_grpc_url.clone())
        .context("Invalid STITCHD_ANALYTICS_SERVICE_GRPC_URL")?;
    let analytics_channel = connect_with_retry_default(
        "analytics-service",
        &config.analytics_service_grpc_url,
        || {
            let endpoint = analytics_endpoint.clone();
            async move { endpoint.connect().await }
        },
    )
    .await
    .context("Failed to connect to analytics-service gRPC")?;
    let analytics_client = Arc::new(Mutex::new(AnalyticsServiceClient::new(analytics_channel)));

    // ── Context registry refresh loop (15-min cadence) ────────────────────────
    let registry_pool = pg_pool.clone();
    let registry_ch_client = ch_client.clone();
    tokio::spawn(async move {
        let refresher = ContextRegistryRefresher::new(
            Arc::new(ClickHouseEvalLogSource::new(registry_ch_client)),
            Arc::new(PgContextRegistryRepository::new(registry_pool)),
        );
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(900)); // 15 min
        loop {
            ticker.tick().await;
            if let Err(e) = refresher.run().await {
                error!("Context registry refresh failed: {e}");
            }
        }
    });

    // ── Repositories + interaction CH reader/writer ───────────────────────────
    use stitchd_db::{PgExperimentRepository, PgMetricRepository};
    use stitchd_stats_service::interaction_compute::{
        ClickHouseInteractionCells, ClickHouseInteractionWriter, run_interaction_sweep,
    };
    let audit: Arc<dyn stitchd_db::AuditLogger> =
        Arc::new(stitchd_db::PgAuditLogger::new(pg_pool.clone()));
    let metric_repo: Arc<dyn stitchd_db::MetricRepository> =
        Arc::new(PgMetricRepository::new(pg_pool.clone(), audit.clone()));
    let experiment_repo: Arc<dyn stitchd_db::ExperimentRepository> =
        Arc::new(PgExperimentRepository::new(pg_pool.clone(), audit.clone()));
    let interaction_cells: Arc<dyn stitchd_stats_service::interaction_compute::InteractionCellReader> =
        Arc::new(ClickHouseInteractionCells::new(Arc::new(ch_client.clone())));
    let interaction_writer: Arc<dyn stitchd_stats_service::interaction_compute::InteractionWriter> =
        Arc::new(ClickHouseInteractionWriter::new(Arc::new(ch_client.clone())));

    // ── Scheduler loop ────────────────────────────────────────────────────────
    let scheduler_pool = pg_pool.clone();
    let scheduler_interval = config.scheduler_interval;
    let scheduler_exp_client = exp_client.clone();
    let scheduler_analytics_client = analytics_client.clone();
    let sweep_experiment_repo = experiment_repo.clone();
    let sweep_metric_repo = metric_repo.clone();
    let sweep_cells = interaction_cells.clone();
    let sweep_writer = interaction_writer.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(scheduler_interval);
        loop {
            ticker.tick().await;
            let experiments = {
                let mut client = scheduler_exp_client.lock().await;
                match fetch_running_experiments(&mut client).await {
                    Err(e) => {
                        error!("Failed to fetch running experiments: {e}");
                        continue;
                    }
                    Ok(exps) => exps,
                }
            };

            for exp in experiments {
                let pool = scheduler_pool.clone();
                let analytics = scheduler_analytics_client.clone();
                tokio::spawn(async move {
                    let computed_at = chrono::Utc::now();
                    // Stats computation is deferred to Phase 3 full implementation.
                    // For scaffold: just update the schedule to record that we ran.
                    {
                        let mut ac = analytics.lock().await;
                        if let Err(e) = write_results(
                            &mut ac,
                            exp.env_id,
                            exp.experiment_id,
                            exp.iteration_id,
                            computed_at,
                            &[],
                        )
                        .await
                        {
                            warn!(experiment_id = %exp.experiment_id, "Failed to write results: {e}");
                            return;
                        }
                    }
                    if let Err(e) = update_schedule_after_run(
                        &pool,
                        exp.experiment_id,
                        computed_at,
                        Duration::from_std(scheduler_interval).unwrap_or(Duration::hours(1)),
                    )
                    .await
                    {
                        warn!(experiment_id = %exp.experiment_id, "Failed to update schedule: {e}");
                    }
                });
            }

            // ── Cross-experiment interaction sweep ─────────────────────────────
            // After the per-experiment pass, recompute pairwise interactions for
            // every environment's running experiments and persist them to the
            // ClickHouse `experiment_interactions` table.
            match sweep_experiment_repo.list_all_running().await {
                Ok(running) => {
                    match run_interaction_sweep(
                        sweep_cells.as_ref(),
                        sweep_writer.as_ref(),
                        sweep_metric_repo.as_ref(),
                        &running,
                        chrono::Utc::now(),
                    )
                    .await
                    {
                        Ok(n) => info!(interactions_written = n, "interaction sweep complete"),
                        Err(e) => error!("interaction sweep failed: {e}"),
                    }
                }
                Err(e) => error!("interaction sweep: failed to list running experiments: {e}"),
            }
        }
    });

    // ── gRPC server ───────────────────────────────────────────────────────────
    let grpc_addr = SocketAddr::from(([0, 0, 0, 0], config.grpc_port));
    info!("StatsService gRPC listening on {grpc_addr}");

    let (health_reporter, health_service) = health_reporter();
    health_reporter
        .set_serving::<StatsServiceServer<StatsServiceImpl>>()
        .await;

    // ── Timeseries reader (Phase 7 Task 3) ────────────────────────────────────
    use stitchd_stats_service::timeseries_reader::ClickHouseTimeseriesReader;
    let timeseries_reader = Arc::new(ClickHouseTimeseriesReader::new(
        Arc::new(ch_client.clone()),
        metric_repo,
        experiment_repo,
    ));

    let stats_svc =
        StatsServiceImpl::new(pg_pool.clone()).with_timeseries_reader(timeseries_reader);

    tokio::spawn(
        Server::builder()
            .add_service(health_service)
            .add_service(StatsServiceServer::new(stats_svc))
            .serve_with_shutdown(grpc_addr, async {
                // Shut down gRPC when the main process receives a signal.
                // We use a simple channel-less approach: just wait for ctrl_c.
                let _ = tokio::signal::ctrl_c().await;
            }),
    );

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
