//! Entry point for the `stitchd-auth-service` gRPC microservice.
//!
//! Environment variables:
//! - `STITCHD_AUTH_SERVICE_GRPC_PORT`         (default: `50051`) — gRPC bind port
//! - `STITCHD_DATABASE_URL`              — PostgreSQL connection string (required)
//! - `STITCHD_AUTH_SERVICE_METRICS_PORT`              (default: `9091`) — Prometheus metrics port
//! - `STITCHD_SUPERADMIN_EMAIL`          — seed a superadmin user on first boot
//! - `STITCHD_SUPERADMIN_PASSWORD`       — plaintext password hashed with Argon2id
//! - `STITCHD_PROVIDER_CACHE_TTL_SECS`   (default: `3600`) — OIDC/SAML provider cache TTL

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
    oidc_factory::OidcProviderFactory,
    oidc_login::{LiveOidcExchanger, OidcLoginServiceImpl, OidcStateStore},
    saml_factory::SamlProviderFactory,
    saml_login::{LiveSamlExchanger, SamlLoginServiceImpl, SamlRelayStore},
    sdk_key_cache::SdkKeyCache,
};
use stitchd_core::auth::CryptoKey;
use stitchd_db::PgAuthProviderRepository;
use stitchd_db::{
    AuthUserRepository, OrgMembershipRepository, OrganisationRepository, PgAuditLogger,
    PgAuthUserRepository, PgContextRegistryRepository, PgEnvironmentRepository,
    PgOrgMembershipRepository, PgOrganisationRepository, PgProjectRepository,
    PgRefreshTokenRepository, PgSdkKeyRepository, RefreshTokenRepository,
};
use stitchd_proto::{
    auth::v1::{
        auth_provider_service_server::AuthProviderServiceServer,
        auth_service_server::AuthServiceServer, oidc_login_service_server::OidcLoginServiceServer,
        saml_login_service_server::SamlLoginServiceServer,
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

    let metrics_port: u16 = std::env::var("STITCHD_AUTH_SERVICE_METRICS_PORT")
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

    let database_url = std::env::var("STITCHD_DATABASE_URL")
        .expect("STITCHD_DATABASE_URL environment variable must be set");
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
    let crypto_key =
        Arc::new(CryptoKey::from_env().expect("STITCHD_AUTH_ENCRYPTION_KEY must be set"));

    // Bootstrap superadmin if configured.
    seed_superadmin(
        &(auth_user_repo.clone() as Arc<dyn AuthUserRepository>),
        &(org_repo.clone() as Arc<dyn OrganisationRepository>),
        &(membership_repo.clone() as Arc<dyn OrgMembershipRepository>),
    )
    .await?;

    let grpc_port: u16 = std::env::var("STITCHD_AUTH_SERVICE_GRPC_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50051_u16);
    let grpc_addr: SocketAddr = format!("0.0.0.0:{grpc_port}").parse()?;

    let auth_service = AuthServiceImpl::new(
        auth_user_repo.clone(),
        sdk_key_repo.clone(),
        membership_repo.clone(),
        org_repo.clone() as Arc<dyn OrganisationRepository>,
        refresh_repo.clone(),
    );
    let sdk_key_cache = SdkKeyCache::new();
    let context_registry = Arc::new(PgContextRegistryRepository::new(pool.clone()));
    let mgmt_service = ManagementServiceImpl::new(
        org_repo,
        project_repo,
        env_repo,
        sdk_key_repo,
        auth_user_repo.clone(),
        membership_repo.clone(),
        sdk_key_cache,
        context_registry,
    );
    let auth_provider_service = AuthProviderServiceImpl::new(
        auth_provider_repo.clone(),
        Arc::clone(&crypto_key),
        provider_caches.clone(),
    );

    let oidc_factory = Arc::new(OidcProviderFactory::new(
        auth_provider_repo.clone() as Arc<dyn stitchd_db::AuthProviderRepository>,
        crypto_key,
    ));
    let oidc_exchanger = Arc::new(LiveOidcExchanger::new(
        Arc::clone(&provider_caches),
        oidc_factory,
    ));
    let oidc_state_store = Arc::new(OidcStateStore::new(std::time::Duration::from_secs(300)));
    let oidc_login_service = OidcLoginServiceImpl::new(
        oidc_exchanger,
        oidc_state_store,
        Arc::clone(&auth_user_repo) as Arc<dyn stitchd_db::AuthUserRepository>,
        Arc::clone(&membership_repo) as Arc<dyn stitchd_db::OrgMembershipRepository>,
        Arc::clone(&refresh_repo),
        Arc::clone(&auth_provider_repo) as Arc<dyn stitchd_db::AuthProviderRepository>,
    );

    let saml_factory = Arc::new(SamlProviderFactory::new(
        Arc::clone(&auth_provider_repo) as Arc<dyn stitchd_db::AuthProviderRepository>
    ));
    let saml_exchanger = Arc::new(LiveSamlExchanger::new(provider_caches, saml_factory));
    let saml_relay_store = Arc::new(SamlRelayStore::new(std::time::Duration::from_secs(600)));
    let saml_login_service = SamlLoginServiceImpl::new(
        saml_exchanger,
        saml_relay_store,
        auth_user_repo as Arc<dyn stitchd_db::AuthUserRepository>,
        membership_repo as Arc<dyn stitchd_db::OrgMembershipRepository>,
        refresh_repo,
        auth_provider_repo as Arc<dyn stitchd_db::AuthProviderRepository>,
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
        .add_service(OidcLoginServiceServer::new(oidc_login_service))
        .add_service(SamlLoginServiceServer::new(saml_login_service))
        .serve_with_shutdown(grpc_addr, async {
            signal::ctrl_c()
                .await
                .expect("failed to install CTRL+C signal handler");
            info!("shutdown signal received");
        })
        .await?;

    Ok(())
}
