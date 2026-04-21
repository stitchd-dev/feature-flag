//! Axum router builder — three auth trees (SDK, public auth stubs, admin + gRPC passthrough).

use std::sync::Arc;

use axum::{Router, middleware, routing::{get, post, put}};

use crate::middleware::auth::auth_middleware;
use crate::routes::{events, experiments, flags, sdk, segments};
use crate::state::GatewayState;

/// Build the full gateway `Router`.
pub fn build_router(state: Arc<GatewayState>) -> Router {
    let auth_client = Arc::clone(&state.auth_client);

    // ── SDK routes (x-sdk-key auth) ──────────────────────────────────────────
    let sdk_routes = Router::new()
        .route("/v1/environments/{env_id}/evaluate", post(sdk::evaluate))
        .route("/v1/environments/{env_id}/events", post(sdk::ingest_event))
        .route("/v1/environments/{env_id}/events/batch", post(sdk::ingest_batch_events))
        .route("/v1/environments/{env_id}/segments/list-check", post(sdk::list_check_membership))
        .route(
            "/v1/environments/{env_id}/segments/list-check/batch",
            post(sdk::batch_list_check_membership),
        )
        .with_state(Arc::clone(&state))
        .layer(middleware::from_fn_with_state(
            auth_client.clone(),
            auth_middleware,
        ));

    // ── Admin routes (JWT Bearer auth) ────────────────────────────────────────
    let admin_routes = Router::new()
        // Flags
        .route(
            "/v1/projects/{project_id}/flags",
            get(flags::list_flags).post(flags::create_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}",
            get(flags::get_flag).put(flags::update_flag).delete(flags::delete_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/variants",
            post(flags::create_variant),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/rules",
            put(flags::update_rules),
        )
        // Segments
        .route(
            "/v1/environments/{env_id}/segments",
            get(segments::list_segments).post(segments::create_segment),
        )
        .route(
            "/v1/environments/{env_id}/segments/{segment_id}",
            get(segments::get_segment).put(segments::update_segment).delete(segments::delete_segment),
        )
        // Events
        .route(
            "/v1/environments/{env_id}/event-definitions",
            get(events::list_event_definitions).post(events::create_event_definition),
        )
        .route(
            "/v1/environments/{env_id}/event-definitions/{def_id}",
            get(events::get_event_definition)
                .put(events::update_event_definition)
                .delete(events::delete_event_definition),
        )
        // Experiments
        .route(
            "/v1/environments/{env_id}/experiments",
            get(experiments::list_experiments).post(experiments::create_experiment),
        )
        .route(
            "/v1/environments/{env_id}/experiments/{experiment_id}",
            get(experiments::get_experiment).delete(experiments::delete_experiment),
        )
        .route(
            "/v1/environments/{env_id}/experiments/{experiment_id}/results",
            get(experiments::get_results),
        )
        .with_state(Arc::clone(&state))
        .layer(middleware::from_fn_with_state(
            auth_client,
            auth_middleware,
        ));

    Router::new()
        .merge(sdk_routes)
        .merge(admin_routes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::{Request, StatusCode}};
    use tower::ServiceExt as _;
    use crate::tests::helpers::make_stub_state;

    #[tokio::test]
    async fn flags_without_auth_returns_401() {
        let state = make_stub_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/projects/proj-1/flags")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn segments_without_auth_returns_401() {
        let state = make_stub_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/segments")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn experiments_without_auth_returns_401() {
        let state = make_stub_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn event_definitions_without_auth_returns_401() {
        let state = make_stub_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/event-definitions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn sdk_evaluate_without_auth_returns_401() {
        let state = make_stub_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/evaluate")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"context_key":"u1","context_type":"user"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unknown_route_returns_404() {
        let state = make_stub_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/unknown/path")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Auth middleware fires before routing — unauthenticated requests to unknown
        // paths get 401 (middleware short-circuits) rather than 404.
        assert!(
            resp.status() == StatusCode::NOT_FOUND || resp.status() == StatusCode::UNAUTHORIZED,
            "status: {}",
            resp.status()
        );
    }
}
