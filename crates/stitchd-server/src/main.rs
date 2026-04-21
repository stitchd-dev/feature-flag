//! Stitchd server binary entry point.
use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use stitchd_events::writer::EventWriter;
use stitchd_proto::flags::v1::flag_sync_service_server::FlagSyncServiceServer;
use stitchd_server::{AppState, build_router, grpc::flag_sync::FlagSyncServiceImpl, telemetry};
use tracing::info;
use utoipa::OpenApi as _;

const SERVICE_NAME: &str = "stitchd-server";
const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<()> {
    // Short-circuit: export OpenAPI JSON and exit (no DB connection needed).
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--export-openapi") {
        let out_path = args.next().unwrap_or_else(|| "openapi.json".to_string());
        let doc = stitchd_server::api::openapi::StitchdApiDoc::openapi();
        let json = serde_json::to_string_pretty(&doc).context("failed to serialise OpenAPI doc")?;
        if out_path == "-" {
            print!("{json}");
        } else {
            std::fs::write(&out_path, json)
                .with_context(|| format!("failed to write OpenAPI JSON to {out_path}"))?;
            println!("OpenAPI JSON written to {out_path}");
        }
        return Ok(());
    }

    // Metrics must be installed before tracing so the metrics layer can record
    // any spans emitted during subscriber setup.
    let metrics_handle = telemetry::init_metrics().context("failed to initialise metrics")?;

    let tracer_provider = telemetry::init_tracing(SERVICE_NAME, SERVICE_VERSION)
        .context("failed to initialise tracing")?;

    info!(
        service = SERVICE_NAME,
        version = SERVICE_VERSION,
        "stitchd-server starting"
    );

    let database_url =
        std::env::var("DATABASE_URL").context("DATABASE_URL environment variable must be set")?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .context("failed to connect to PostgreSQL")?;

    let audit_logger =
        std::sync::Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(pool.clone()));

    let state = AppState {
        db: pool.clone(),
        metrics_handle,
        user_repo: std::sync::Arc::new(stitchd_db::repository::pg::PgUserRepository::new(
            pool.clone(),
            audit_logger.clone(),
        )),
        auth_user_repo: std::sync::Arc::new(stitchd_db::PgAuthUserRepository::new(pool.clone())),
        membership_repo: std::sync::Arc::new(stitchd_db::PgOrgMembershipRepository::new(pool.clone())),
        refresh_token_repo: std::sync::Arc::new(stitchd_db::PgRefreshTokenRepository::new(pool.clone())),
        mfa_repo: std::sync::Arc::new(stitchd_db::PgMfaRepository::new(pool.clone())),
        auth_provider_repo: std::sync::Arc::new(stitchd_db::PgAuthProviderRepository::new(pool.clone())),
        segment_repo: std::sync::Arc::new(stitchd_db::repository::pg::PgSegmentRepository::new(
            pool.clone(),
            audit_logger.clone(),
        )),
        flag_repo: std::sync::Arc::new(stitchd_db::repository::pg::PgFlagRepository::new(
            pool.clone(),
            audit_logger.clone(),
        )),
        variant_repo: std::sync::Arc::new(stitchd_db::repository::pg::PgVariantRepository::new(
            pool.clone(),
            audit_logger.clone(),
        )),
        sdk_key_repo: std::sync::Arc::new(stitchd_db::repository::pg::PgSdkKeyRepository::new(
            pool.clone(),
            audit_logger.clone(),
        )),
        event_definition_repo: std::sync::Arc::new(
            stitchd_db::repository::pg::PgEventDefinitionRepository::new(
                pool.clone(),
                audit_logger.clone(),
            ),
        ),
        experiment_repo: std::sync::Arc::new(
            stitchd_db::repository::pg::PgExperimentRepository::new(pool.clone(), audit_logger),
        ),
        results_repo: std::sync::Arc::new(
            stitchd_db::experiment_results::PgExperimentResultsRepository::new(pool.clone()),
        ),
        ch_client: std::env::var("CLICKHOUSE_URL").ok().map(|url| {
            info!(url = %url, "ClickHouse client enabled");
            std::sync::Arc::new(clickhouse::Client::default().with_url(url))
        }),
        event_writer: std::env::var("CLICKHOUSE_URL")
            .ok()
            .map(|url| EventWriter::new(clickhouse::Client::default().with_url(url))),
        oidc_state_cache: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };

    let http_port: u16 = std::env::var("HTTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let grpc_port: u16 = std::env::var("GRPC_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9090);

    let http_addr = std::net::SocketAddr::from(([0, 0, 0, 0], http_port));
    let grpc_addr = std::net::SocketAddr::from(([0, 0, 0, 0], grpc_port));

    let listener = tokio::net::TcpListener::bind(http_addr)
        .await
        .with_context(|| format!("failed to bind HTTP to {http_addr}"))?;

    info!(address = %http_addr, "HTTP server listening");
    info!(address = %grpc_addr, "gRPC server listening");

    // Run initial maintenance and spawn background task
    stitchd_server::startup::run_partman_maintenance(&pool).await;
    stitchd_server::startup::spawn_maintenance_task(pool);

    let app = build_router(state.clone());
    let grpc_svc = FlagSyncServiceServer::new(FlagSyncServiceImpl::new(state));

    // Run HTTP and gRPC servers concurrently; stop both when either receives shutdown.
    tokio::select! {
        result = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()) => {
            result.context("HTTP server error")?;
        }
        result = tonic::transport::Server::builder()
            .add_service(grpc_svc)
            .serve_with_shutdown(grpc_addr, shutdown_signal()) => {
            result.context("gRPC server error")?;
        }
    }

    info!("shutting down");
    telemetry::shutdown_tracing(&tracer_provider);

    Ok(())
}

/// Resolves when SIGTERM or Ctrl-C is received.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = sigterm => {},
    }

    info!("shutdown signal received");
}
