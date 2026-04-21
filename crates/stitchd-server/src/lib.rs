//! Stitchd server: Axum REST Admin API + tonic gRPC SDK API.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

/// API routes and handlers.
pub mod api;
/// Experimentation server-side logic (async compute pipeline).
pub mod experimentation;
/// gRPC service implementations.
pub mod grpc;
/// Server startup and maintenance tasks.
pub mod startup;
/// Telemetry and observability setup.
pub mod telemetry;

use axum::{Json, Router, extract::State, routing::get};
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Serialize;
use sqlx::PgPool;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use stitchd_core::id::AuthProviderId;
use stitchd_db::{
    AuthProviderRepository, AuthUserRepository, EventDefinitionRepository, ExperimentRepository,
    FlagRepository, OrgMembershipRepository, RefreshTokenRepository, SdkKeyRepository,
    SegmentRepository, UserRepository, VariantRepository,
    experiment_results::ExperimentResultsRepository,
};
use stitchd_events::writer::EventWriter;
use utoipa::OpenApi as _;

/// Type alias for the OIDC state cache inner map: state token → (`pkce_verifier`, `provider_id`, `issued_at`).
pub type OidcStateMap = HashMap<String, (String, AuthProviderId, Instant)>;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Postgres connection pool.
    pub db: PgPool,
    /// Prometheus metrics handle.
    pub metrics_handle: PrometheusHandle,
    /// Repository for platform user management and JWT auth.
    pub user_repo: Arc<dyn UserRepository>,
    /// Auth-focused user repository (create, find, rotate secret, update status/password/profile).
    pub auth_user_repo: Arc<dyn AuthUserRepository>,
    /// Org membership repository (add/find/list/remove/update members).
    pub membership_repo: Arc<dyn OrgMembershipRepository>,
    /// Refresh token repository (create, find, consume, revoke).
    pub refresh_token_repo: Arc<dyn RefreshTokenRepository>,
    /// Auth provider repository (OIDC/SAML provider management).
    pub auth_provider_repo: Arc<dyn AuthProviderRepository>,
    /// Repository for segment management.
    pub segment_repo: Arc<dyn SegmentRepository>,
    /// Repository for feature flag management.
    pub flag_repo: Arc<dyn FlagRepository>,
    /// Repository for flag variant management.
    pub variant_repo: Arc<dyn VariantRepository>,
    /// Repository for SDK key authentication.
    pub sdk_key_repo: Arc<dyn SdkKeyRepository>,
    /// Repository for event definition registration.
    pub event_definition_repo: Arc<dyn EventDefinitionRepository>,
    /// Repository for experiment management.
    pub experiment_repo: Arc<dyn ExperimentRepository>,
    /// Repository for experiment results read/write.
    pub results_repo: Arc<dyn ExperimentResultsRepository>,
    /// ClickHouse client used for fire-and-forget compute jobs. `None` when
    /// ClickHouse is unavailable (recompute requests are accepted but not executed).
    pub ch_client: Option<Arc<clickhouse::Client>>,
    /// ClickHouse event writer. `None` when ClickHouse is unavailable (writes are skipped).
    pub event_writer: Option<EventWriter>,
    /// Short-lived OIDC state cache: state → (`pkce_verifier`, `provider_id`, `issued_at`).
    /// TTL is 10 minutes; entries are checked on use.
    pub oidc_state_cache: Arc<Mutex<OidcStateMap>>,
}

/// Build the Axum router.
///
/// Currently only exposes infrastructure endpoints (`/health`, `/metrics`).
/// Feature routes will be added in subsequent tracks.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .route("/api-docs/openapi.json", get(openapi_json_handler))
        .merge(api::router::build_api_router())
        .with_state(state)
}

/// `GET /api-docs/openapi.json` — Serve the `OpenAPI` 3.x JSON document.
async fn openapi_json_handler() -> Json<utoipa::openapi::OpenApi> {
    Json(api::openapi::StitchdApiDoc::openapi())
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    db: &'static str,
}

/// `GET /health` — liveness and readiness probe.
async fn health_handler(State(state): State<AppState>) -> Json<HealthResponse> {
    let db_status = if sqlx::query("SELECT 1").execute(&state.db).await.is_ok() {
        "ok"
    } else {
        "error"
    };

    let status = if db_status == "ok" { "ok" } else { "degraded" };

    Json(HealthResponse {
        status,
        db: db_status,
    })
}

/// `GET /metrics` — Prometheus scrape endpoint.
async fn metrics_handler(State(state): State<AppState>) -> String {
    state.metrics_handle.render()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt as _;

    async fn setup_test_state() -> AppState {
        let db = PgPool::connect("postgres://stitchd:stitchd@localhost:5432/stitchd")
            .await
            .unwrap();
        let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle();
        let audit = std::sync::Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(db.clone()));
        let audit2 =
            std::sync::Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(db.clone()));
        let audit3 =
            std::sync::Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(db.clone()));
        let audit4 =
            std::sync::Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(db.clone()));
        let audit5 =
            std::sync::Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(db.clone()));
        let segment_repo = std::sync::Arc::new(
            stitchd_db::repository::pg::PgSegmentRepository::new(db.clone(), audit),
        );
        let flag_repo = std::sync::Arc::new(stitchd_db::repository::pg::PgFlagRepository::new(
            db.clone(),
            audit2,
        ));
        let variant_repo = std::sync::Arc::new(
            stitchd_db::repository::pg::PgVariantRepository::new(db.clone(), audit3),
        );
        let sdk_key_repo = std::sync::Arc::new(
            stitchd_db::repository::pg::PgSdkKeyRepository::new(db.clone(), audit4),
        );
        let event_definition_repo = std::sync::Arc::new(
            stitchd_db::repository::pg::PgEventDefinitionRepository::new(db.clone(), audit5),
        );
        let audit6 =
            std::sync::Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(db.clone()));
        let experiment_repo = std::sync::Arc::new(
            stitchd_db::repository::pg::PgExperimentRepository::new(db.clone(), audit6),
        );
        let results_repo = std::sync::Arc::new(
            stitchd_db::experiment_results::PgExperimentResultsRepository::new(db.clone()),
        );
        let audit7 =
            std::sync::Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(db.clone()));
        let user_repo = std::sync::Arc::new(
            stitchd_db::repository::pg::PgUserRepository::new(db.clone(), audit7),
        );
        let auth_user_repo = std::sync::Arc::new(
            stitchd_db::PgAuthUserRepository::new(db.clone()),
        );
        let membership_repo = std::sync::Arc::new(
            stitchd_db::PgOrgMembershipRepository::new(db.clone()),
        );
        let refresh_token_repo = std::sync::Arc::new(
            stitchd_db::PgRefreshTokenRepository::new(db.clone()),
        );
        let auth_provider_repo = std::sync::Arc::new(
            stitchd_db::PgAuthProviderRepository::new(db.clone()),
        );
        AppState {
            db,
            metrics_handle,
            user_repo,
            auth_user_repo,
            membership_repo,
            refresh_token_repo,
            auth_provider_repo,
            segment_repo,
            flag_repo,
            variant_repo,
            sdk_key_repo,
            event_definition_repo,
            experiment_repo,
            results_repo,
            ch_client: None,
            event_writer: None,
            oidc_state_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn make_stub_state() -> AppState {
        use metrics_exporter_prometheus::PrometheusBuilder;
        use std::collections::HashMap;

        struct StubRepo;
        #[async_trait::async_trait]
        impl stitchd_db::FlagRepository for StubRepo {
            async fn find_by_id(
                &self,
                id: stitchd_core::id::FlagId,
            ) -> Result<stitchd_core::flag::FlagRecord, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(
                &self,
                key: &stitchd_core::id::FlagKey,
                _: stitchd_core::id::ProjectId,
            ) -> Result<stitchd_core::flag::FlagRecord, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound {
                    id: key.to_string(),
                })
            }
            async fn list_by_project(
                &self,
                _: stitchd_core::id::ProjectId,
            ) -> Result<Vec<stitchd_core::flag::FlagRecord>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn list_by_environment(
                &self,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<Vec<stitchd_core::flag::FlagRecord>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: &stitchd_core::flag::FlagRecord,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                f: &stitchd_core::flag::FlagRecord,
            ) -> Result<stitchd_core::flag::FlagRecord, stitchd_db::RepositoryError> {
                Ok(f.clone())
            }
            async fn soft_delete(
                &self,
                id: stitchd_core::id::FlagId,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_hashing_config(
                &self,
                _: stitchd_core::id::FlagId,
            ) -> Result<Vec<stitchd_core::flag::FlagHashingConfig>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn upsert_hashing_config(
                &self,
                _: stitchd_core::id::FlagId,
                _: &[stitchd_core::flag::FlagHashingConfig],
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn find_rules(
                &self,
                _: stitchd_core::id::FlagId,
            ) -> Result<Vec<stitchd_core::flag::FlagRule>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn upsert_rules(
                &self,
                _: stitchd_core::id::FlagId,
                _: &[stitchd_core::flag::FlagRule],
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
        }
        #[async_trait::async_trait]
        impl stitchd_db::VariantRepository for StubRepo {
            async fn find_by_flag(
                &self,
                _: stitchd_core::id::FlagId,
            ) -> Result<Vec<stitchd_core::flag::Variant>, stitchd_db::RepositoryError> {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: stitchd_core::id::FlagId,
                _: &stitchd_core::flag::Variant,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                v: &stitchd_core::flag::Variant,
            ) -> Result<stitchd_core::flag::Variant, stitchd_db::RepositoryError> {
                Ok(v.clone())
            }
            async fn delete(
                &self,
                _: stitchd_core::id::VariantId,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
        }
        #[async_trait::async_trait]
        impl stitchd_db::SegmentRepository for StubRepo {
            async fn find_by_id(
                &self,
                id: stitchd_core::id::SegmentId,
            ) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(
                &self,
                key: &str,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound {
                    id: key.to_string(),
                })
            }
            async fn list_by_environment(
                &self,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<Vec<stitchd_core::segment::Segment>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: &stitchd_core::segment::Segment,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                s: &stitchd_core::segment::Segment,
            ) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> {
                Ok(s.clone())
            }
            async fn find_with_rules(
                &self,
                id: stitchd_core::id::SegmentId,
            ) -> Result<stitchd_core::segment::RuleBasedSegment, stitchd_db::RepositoryError>
            {
                Ok(stitchd_core::segment::RuleBasedSegment { id, rules: vec![] })
            }
            async fn find_with_list(
                &self,
                id: stitchd_core::id::SegmentId,
            ) -> Result<stitchd_core::segment::ListBasedSegment, stitchd_db::RepositoryError>
            {
                Ok(stitchd_core::segment::ListBasedSegment {
                    id,
                    lists: HashMap::new(),
                })
            }
            async fn upsert_rules(
                &self,
                _: stitchd_core::id::SegmentId,
                _: &[stitchd_core::rule_engine::types::Rule],
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn set_list_entries(
                &self,
                _: stitchd_core::id::SegmentId,
                _: &str,
                _: &[String],
                _: &[String],
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn soft_delete(
                &self,
                _: stitchd_core::id::SegmentId,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn check_list_membership(
                &self,
                _: stitchd_core::id::EnvironmentId,
                _: &str,
                _: &str,
                keys: &[String],
            ) -> Result<HashMap<String, bool>, stitchd_db::RepositoryError> {
                Ok(keys.iter().map(|k| (k.clone(), false)).collect())
            }
            async fn batch_check_list_membership(
                &self,
                _: stitchd_core::id::EnvironmentId,
                _: &[(String, String)],
                _: &[String],
            ) -> Result<Vec<stitchd_db::ContextMembership>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
        }
        #[async_trait::async_trait]
        impl stitchd_db::SdkKeyRepository for StubRepo {
            async fn find_by_id(
                &self,
                id: stitchd_core::id::SdkKeyId,
            ) -> Result<stitchd_core::tenant::SdkKey, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_by_environment(
                &self,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<Vec<stitchd_core::tenant::SdkKey>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: &stitchd_core::tenant::SdkKey,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn revoke(
                &self,
                id: stitchd_core::id::SdkKeyId,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_active_by_environment(
                &self,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<Vec<stitchd_core::tenant::SdkKey>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn find_active_by_hash(
                &self,
                h: &str,
            ) -> Result<stitchd_core::tenant::SdkKey, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: h.to_string() })
            }
        }

        #[async_trait::async_trait]
        impl stitchd_db::EventDefinitionRepository for StubRepo {
            async fn find_by_id(
                &self,
                id: stitchd_core::id::EventDefinitionId,
            ) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(
                &self,
                key: &str,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound {
                    id: key.to_string(),
                })
            }
            async fn list_by_environment(
                &self,
                _: stitchd_core::id::EnvironmentId,
            ) -> Result<Vec<stitchd_core::event::EventDefinition>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: &stitchd_core::event::EventDefinition,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                d: &stitchd_core::event::EventDefinition,
            ) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError>
            {
                Ok(d.clone())
            }
            async fn soft_delete(
                &self,
                id: stitchd_core::id::EventDefinitionId,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
        }

        #[async_trait::async_trait]
        impl stitchd_db::ExperimentRepository for StubRepo {
            async fn find_by_id(
                &self,
                id: stitchd_core::id::ExperimentId,
            ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_by_environment(
                &self,
                _: stitchd_core::id::EnvironmentId,
                _: Option<stitchd_core::experimentation::ExperimentStatus>,
            ) -> Result<Vec<stitchd_core::experimentation::Experiment>, stitchd_db::RepositoryError>
            {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: &stitchd_core::experimentation::Experiment,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                e: &stitchd_core::experimentation::Experiment,
            ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
            {
                Ok(e.clone())
            }
            async fn soft_delete(
                &self,
                id: stitchd_core::id::ExperimentId,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_iterations(
                &self,
                _: stitchd_core::id::ExperimentId,
            ) -> Result<
                Vec<stitchd_core::experimentation::ExperimentIteration>,
                stitchd_db::RepositoryError,
            > {
                Ok(vec![])
            }
            async fn apply_transition(
                &self,
                id: stitchd_core::id::ExperimentId,
                _: stitchd_core::experimentation::ExperimentStatus,
                _: Option<stitchd_core::id::UserId>,
            ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
            {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
        }

        #[async_trait::async_trait]
        impl stitchd_db::UserRepository for StubRepo {
            async fn find_by_id(
                &self,
                id: stitchd_core::id::UserId,
            ) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_email(
                &self,
                email: &str,
            ) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound {
                    id: email.to_string(),
                })
            }
            async fn list_by_organisation(
                &self,
                _: stitchd_core::id::OrganisationId,
            ) -> Result<Vec<stitchd_core::auth::User>, stitchd_db::RepositoryError> {
                Ok(vec![])
            }
            async fn create(
                &self,
                _: &stitchd_core::auth::User,
            ) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                u: &stitchd_core::auth::User,
            ) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> {
                Ok(u.clone())
            }
            async fn find_permissions_for_user(
                &self,
                _: stitchd_core::id::UserId,
                _: stitchd_core::id::ProjectId,
            ) -> Result<Vec<stitchd_core::user::Permission>, stitchd_db::RepositoryError> {
                Ok(vec![])
            }
        }

        struct NullResultsRepo;
        #[async_trait::async_trait]
        impl stitchd_db::experiment_results::ExperimentResultsRepository for NullResultsRepo {
            async fn upsert(
                &self,
                _: &stitchd_db::experiment_results::UpsertResultRow,
            ) -> Result<stitchd_db::experiment_results::ExperimentResultRow, sqlx::Error>
            {
                Err(sqlx::Error::RowNotFound)
            }
            async fn fetch_latest(
                &self,
                _: uuid::Uuid,
            ) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error>
            {
                Ok(vec![])
            }
            async fn fetch_by_iteration(
                &self,
                _: uuid::Uuid,
                _: uuid::Uuid,
            ) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error>
            {
                Ok(vec![])
            }
            async fn is_stale(&self, _: uuid::Uuid, _: uuid::Uuid) -> Result<bool, sqlx::Error> {
                Ok(false)
            }
        }

        struct StubAuthUserRepo;
        #[async_trait::async_trait]
        impl stitchd_db::AuthUserRepository for StubAuthUserRepo {
            async fn create(&self, email: &str, _: &str, _: Option<&str>) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: email.to_string() })
            }
            async fn find_by_email(&self, _: &str) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> {
                Ok(None)
            }
            async fn find_by_id(&self, _: stitchd_core::id::UserId) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> {
                Ok(None)
            }
            async fn rotate_token_secret(&self, id: stitchd_core::id::UserId) -> Result<uuid::Uuid, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn update_status(&self, id: stitchd_core::id::UserId, _: stitchd_core::auth::UserStatus) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn update_password_hash(&self, id: stitchd_core::id::UserId, _: &str) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn update_profile(&self, id: stitchd_core::id::UserId, _: &str, _: Option<&str>) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
        }

        struct StubMembershipRepo;
        #[async_trait::async_trait]
        impl stitchd_db::OrgMembershipRepository for StubMembershipRepo {
            async fn add_member(&self, user_id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<stitchd_core::auth::OrgMembership, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: user_id.to_string() })
            }
            async fn find_membership(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId) -> Result<Option<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> {
                Ok(None)
            }
            async fn list_orgs_for_user(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> {
                Ok(vec![])
            }
            async fn remove_member(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn update_role(&self, user_id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<(), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: user_id.to_string() })
            }
        }

        struct StubRefreshTokenRepo;
        #[async_trait::async_trait]
        impl stitchd_db::RefreshTokenRepository for StubRefreshTokenRepo {
            async fn create(&self, user_id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: Option<&str>, _: i64) -> Result<(stitchd_core::auth::RefreshToken, String), stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: user_id.to_string() })
            }
            async fn find_by_hash(&self, _: &str) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> {
                Ok(None)
            }
            async fn consume(&self, id: stitchd_core::id::RefreshTokenId) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn revoke(&self, _: stitchd_core::id::RefreshTokenId) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn revoke_all_for_user(&self, _: stitchd_core::id::UserId) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
            async fn list_active(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> {
                Ok(vec![])
            }
        }

        struct StubAuthProviderRepo;
        #[async_trait::async_trait]
        impl stitchd_db::AuthProviderRepository for StubAuthProviderRepo {
            async fn create(&self, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::ProviderType, _: &str, _: serde_json::Value, _: bool) -> Result<stitchd_core::auth::AuthProvider, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::Unexpected(anyhow::anyhow!("stub")))
            }
            async fn find_by_id(&self, id: stitchd_core::id::AuthProviderId) -> Result<Option<stitchd_core::auth::AuthProvider>, stitchd_db::RepositoryError> {
                let _ = id;
                Ok(None)
            }
            async fn list_for_org(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<stitchd_core::auth::AuthProvider>, stitchd_db::RepositoryError> {
                Ok(vec![])
            }
            async fn update(&self, id: stitchd_core::id::AuthProviderId, _: &str, _: serde_json::Value, _: bool) -> Result<stitchd_core::auth::AuthProvider, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn delete(&self, _: stitchd_core::id::AuthProviderId) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
        }

        let stub = Arc::new(StubRepo);
        AppState {
            db: PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_stub")
                .expect("lazy pool"),
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            user_repo: stub.clone(),
            auth_user_repo: Arc::new(StubAuthUserRepo),
            membership_repo: Arc::new(StubMembershipRepo),
            refresh_token_repo: Arc::new(StubRefreshTokenRepo),
            auth_provider_repo: Arc::new(StubAuthProviderRepo),
            segment_repo: stub.clone(),
            flag_repo: stub.clone(),
            variant_repo: stub.clone(),
            sdk_key_repo: stub.clone(),
            event_definition_repo: stub.clone(),
            experiment_repo: stub,
            results_repo: Arc::new(NullResultsRepo),
            ch_client: None,
            event_writer: None,
            oidc_state_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[tokio::test]
    async fn openapi_json_endpoint_returns_valid_document() {
        let app = build_router(make_stub_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api-docs/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(doc["openapi"], "3.1.0");
        assert_eq!(doc["info"]["title"], "Stitchd Feature Flag API");
        assert!(doc["paths"].is_object());
    }

    #[tokio::test]
    #[ignore = "requires local DB"]
    async fn health_endpoint_returns_ok() {
        let state = setup_test_state().await;
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    #[ignore = "requires local DB"]
    async fn metrics_endpoint_returns_ok() {
        let state = setup_test_state().await;
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
