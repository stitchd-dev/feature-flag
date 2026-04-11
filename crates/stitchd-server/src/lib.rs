//! Stitchd server: Axum REST Admin API + tonic gRPC SDK API.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod telemetry;

use axum::{Router, extract::State, routing::get};
use metrics_exporter_prometheus::PrometheusHandle;

/// Build the Axum router.
///
/// Currently only exposes infrastructure endpoints (`/health`, `/metrics`).
/// Feature routes will be added in subsequent tracks.
pub fn build_router(metrics_handle: PrometheusHandle) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(metrics_handle)
}

/// `GET /health` — liveness probe.
async fn health_handler() -> &'static str {
    "ok"
}

/// `GET /metrics` — Prometheus scrape endpoint.
async fn metrics_handler(State(handle): State<PrometheusHandle>) -> String {
    handle.render()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::{Request, StatusCode}};
    use tower::ServiceExt as _;

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle();
        let app = build_router(handle);
        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_endpoint_returns_ok() {
        let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle();
        let app = build_router(handle);
        let response = app
            .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
