//! Entry point for the `stitchd-auth-service` gRPC microservice.
//!
//! Environment variables:
//! - `AUTH_SERVICE_PORT`         (default: `50051`) — gRPC bind port
//! - `DATABASE_URL`              — PostgreSQL connection string (required)
//! - `METRICS_PORT`              (default: `9091`) — Prometheus metrics port
//! - `SUPERADMIN_EMAIL`          — seed a superadmin user on first boot
//! - `SUPERADMIN_PASSWORD`       — plaintext password hashed with Argon2id
//! - `PROVIDER_CACHE_TTL_SECS`   (default: `3600`) — OIDC/SAML provider cache TTL

use std::{net::SocketAddr, sync::Arc};

use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::signal;
use tonic::transport::Server;
use tonic_health::server::health_reporter;
use tracing::info;

use stitchd_auth_service::{
    app_state::ProviderCaches,
    auth_provider::AuthProviderServiceImpl,
    bootstrap::seed_superadmin,
    grpc::AuthServiceImpl,
    management::ManagementServiceImpl,
};
use stitchd_db::{
    AuthUserRepository, OrgMembershipRepository, OrganisationRepository, PgAuditLogger,
    PgAuthUserRepository, PgEnvironmentRepository, PgOrgMembershipRepository,
    PgOrganisationRepository, PgProjectRepository, PgRefreshTokenRepository, PgSdkKeyRepository,
    RefreshTokenRepository,
};
use stitchd_core::auth::CryptoKey;
use stitchd_db::PgAuthProviderRepository;
use stitchd_proto::{
    auth::v1::{
        auth_provider_service_server::AuthProviderServiceServer,
        auth_service_server::AuthServiceServer,
    },
    management::v1::management_service_server::ManagementServiceServer,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

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

    let database_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL environment variable must be set");
    let pool = sqlx::PgPool::connect(&database_url).await?;

    // Run migrations.
    sqlx::migrate!("../stitchd-db/migrations")
        .run(&pool)
        .await?;

    let audit = Arc::new(PgAuditLogger::new(pool.clone()));

    // Repositories
    let auth_user_repo = Arc::new(PgAuthUserRepository::new(pool.clone()));
    let sdk_key_repo = Arc::new(PgSdkKeyRepository::new(pool.clone(), audit.clone()));
    let membership_repo = Arc::new(PgOrgMembershipRepository::new(pool.clone()));
    let refresh_repo: Arc<dyn RefreshTokenRepository> =
        Arc::new(PgRefreshTokenRepository::new(pool.clone()));
    let org_repo = Arc::new(PgOrganisationRepository::new(pool.clone(), audit.clone()));
    let project_repo = Arc::new(PgProjectRepository::new(pool.clone(), audit.clone()));
    let env_repo = Arc::new(PgEnvironmentRepository::new(pool.clone(), audit.clone()));

    // Provider caches — zero providers loaded at startup; built lazily on first login.
    let provider_caches = Arc::new(ProviderCaches::from_env());
    let auth_provider_repo = Arc::new(PgAuthProviderRepository::new(pool.clone()));
    let crypto_key = Arc::new(CryptoKey::from_env().expect("AUTH_ENCRYPTION_KEY must be set"));

    // Bootstrap superadmin if configured.
    seed_superadmin(
        &(auth_user_repo.clone() as Arc<dyn AuthUserRepository>),
        &(org_repo.clone() as Arc<dyn OrganisationRepository>),
        &(membership_repo.clone() as Arc<dyn OrgMembershipRepository>),
    )
    .await?;

    let grpc_port: u16 = std::env::var("AUTH_SERVICE_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50051_u16);
    let grpc_addr: SocketAddr = format!("0.0.0.0:{grpc_port}").parse()?;

    let auth_service = AuthServiceImpl::new(
        auth_user_repo.clone(),
        sdk_key_repo.clone(),
        membership_repo.clone(),
        org_repo.clone() as Arc<dyn OrganisationRepository>,
        refresh_repo,
    );
    let mgmt_service = ManagementServiceImpl::new(
        org_repo,
        project_repo,
        env_repo,
        sdk_key_repo,
        auth_user_repo,
        membership_repo,
    );
    let auth_provider_service = AuthProviderServiceImpl::new(
        auth_provider_repo,
        crypto_key,
        provider_caches,
    );

    let (health_reporter, health_service) = health_reporter();
    health_reporter
        .set_serving::<AuthServiceServer<AuthServiceImpl>>()
        .await;
    health_reporter
        .set_serving::<ManagementServiceServer<ManagementServiceImpl>>()
        .await;

    info!(%grpc_addr, "stitchd-auth-service starting");

    Server::builder()
        .add_service(health_service)
        .add_service(AuthServiceServer::new(auth_service))
        .add_service(ManagementServiceServer::new(mgmt_service))
        .add_service(AuthProviderServiceServer::new(auth_provider_service))
        .serve_with_shutdown(grpc_addr, async {
            signal::ctrl_c()
                .await
                .expect("failed to install CTRL+C signal handler");
            info!("shutdown signal received");
        })
        .await?;

    Ok(())
}
