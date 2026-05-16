//! SDK auth middleware: validates `x-sdk-key`, resolves the environment, and
//! injects an [`SdkContext`] as a request extension for downstream SDK routes.
//!
//! ### Why a separate middleware from [`crate::middleware::auth::auth_middleware`]?
//!
//! 1. **SDK-only auth path** — rejects bearer tokens. SDK routes MUST NOT
//!    accept human-user JWTs; that confusion would let admin tooling write
//!    to SDK-only telemetry endpoints.
//! 2. **Strongly-typed context** — downstream handlers get [`SdkContext`]
//!    (just `environment_id` + `organisation_id` + `sdk_key_id`), not the
//!    full `RbacContext` (which carries roles/permissions irrelevant to SDKs).
//! 3. **Cache reuse** — under the hood we still call
//!    `AuthService.ValidateCredential`, which internally consults
//!    `SdkKeyCache` (from `db_optim_20260516`). The TTL-cached lookup means
//!    SDK auth is sub-millisecond after first hit.

use std::sync::Arc;

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use tokio::sync::Mutex;
use tonic::transport::Channel;

use stitchd_proto::auth::v1::{
    CredentialRequest, RbacContext, auth_service_client::AuthServiceClient,
    credential_request::Credential,
};

/// Resolved SDK identity, injected by [`sdk_auth_middleware`] for SDK routes.
///
/// Fields here are the ONLY subset of `RbacContext` the SDK-facing surface
/// needs to forward to backend services. `organisation_id` is included for
/// log correlation; backend services do not read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkContext {
    /// UUID of the environment the SDK key is scoped to (forwarded as
    /// `x-env-id` gRPC metadata to backend services).
    pub environment_id: String,
    /// UUID of the org the environment belongs to. Carried for log
    /// correlation only.
    pub organisation_id: String,
    /// UUID of the resolved SDK key record (subject of the RbacContext).
    /// Used for audit logging.
    pub sdk_key_id: String,
}

impl SdkContext {
    /// Build an `SdkContext` from a successfully-validated `RbacContext`.
    ///
    /// `RbacContext.tenant_id` carries the organisation UUID,
    /// `environment_id` carries the env UUID, and `subject` carries the
    /// SDK key record UUID (per `proto/auth/v1/auth_service.proto`).
    #[must_use]
    pub fn from_rbac(rbac: &RbacContext) -> Self {
        Self {
            environment_id: rbac.environment_id.clone(),
            organisation_id: rbac.tenant_id.clone(),
            sdk_key_id: rbac.subject.clone(),
        }
    }
}

/// Extract the SDK key from `x-sdk-key` header. Returns `None` if absent or
/// the header value isn't valid UTF-8.
fn extract_sdk_key(req: &Request) -> Option<String> {
    req.headers()
        .get("x-sdk-key")
        .and_then(|v| v.to_str().ok())
        .map(std::string::ToString::to_string)
}

/// 401 response with a machine-readable error code.
fn unauthorized(error: &str, message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({ "error": error, "message": message })),
    )
        .into_response()
}

/// Axum middleware that validates `x-sdk-key` via the Auth Service and
/// injects an [`SdkContext`] into request extensions.
///
/// Rejects with `401 Unauthorized` when:
/// - No `x-sdk-key` header present (`error: "missing_sdk_key"`).
/// - Auth Service rejects the key — invalid, revoked, or unknown
///   (`error: "invalid_sdk_key"`).
///
/// Bearer tokens are intentionally NOT accepted on SDK routes — even if the
/// caller supplies a valid JWT, requests reach here with `401` if they lack
/// `x-sdk-key`. This is defence in depth: SDK telemetry surfaces should only
/// be hit by embedded SDKs, never by admin UI or other human-user flows.
pub async fn sdk_auth_middleware(
    axum::extract::State(auth_client): axum::extract::State<Arc<Mutex<AuthServiceClient<Channel>>>>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(sdk_key) = extract_sdk_key(&req) else {
        return unauthorized("missing_sdk_key", "x-sdk-key header required for SDK routes");
    };

    let credential_req = CredentialRequest {
        credential: Some(Credential::SdkKey(sdk_key)),
    };

    let result = {
        let mut client = auth_client.lock().await;
        client
            .validate_credential(tonic::Request::new(credential_req))
            .await
    };

    match result {
        Ok(resp) => {
            let rbac = resp.into_inner();
            let sdk_ctx = SdkContext::from_rbac(&rbac);
            // Insert SDK-specific context — handlers extract this, not RbacContext.
            req.extensions_mut().insert(sdk_ctx);
            // Also keep the RbacContext so the existing routes/utilities can use it.
            req.extensions_mut().insert(rbac);
            next.run(req).await
        }
        Err(status) => {
            tracing::debug!(code = ?status.code(), "SDK key rejected: {}", status.message());
            unauthorized("invalid_sdk_key", "SDK key is invalid, revoked, or unknown")
        }
    }
}

/// Extract the `SdkContext` from a request extension. Returns `None` if the
/// SDK middleware did not run (i.e. the route is misconfigured).
#[must_use]
pub fn sdk_context(req: &Request) -> Option<&SdkContext> {
    req.extensions().get::<SdkContext>()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;

    // ── Pure helpers ────────────────────────────────────────────────────────

    fn req_with_sdk_key(key: &str) -> Request {
        HttpRequest::builder()
            .header("x-sdk-key", key)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    fn req_no_auth() -> Request {
        HttpRequest::builder()
            .body(axum::body::Body::empty())
            .unwrap()
    }

    fn req_with_bearer(token: &str) -> Request {
        HttpRequest::builder()
            .header("authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap()
    }

    #[test]
    fn extract_sdk_key_present() {
        let req = req_with_sdk_key("sk_test_123");
        assert_eq!(extract_sdk_key(&req).as_deref(), Some("sk_test_123"));
    }

    #[test]
    fn extract_sdk_key_absent() {
        assert!(extract_sdk_key(&req_no_auth()).is_none());
    }

    #[test]
    fn extract_sdk_key_ignores_bearer() {
        // Critical: a request with only a Bearer token must NOT yield an SDK key.
        let req = req_with_bearer("a-valid-jwt");
        assert!(extract_sdk_key(&req).is_none());
    }

    // ── SdkContext::from_rbac ───────────────────────────────────────────────

    #[test]
    fn sdk_context_from_rbac_maps_fields() {
        let rbac = RbacContext {
            tenant_id: "11111111-1111-1111-1111-111111111111".into(),
            environment_id: "22222222-2222-2222-2222-222222222222".into(),
            subject: "33333333-3333-3333-3333-333333333333".into(),
            roles: vec!["sdk_key".into()],
            permissions: vec![],
            is_system: false,
        };
        let ctx = SdkContext::from_rbac(&rbac);
        assert_eq!(ctx.organisation_id, "11111111-1111-1111-1111-111111111111");
        assert_eq!(ctx.environment_id, "22222222-2222-2222-2222-222222222222");
        assert_eq!(ctx.sdk_key_id, "33333333-3333-3333-3333-333333333333");
    }

    #[test]
    fn sdk_context_clones_strings_does_not_borrow() {
        let rbac = RbacContext {
            tenant_id: "org".into(),
            environment_id: "env".into(),
            subject: "key".into(),
            ..Default::default()
        };
        let ctx = SdkContext::from_rbac(&rbac);
        drop(rbac); // borrow-free
        assert_eq!(ctx.environment_id, "env");
    }

    // ── sdk_context() extension helper ──────────────────────────────────────

    #[test]
    fn sdk_context_helper_returns_inserted_value() {
        let mut req = req_no_auth();
        let ctx = SdkContext {
            environment_id: "env".into(),
            organisation_id: "org".into(),
            sdk_key_id: "key".into(),
        };
        req.extensions_mut().insert(ctx.clone());
        assert_eq!(sdk_context(&req), Some(&ctx));
    }

    #[test]
    fn sdk_context_helper_returns_none_when_unset() {
        let req = req_no_auth();
        assert!(sdk_context(&req).is_none());
    }

    // ── Middleware behaviour: missing key ──────────────────────────────────
    //
    // Full middleware integration tests live in the gateway's integration
    // test suite (they require a running auth-service mock). The unit tests
    // here cover the missing-credential short-circuit path, which doesn't
    // touch the auth client at all.

    #[tokio::test]
    async fn middleware_returns_401_when_x_sdk_key_missing() {
        use axum::{Router, middleware::from_fn_with_state, routing::get};
        use stitchd_proto::auth::v1::auth_service_client::AuthServiceClient;
        use tower::ServiceExt;

        // A dummy client; never actually called because the missing-key check
        // short-circuits before the auth call.
        // tonic::transport::Channel::balance_channel returns a channel that
        // never connects — the request will fail the missing-key check
        // before any network call is attempted.
        let (channel, _tx) = tonic::transport::Channel::balance_channel::<usize>(1);
        let client: Arc<Mutex<AuthServiceClient<Channel>>> =
            Arc::new(Mutex::new(AuthServiceClient::new(channel)));

        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(from_fn_with_state(client, sdk_auth_middleware));

        let resp = app.oneshot(req_no_auth()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn middleware_returns_401_for_bearer_only_request() {
        use axum::{Router, middleware::from_fn_with_state, routing::get};
        use tower::ServiceExt;

        let (channel, _tx) = tonic::transport::Channel::balance_channel::<usize>(1);
        let client: Arc<Mutex<AuthServiceClient<Channel>>> =
            Arc::new(Mutex::new(AuthServiceClient::new(channel)));

        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(from_fn_with_state(client, sdk_auth_middleware));

        // Bearer-only requests have no x-sdk-key, so they get 401 BEFORE the
        // auth client is called. This proves bearer tokens are not silently
        // accepted on SDK routes.
        let resp = app.oneshot(req_with_bearer("some-jwt")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
