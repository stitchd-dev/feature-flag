//! Entry point for the `stitchd-auth-service` gRPC microservice.
//!
//! Reads configuration from environment variables:
//! - `AUTH_SERVICE_PORT` (default: `50051`) — port to bind the gRPC server on.
//! - `DATABASE_URL` — PostgreSQL connection string (required).
//! - `METRICS_PORT` (default: `9091`) — port for the Prometheus metrics endpoint.

use std::{net::SocketAddr, sync::Arc};

use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::signal;
use tonic::transport::Server;
use tracing::info;

use stitchd_auth_service::grpc::AuthServiceImpl;
use stitchd_db::{PgAuthUserRepository, PgSdkKeyRepository, PgAuditLogger};
use stitchd_proto::auth::v1::auth_service_server::AuthServiceServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Observability ────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // ── Prometheus metrics ───────────────────────────────────────────────────
    let metrics_port: u16 = std::env::var("METRICS_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9091_u16);
    let metrics_addr: SocketAddr = format!("0.0.0.0:{metrics_port}").parse()?;

    let builder = PrometheusBuilder::new();
    let handle = builder
        .with_http_listener(metrics_addr)
        .install_recorder()?;
    info!(%metrics_addr, "Prometheus metrics endpoint ready");
    drop(handle);

    // ── Database ─────────────────────────────────────────────────────────────
    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL environment variable must be set");
    let pool = sqlx::PgPool::connect(&database_url).await?;

    // ── Audit logger (no-op for read-only auth path) ──────────────────────────
    let audit = std::sync::Arc::new(PgAuditLogger::new(pool.clone()));

    // ── Repositories ─────────────────────────────────────────────────────────
    let auth_user_repo = Arc::new(PgAuthUserRepository::new(pool.clone()));
    let sdk_key_repo = Arc::new(PgSdkKeyRepository::new(pool.clone(), audit));

    // ── gRPC server ───────────────────────────────────────────────────────────
    let grpc_port: u16 = std::env::var("AUTH_SERVICE_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50051_u16);
    let grpc_addr: SocketAddr = format!("0.0.0.0:{grpc_port}").parse()?;

    let auth_service = AuthServiceImpl::new(auth_user_repo, sdk_key_repo);

    info!(%grpc_addr, "stitchd-auth-service starting");

    Server::builder()
        .add_service(AuthServiceServer::new(auth_service))
        .serve_with_shutdown(grpc_addr, async {
            signal::ctrl_c()
                .await
                .expect("failed to install CTRL+C signal handler");
            info!("shutdown signal received, stopping auth service");
        })
        .await?;

    Ok(())
}
