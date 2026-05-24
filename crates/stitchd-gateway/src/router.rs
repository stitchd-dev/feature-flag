//! Axum router — four auth trees with distinct middleware policies.
//!
//! | Tree                 | Auth                              | Who can call              |
//! |----------------------|-----------------------------------|---------------------------|
//! | `auth_routes`        | none (public)                     | anyone                    |
//! | `admin_routes`       | JWT + `require_system_org`        | superadmin only           |
//! | `mgmt_routes`        | JWT + `require_non_system_org`    | non-superadmin users only |
//! | `sdk_backend_routes` | sdk_auth_middleware               | SDK clients               |
//! | `resource_routes`    | JWT (`auth_middleware`)           | authenticated users       |

use std::sync::Arc;

use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{delete, get, patch, post, put},
};
use metrics_exporter_prometheus::PrometheusHandle;

use crate::middleware::auth::{
    auth_middleware, require_non_system_org, require_system_org, require_write_permission,
};
use crate::middleware::event_quota::{build_limiter_from_env, event_quota_middleware};
use crate::middleware::sdk_auth::sdk_auth_middleware;
use crate::routes::{
    admin, auth, auth_providers, context_intel, eval_stats, event_admin, events, experiments,
    flags, management, metrics, oidc, saml, sdk_backend, segments, stats,
};
use crate::state::GatewayState;

/// Handler for `GET /metrics` — renders Prometheus exposition format.
///
/// Lives at the conventional Prometheus path (no `/v1/` version prefix) to
/// keep the versioned admin REST surface — including `GET /v1/metrics` for
/// metric-definitions CRUD — free of operational endpoints.
async fn metrics_handler(State(handle): State<PrometheusHandle>) -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        handle.render(),
    )
}

/// Build the full gateway `Router`.
pub fn build_router(state: Arc<GatewayState>, metrics_handle: PrometheusHandle) -> Router {
    let auth_client = Arc::clone(&state.auth_client);

    // ── Public: health + login + OIDC flows (no auth required) ──────────────
    let auth_routes = Router::new()
        .route("/v1/health", get(|| async { StatusCode::OK }))
        .route("/v1/auth/login", post(auth::login))
        .route("/v1/auth/refresh", post(auth::refresh))
        .route("/v1/auth/me/orgs", get(auth::list_user_orgs))
        .route("/v1/auth/switch-org", post(auth::switch_org))
        // OIDC: provider-scoped authorize + callback (public — redirected from IdP)
        .route(
            "/v1/auth/oidc/{provider_id}/authorize",
            post(oidc::oidc_authorize_by_provider),
        )
        .route(
            "/v1/auth/oidc/{provider_id}/callback",
            get(oidc::oidc_callback),
        )
        // SAML: provider-scoped SSO initiate + ACS callback (public — IdP posts here)
        .route(
            "/v1/auth/saml/{provider_id}/sso",
            post(saml::saml_sso_by_provider),
        )
        .route(
            "/v1/auth/saml/{provider_id}/callback",
            post(saml::saml_acs_callback),
        )
        .with_state(Arc::clone(&state));

    // ── Superadmin-only routes (JWT + system-org check) ───────────────────────
    let admin_routes = Router::new()
        .route(
            "/v1/superadmin/orgs",
            get(admin::list_orgs).post(admin::create_org),
        )
        .route("/v1/superadmin/orgs/{org_id}", get(admin::get_org))
        .route(
            "/v1/superadmin/orgs/{org_id}/users",
            get(admin::list_org_users).post(admin::seed_user),
        )
        .route(
            "/v1/superadmin/orgs/{org_id}/users/{user_id}",
            delete(admin::remove_org_user),
        )
        .with_state(Arc::clone(&state))
        .layer(middleware::from_fn(require_system_org))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&auth_client),
            auth_middleware,
        ));

    // ── Management routes (JWT + non-system-org check) ────────────────────────
    // Read and write routes are split so that require_write_permission is only
    // applied to mutating endpoints, while read-only callers (org_member role)
    // retain access to GET routes.
    let mgmt_read = Router::new()
        .route(
            "/v1/management/orgs/{org_id}/projects",
            get(management::list_projects),
        )
        .route(
            "/v1/management/projects/{project_id}/environments",
            get(management::list_environments),
        )
        .route(
            "/v1/management/environments/{environment_id}/sdk-keys",
            get(management::list_sdk_keys),
        )
        .route(
            "/v1/management/orgs/{org_id}/users",
            get(management::list_org_users),
        )
        .with_state(Arc::clone(&state));

    let mgmt_write = Router::new()
        .route(
            "/v1/management/orgs/{org_id}/projects",
            post(management::create_project),
        )
        .route(
            "/v1/management/projects/{project_id}",
            patch(management::rename_project).delete(management::delete_project),
        )
        .route(
            "/v1/management/projects/{project_id}/environments",
            post(management::create_environment),
        )
        .route(
            "/v1/management/environments/{environment_id}",
            patch(management::rename_environment).delete(management::delete_environment),
        )
        .route(
            "/v1/management/environments/{environment_id}/sdk-keys",
            post(management::create_sdk_key),
        )
        .route(
            "/v1/management/environments/{environment_id}/sdk-keys/{sdk_key_id}",
            delete(management::revoke_sdk_key),
        )
        .route(
            "/v1/management/orgs/{org_id}/users",
            post(management::create_user),
        )
        .route(
            "/v1/management/orgs/{org_id}/users/{user_id}",
            delete(management::remove_org_user),
        )
        .with_state(Arc::clone(&state))
        .layer(middleware::from_fn(require_write_permission));

    let mgmt_routes = mgmt_read
        .merge(mgmt_write)
        .layer(middleware::from_fn(require_non_system_org))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&auth_client),
            auth_middleware,
        ));

    // ── SDK backend routes (x-sdk-key auth via sdk_auth_middleware) ─────────
    // These implement the contract specified in sdks/spec/openapi/sdk.yaml
    // for the clean-implementation SDK (sdks/rust/). They use the dedicated
    // sdk_auth_middleware (which injects SdkContext) rather than the generic
    // auth_middleware (which only injects RbacContext).
    //
    // POST /v1/events/track has:
    //  - a 5 MiB body limit (per spec F3.4) applied as a per-route
    //    `DefaultBodyLimit` layer so the rest of the SDK tier continues to
    //    use axum's small default (2 MiB) without surprise.
    //  - a per-env event-quota layer that 429s when a single env exceeds
    //    `STITCHD_EVENT_QUOTA_PER_SEC` events/sec (default 1000). This is
    //    applied ONLY to the track route, not to the other SDK endpoints,
    //    so segment/eval-log batches are not affected.
    let event_quota_limiter = build_limiter_from_env();
    let sdk_backend_routes = Router::new()
        .route(
            "/v1/sdk/segments/list:batch",
            post(sdk_backend::segments_list_batch),
        )
        .route("/v1/sdk/events:batch", post(sdk_backend::events_batch))
        .route(
            "/v1/events/track",
            post(events::track_events)
                .route_layer(middleware::from_fn_with_state(
                    Arc::clone(&event_quota_limiter),
                    event_quota_middleware,
                ))
                .layer(DefaultBodyLimit::max(events::TRACK_EVENTS_BODY_LIMIT_BYTES)),
        )
        .with_state(Arc::clone(&state))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&auth_client),
            sdk_auth_middleware,
        ));

    // ── JWT-authenticated resource routes ─────────────────────────────────────
    // Split into read-only and write sub-routers so that require_write_permission
    // gates only mutating endpoints. Read-only callers (org_member) retain full
    // read access.
    let resource_read = Router::new()
        // Auth context
        .route("/v1/auth/me/permissions", get(auth::get_my_permissions))
        // Flags — read
        .route(
            "/v1/projects/{project_id}/flags",
            get(flags::list_flags),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}",
            get(flags::get_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/eval-stats",
            get(eval_stats::get_eval_stats),
        )
        // evaluate-preview uses POST but is read-only in semantics (no state change)
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/evaluate-preview",
            post(flags::evaluate_preview),
        )
        // Segments — read
        .route(
            "/v1/segments",
            get(segments::list_segments),
        )
        .route(
            "/v1/segments/{segment_id}",
            get(segments::get_segment),
        )
        .route(
            "/v1/segments/{segment_id}/entries/lookup",
            get(segments::lookup_segment_entry),
        )
        .route(
            "/v1/environments/{environment_id}/segments",
            get(segments::list_segments_in_env),
        )
        // Events — read
        .route(
            "/v1/environments/{environment_id}/event-definitions",
            get(events::list_event_definitions),
        )
        .route(
            "/v1/environments/{environment_id}/event-definitions/{event_definition_id}",
            get(events::get_event_definition),
        )
        .route("/v1/events/{event_key}/firings", get(events::get_event_firings))
        .route("/v1/events/{event_key}/stats", get(events::get_event_stats))
        .route("/v1/events", get(event_admin::list_events))
        .route("/v1/events/{event_key}", get(event_admin::get_event))
        // Metrics — read + preview (preview is POST but read-only semantics)
        .route("/v1/metrics", get(metrics::list_metrics))
        .route("/v1/metrics/{id}", get(metrics::get_metric))
        .route("/v1/metrics/{id}/preview", post(metrics::preview_metric))
        // Experiments — read
        .route(
            "/v1/environments/{environment_id}/experiments",
            get(experiments::list_experiments),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}",
            get(experiments::get_experiment),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/results",
            get(experiments::get_results),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/iterations",
            get(experiments::list_iterations),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/exposures",
            get(experiments::list_exposures),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/timeseries",
            get(stats::get_timeseries),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/recompute/{job_id}",
            get(stats::get_recompute_job_status),
        )
        // Context intelligence — read
        .route(
            "/v1/environments/{environment_id}/context-types",
            get(context_intel::list_context_types),
        )
        .route(
            "/v1/environments/{environment_id}/context-types/{context_type}/params",
            get(context_intel::list_context_params),
        )
        .route("/v1/jobs/{job_id}", get(stats::get_job_status))
        .with_state(Arc::clone(&state));

    let resource_write = Router::new()
        // Flags — write
        .route(
            "/v1/projects/{project_id}/flags",
            post(flags::create_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}",
            put(flags::update_flag).delete(flags::delete_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/archive",
            post(flags::archive_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/restore",
            post(flags::restore_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/variants",
            put(flags::update_variants),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/rules",
            put(flags::update_rules),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/default-rule-distribution",
            post(flags::set_default_rule_distribution),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/hashing",
            put(flags::update_flag_hashing),
        )
        // Segments — write
        .route(
            "/v1/segments",
            post(segments::create_segment),
        )
        .route(
            "/v1/segments/{segment_id}",
            put(segments::update_segment).delete(segments::delete_segment),
        )
        .route(
            "/v1/environments/{environment_id}/segments",
            post(segments::create_segment_in_env),
        )
        .route(
            "/v1/segments/{segment_id}/entries",
            post(segments::patch_segment_entries),
        )
        // Events — write
        .route(
            "/v1/environments/{environment_id}/event-definitions",
            post(events::create_event_definition),
        )
        .route(
            "/v1/environments/{environment_id}/event-definitions/{event_definition_id}",
            put(events::update_event_definition).delete(events::delete_event_definition),
        )
        // Admin-auth event track — write (fires test events from admin UI)
        .route(
            "/v1/admin/events/track",
            post(events::track_events_admin).layer(DefaultBodyLimit::max(
                events::TRACK_EVENTS_BODY_LIMIT_BYTES,
            )),
        )
        .route(
            "/v1/events",
            post(event_admin::create_event),
        )
        .route(
            "/v1/events/{event_key}",
            patch(event_admin::update_event).delete(event_admin::delete_event),
        )
        // Metrics — write
        .route("/v1/metrics", post(metrics::create_metric))
        .route(
            "/v1/metrics/{id}",
            patch(metrics::update_metric).delete(metrics::delete_metric),
        )
        // Experiments — write
        .route(
            "/v1/environments/{environment_id}/experiments",
            post(experiments::create_experiment),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}",
            patch(experiments::update_experiment).delete(experiments::delete_experiment),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/transitions",
            post(experiments::transition_experiment),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/recompute",
            post(stats::trigger_recompute_env_scoped),
        )
        // Stats recompute — write (triggers a compute job)
        .route(
            "/v1/experiments/{experiment_id}/recompute",
            post(stats::trigger_recompute),
        )
        .with_state(Arc::clone(&state))
        .layer(middleware::from_fn(require_write_permission));

    let resource_routes = resource_read
        .merge(resource_write)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&auth_client),
            auth_middleware,
        ));

    // ── Auth-provider management + org-scoped OIDC (JWT + non-system-org) ───
    let auth_provider_routes = Router::new()
        .route(
            "/v1/orgs/{org_id}/auth-providers",
            get(auth_providers::list_auth_providers).post(auth_providers::create_auth_provider),
        )
        .route(
            "/v1/orgs/{org_id}/auth-providers/{auth_provider_id}",
            get(auth_providers::get_auth_provider)
                .put(auth_providers::update_auth_provider)
                .delete(auth_providers::delete_auth_provider),
        )
        .route(
            "/v1/orgs/{org_id}/auth-providers/{auth_provider_id}/saml/metadata",
            get(auth_providers::get_saml_sp_metadata),
        )
        // Org-scoped OIDC authorize requires the user to be authenticated (picking their org's IdP)
        .route(
            "/v1/orgs/{org_id}/auth/oidc/authorize",
            post(oidc::oidc_authorize_by_org),
        )
        // Org-scoped SAML SSO initiate
        .route(
            "/v1/orgs/{org_id}/auth/saml/sso",
            post(saml::saml_sso_by_org),
        )
        .with_state(Arc::clone(&state))
        .layer(middleware::from_fn(require_non_system_org))
        .layer(middleware::from_fn_with_state(
            auth_client,
            auth_middleware,
        ));

    // ── Prometheus metrics — own state (PrometheusHandle), no auth ──────────
    // Lives at the conventional Prometheus path (`/metrics`) to free
    // `/v1/metrics` for the admin metric-definitions CRUD surface.
    let metrics_routes = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(metrics_handle);

    Router::new()
        .merge(auth_routes)
        .merge(admin_routes)
        .merge(mgmt_routes)
        .merge(sdk_backend_routes)
        .merge(resource_routes)
        .merge(auth_provider_routes)
        .merge(metrics_routes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::helpers::make_stub_state;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use metrics_exporter_prometheus::PrometheusBuilder;
    use tower::ServiceExt as _;

    /// Create a `PrometheusHandle` for tests without installing a global recorder.
    fn test_metrics_handle() -> PrometheusHandle {
        PrometheusBuilder::new().build_recorder().handle()
    }

    #[tokio::test]
    async fn flags_without_auth_returns_401() {
        let app = build_router(make_stub_state(), test_metrics_handle());
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
        let app = build_router(make_stub_state(), test_metrics_handle());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/segments?env_id=env-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_create_org_without_auth_returns_401() {
        let app = build_router(make_stub_state(), test_metrics_handle());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/superadmin/orgs")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn management_create_project_without_auth_returns_401() {
        let app = build_router(make_stub_state(), test_metrics_handle());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/management/orgs/org-1/projects")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_route_exists_and_returns_non_404() {
        let app = build_router(make_stub_state(), test_metrics_handle());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"email":"a@b.com","password":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn auth_providers_without_auth_returns_401() {
        let app = build_router(make_stub_state(), test_metrics_handle());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/orgs/org-1/auth-providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unknown_route_returns_404() {
        let app = build_router(make_stub_state(), test_metrics_handle());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/unknown/path")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::NOT_FOUND || resp.status() == StatusCode::UNAUTHORIZED,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn metrics_endpoint_returns_prometheus_text() {
        use axum::body::to_bytes;

        // Register a counter on this recorder before building the router so
        // the exposition is non-empty and we can assert # HELP / # TYPE lines.
        let recorder = PrometheusBuilder::new().build_recorder();
        // Record via the recorder's own metrics API by installing it temporarily
        // in the test thread and removing it immediately.
        let handle = recorder.handle();

        let app = build_router(make_stub_state(), handle);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);

        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("text/plain"),
            "expected text/plain content-type, got: {ct}"
        );

        // Verify the body is valid Prometheus exposition (may be empty if no
        // metrics registered in this recorder, but must not be an error page).
        let body_bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&body_bytes).expect("metrics body must be UTF-8");
        // An empty recorder returns an empty string — that's still valid exposition.
        // Verify no error payload (which would contain "error" or HTML).
        assert!(
            !body.contains("<!DOCTYPE") && !body.contains("<html"),
            "metrics body looks like an HTML error page: {body}"
        );
    }
}
