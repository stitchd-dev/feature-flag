//! `stitchd-schedule-service` — Scheduled-change lifecycle service.
//!
//! Exposes gRPC `ScheduleService` on `STITCHD_SCHEDULE_SERVICE_GRPC_PORT`
//! (default: 50057) and health/metrics over HTTP on
//! `STITCHD_SCHEDULE_SERVICE_HTTP_PORT` (default: 9201).
//!
//! A background tokio-interval scheduler loop claims due scheduled changes from
//! PostgreSQL (`FOR UPDATE SKIP LOCKED`, restart-safe + idempotent) and applies
//! each via the owning service's canonical mutation RPC.
//!
//! ## Configuration
//! - `STITCHD_DATABASE_URL` — PostgreSQL connection URL (required).
//! - `STITCHD_SCHEDULE_SCHEDULER_INTERVAL_SECS` — tick cadence (default 60).
//! - `STITCHD_SCHEDULE_CLAIM_BATCH` — max claims per tick (default 100).
//! - `STITCHD_FLAG_SERVICE_GRPC_URL` — flag-service endpoint
//!   (default `http://localhost:50051`).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context as _;
use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::signal;
use tokio::sync::Mutex;
use tonic::transport::{Channel, Server};
use tonic_health::server::health_reporter;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use stitchd_core::util::grpc_connect::connect_with_retry_default;
use stitchd_db::ScheduledChangeRepository;
use stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient;
use stitchd_proto::flags::v1::flag_service_client::FlagServiceClient;
use stitchd_proto::schedule::v1::schedule_service_server::ScheduleServiceServer;
use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;
use stitchd_schedule_service::{
    apply::{
        Dispatcher,
        experiment::{ExperimentApplier, GrpcExperimentTransitioner},
        flag::{FlagApplier, GrpcFlagMutator},
        segment::{GrpcSegmentUpdater, SegmentApplier},
    },
    config::ScheduleConfig,
    grpc::ScheduleServiceImpl,
    scheduler::{SystemClock, process_due_changes},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    let config = ScheduleConfig::from_env().context("Failed to load configuration")?;

    let prometheus_handle = PrometheusBuilder::new()
        .install_recorder()
        .context("Failed to install Prometheus metrics recorder")?;

    let pg_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .context("Failed to connect to PostgreSQL")?;

    let repo = ScheduledChangeRepository::new(pg_pool.clone());

    // ── flag-service gRPC client ──────────────────────────────────────────────
    info!(url = %config.flag_service_grpc_url, "Connecting to flag-service gRPC");
    let flag_endpoint = Channel::from_shared(config.flag_service_grpc_url.clone())
        .context("Invalid STITCHD_FLAG_SERVICE_GRPC_URL")?;
    let flag_channel =
        connect_with_retry_default("flag-service", &config.flag_service_grpc_url, || {
            let endpoint = flag_endpoint.clone();
            async move { endpoint.connect().await }
        })
        .await
        .context("Failed to connect to flag-service gRPC")?;
    let flag_client = Arc::new(Mutex::new(FlagServiceClient::new(flag_channel)));

    // ── experimentation-service gRPC client ───────────────────────────────────
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
    let experiment_client = Arc::new(Mutex::new(ExperimentationServiceClient::new(exp_channel)));

    // ── segmentation-service gRPC client ──────────────────────────────────────
    info!(
        url = %config.segmentation_service_grpc_url,
        "Connecting to segmentation-service gRPC"
    );
    let seg_endpoint = Channel::from_shared(config.segmentation_service_grpc_url.clone())
        .context("Invalid STITCHD_SEGMENTATION_SERVICE_GRPC_URL")?;
    let seg_channel = connect_with_retry_default(
        "segmentation-service",
        &config.segmentation_service_grpc_url,
        || {
            let endpoint = seg_endpoint.clone();
            async move { endpoint.connect().await }
        },
    )
    .await
    .context("Failed to connect to segmentation-service gRPC")?;
    let segment_client = Arc::new(Mutex::new(SegmentationServiceClient::new(seg_channel)));

    // ── Scheduler loop ────────────────────────────────────────────────────────
    let scheduler_repo = repo.clone();
    let interval = config.scheduler_interval;
    let batch = config.claim_batch;
    let scheduler_flag_client = flag_client.clone();
    let scheduler_experiment_client = experiment_client.clone();
    let scheduler_segment_client = segment_client.clone();
    tokio::spawn(async move {
        // Entity-type `Dispatcher`: flag changes → flag-service `MutateFlag`,
        // experiment changes → experimentation-service `TransitionExperiment`,
        // segment changes → segmentation-service `UpdateAdminSegment`.
        let flag_applier = FlagApplier::new(GrpcFlagMutator::new(scheduler_flag_client));
        let experiment_applier =
            ExperimentApplier::new(GrpcExperimentTransitioner::new(scheduler_experiment_client));
        let segment_applier =
            SegmentApplier::new(GrpcSegmentUpdater::new(scheduler_segment_client));
        let dispatcher = Dispatcher::new(flag_applier, experiment_applier, segment_applier);
        let clock = SystemClock;
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            if let Err(e) = process_due_changes(&scheduler_repo, &dispatcher, &clock, batch).await {
                error!("scheduler tick failed: {e}");
            }
        }
    });

    // ── gRPC server ───────────────────────────────────────────────────────────
    let grpc_addr = SocketAddr::from(([0, 0, 0, 0], config.grpc_port));
    info!("ScheduleService gRPC listening on {grpc_addr}");

    let (health_reporter, health_service) = health_reporter();
    health_reporter
        .set_serving::<ScheduleServiceServer<ScheduleServiceImpl>>()
        .await;

    let schedule_svc = ScheduleServiceImpl::new(repo);

    tokio::spawn(
        Server::builder()
            .add_service(health_service)
            .add_service(ScheduleServiceServer::new(schedule_svc))
            .serve_with_shutdown(grpc_addr, async {
                let _ = tokio::signal::ctrl_c().await;
            }),
    );

    // ── Axum HTTP server (health + metrics) ──────────────────────────────────
    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(prometheus_handle);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.http_port));
    info!("Schedule service HTTP listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("Failed to bind HTTP listener")?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")?;

    info!("Schedule service shut down gracefully");
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
