//! Stitchd server: Axum REST Admin API + tonic gRPC SDK API.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

/// API routes and handlers.
pub mod api;
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
use std::sync::Arc;
use stitchd_db::{FlagRepository, SdkKeyRepository, SegmentRepository, VariantRepository};
use utoipa::OpenApi as _;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Postgres connection pool.
    pub db: PgPool,
    /// Prometheus metrics handle.
    pub metrics_handle: PrometheusHandle,
    /// Repository for segment management.
    pub segment_repo: Arc<dyn SegmentRepository>,
    /// Repository for feature flag management.
    pub flag_repo: Arc<dyn FlagRepository>,
    /// Repository for flag variant management.
    pub variant_repo: Arc<dyn VariantRepository>,
    /// Repository for SDK key authentication.
    pub sdk_key_repo: Arc<dyn SdkKeyRepository>,
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

/// `GET /api-docs/openapi.json` — Serve the OpenAPI 3.x JSON document.
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
        AppState {
            db,
            metrics_handle,
            segment_repo,
            flag_repo,
            variant_repo,
            sdk_key_repo,
        }
    }

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

        let stub = Arc::new(StubRepo);
        AppState {
            db: PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_stub")
                .expect("lazy pool"),
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            segment_repo: stub.clone(),
            flag_repo: stub.clone(),
            variant_repo: stub.clone(),
            sdk_key_repo: stub,
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
