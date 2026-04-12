//! Stitchd server: Axum REST Admin API + tonic gRPC SDK API.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod telemetry;
pub mod api;
pub mod startup;

use std::sync::Arc;
use axum::{Json, Router, extract::State, routing::get};
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Serialize;
use sqlx::PgPool;
use stitchd_db::SegmentRepository;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Postgres connection pool.
    pub db: PgPool,
    /// Prometheus metrics handle.
    pub metrics_handle: PrometheusHandle,
    /// Repository for segment management.
    pub segment_repo: Arc<dyn SegmentRepository>,
}

/// Build the Axum router.
///
/// Currently only exposes infrastructure endpoints (`/health`, `/metrics`).
/// Feature routes will be added in subsequent tracks.
#[must_use]
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .merge(api::router::build_api_router())
        .with_state(state)
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
        let segment_repo = std::sync::Arc::new(stitchd_db::repository::pg::PgSegmentRepository::new(db.clone(), audit));
        AppState { db, metrics_handle, segment_repo }
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
