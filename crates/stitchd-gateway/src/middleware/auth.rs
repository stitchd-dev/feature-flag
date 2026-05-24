//! Auth middleware: extract credential from request headers, call Auth Service,
//! inject `RbacContext` as a request extension.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use tokio::sync::Mutex;

use stitchd_proto::auth::v1::{
    CredentialRequest, RbacContext, auth_service_client::AuthServiceClient,
    credential_request::Credential,
};
use tonic::transport::Channel;

/// Extract a `Bearer` token from the `Authorization` header.
fn extract_bearer(req: &Request) -> Option<String> {
    req.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// Extract an SDK key from the `x-sdk-key` header.
fn extract_sdk_key(req: &Request) -> Option<String> {
    req.headers()
        .get("x-sdk-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Build a [`CredentialRequest`] from the incoming HTTP request.
///
/// Returns `None` if no recognised credential header is present.
pub fn build_credential(req: &Request) -> Option<CredentialRequest> {
    if let Some(token) = extract_bearer(req) {
        return Some(CredentialRequest {
            credential: Some(Credential::BearerToken(token)),
        });
    }
    if let Some(key) = extract_sdk_key(req) {
        return Some(CredentialRequest {
            credential: Some(Credential::SdkKey(key)),
        });
    }
    None
}

/// Axum middleware that validates credentials via the Auth Service and injects
/// the returned [`RbacContext`] as a request extension.
///
/// Returns `401 Unauthorized` when:
/// - No credential header is present.
/// - The Auth Service rejects the credential.
pub async fn auth_middleware(
    axum::extract::State(auth_client): axum::extract::State<Arc<Mutex<AuthServiceClient<Channel>>>>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(credential_req) = build_credential(&req) else {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "error": "missing credentials" })),
        )
            .into_response();
    };

    let result = {
        let mut client = auth_client.lock().await;
        client
            .validate_credential(tonic::Request::new(credential_req))
            .await
    };

    match result {
        Ok(resp) => {
            req.extensions_mut().insert(resp.into_inner());
            next.run(req).await
        }
        Err(status) => {
            let msg = status.message().to_string();
            (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({ "error": msg })),
            )
                .into_response()
        }
    }
}

/// Extract an `RbacContext` from a request extension, returning `None` if absent.
/// Handlers that need the auth context call this helper.
pub fn rbac_context(req: &Request) -> Option<&RbacContext> {
    req.extensions().get::<RbacContext>()
}

/// Middleware that allows only callers whose token has `is_system = true`.
/// Used on `/admin` routes — only superadmin (System-org users) may access them.
/// Returns `403 Forbidden` for non-system callers.
pub async fn require_system_org(req: Request, next: Next) -> Response {
    match req.extensions().get::<RbacContext>() {
        Some(ctx) if ctx.is_system => next.run(req).await,
        _ => (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "error": "superadmin access required" })),
        )
            .into_response(),
    }
}

/// Middleware that blocks callers whose token has `is_system = true`.
/// Used on `/management` routes — System-org users (superadmins) must not
/// call org-scoped management APIs.
/// Returns `403 Forbidden` for system-org callers.
pub async fn require_non_system_org(req: Request, next: Next) -> Response {
    match req.extensions().get::<RbacContext>() {
        Some(ctx) if ctx.is_system => (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "error": "superadmin cannot use management APIs" })),
        )
            .into_response(),
        Some(_) => next.run(req).await,
        None => (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "error": "missing credentials" })),
        )
            .into_response(),
    }
}

/// Middleware that enforces write-access permissions on mutating routes.
///
/// Checks that the caller's `RbacContext.permissions` contains at least one
/// write-capable permission (any action that modifies state). Returns 403 for
/// read-only roles such as `org_member`.
pub async fn require_write_permission(req: Request, next: Next) -> Response {
    match req.extensions().get::<RbacContext>() {
        Some(ctx) if ctx.permissions.iter().any(is_write_action) => next.run(req).await,
        Some(_) => (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "error": "write access required" })),
        )
            .into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "error": "missing credentials" })),
        )
            .into_response(),
    }
}

fn is_write_action(p: &String) -> bool {
    p.ends_with(":write")
        || p.ends_with(":create")
        || p.ends_with(":delete")
        || p.ends_with(":rename")
        || p.ends_with(":revoke")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn req_with_bearer(token: &str) -> Request<axum::body::Body> {
        Request::builder()
            .header("authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap()
    }

    fn req_with_sdk_key(key: &str) -> Request<axum::body::Body> {
        Request::builder()
            .header("x-sdk-key", key)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    fn req_no_auth() -> Request<axum::body::Body> {
        Request::builder().body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn extract_bearer_finds_token() {
        let req = req_with_bearer("mytoken");
        assert_eq!(extract_bearer(&req), Some("mytoken".to_string()));
    }

    #[test]
    fn extract_bearer_absent_returns_none() {
        let req = req_no_auth();
        assert_eq!(extract_bearer(&req), None);
    }

    #[test]
    fn extract_sdk_key_finds_key() {
        let req = req_with_sdk_key("sdk-123");
        assert_eq!(extract_sdk_key(&req), Some("sdk-123".to_string()));
    }

    #[test]
    fn extract_sdk_key_absent_returns_none() {
        let req = req_no_auth();
        assert_eq!(extract_sdk_key(&req), None);
    }

    #[test]
    fn build_credential_bearer() {
        let req = req_with_bearer("tok");
        let cred = build_credential(&req).unwrap();
        assert!(matches!(cred.credential, Some(Credential::BearerToken(_))));
    }

    #[test]
    fn build_credential_sdk_key() {
        let req = req_with_sdk_key("key");
        let cred = build_credential(&req).unwrap();
        assert!(matches!(cred.credential, Some(Credential::SdkKey(_))));
    }

    #[test]
    fn build_credential_no_auth_returns_none() {
        let req = req_no_auth();
        assert!(build_credential(&req).is_none());
    }

    #[test]
    fn bearer_takes_precedence_over_sdk_key() {
        let req = Request::builder()
            .header("authorization", "Bearer tok")
            .header("x-sdk-key", "key")
            .body(axum::body::Body::empty())
            .unwrap();
        let cred = build_credential(&req).unwrap();
        assert!(matches!(cred.credential, Some(Credential::BearerToken(_))));
    }

    /// Build a request with an `RbacContext` extension already inserted.
    fn req_with_rbac(is_system: bool) -> Request<axum::body::Body> {
        let mut req = Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        req.extensions_mut().insert(RbacContext {
            is_system,
            ..Default::default()
        });
        req
    }

    #[tokio::test]
    async fn require_system_org_rejects_non_system() {
        use axum::{Router, middleware::from_fn, routing::get};
        use tower::ServiceExt;

        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(from_fn(require_system_org));

        let req = req_with_rbac(false);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn require_system_org_allows_system() {
        use axum::{Router, middleware::from_fn, routing::get};
        use tower::ServiceExt;

        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(from_fn(require_system_org));

        let req = req_with_rbac(true);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn require_non_system_org_rejects_system() {
        use axum::{Router, middleware::from_fn, routing::get};
        use tower::ServiceExt;

        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(from_fn(require_non_system_org));

        let req = req_with_rbac(true);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn require_non_system_org_allows_non_system() {
        use axum::{Router, middleware::from_fn, routing::get};
        use tower::ServiceExt;

        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(from_fn(require_non_system_org));

        let req = req_with_rbac(false);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn require_non_system_org_rejects_missing_context() {
        use axum::{Router, middleware::from_fn, routing::get};
        use tower::ServiceExt;

        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(from_fn(require_non_system_org));

        let req = req_no_auth();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn rbac_context_returns_none_when_absent() {
        let req = req_no_auth();
        assert!(rbac_context(&req).is_none());
    }

    #[test]
    fn rbac_context_returns_some_when_present() {
        let mut req = req_no_auth();
        req.extensions_mut().insert(RbacContext {
            is_system: false,
            ..Default::default()
        });
        assert!(rbac_context(&req).is_some());
    }

    fn req_with_permissions(perms: Vec<&str>) -> Request<axum::body::Body> {
        let mut req = Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        req.extensions_mut().insert(RbacContext {
            permissions: perms.iter().map(|p| p.to_string()).collect(),
            ..Default::default()
        });
        req
    }

    #[test]
    fn is_write_action_recognises_write_suffixes() {
        for perm in ["flag:write", "env:create", "env:delete", "env:rename", "sdk_key:revoke"] {
            assert!(is_write_action(&perm.to_string()), "{perm} should be write");
        }
    }

    #[test]
    fn is_write_action_ignores_read_permissions() {
        for perm in ["flag:read", "metric:read", "segment:read"] {
            assert!(!is_write_action(&perm.to_string()), "{perm} should not be write");
        }
    }

    #[tokio::test]
    async fn require_write_permission_blocks_read_only_caller() {
        use axum::{Router, middleware::from_fn, routing::get};
        use tower::ServiceExt;

        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(from_fn(require_write_permission));

        let req = req_with_permissions(vec!["flag:read", "segment:read"]);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn require_write_permission_allows_write_caller() {
        use axum::{Router, middleware::from_fn, routing::get};
        use tower::ServiceExt;

        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(from_fn(require_write_permission));

        let req = req_with_permissions(vec!["flag:read", "flag:write"]);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn require_write_permission_missing_context_returns_401() {
        use axum::{Router, middleware::from_fn, routing::get};
        use tower::ServiceExt;

        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(from_fn(require_write_permission));

        let req = req_no_auth();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_middleware_no_credential_returns_401() {
        use axum::{Router, middleware::from_fn_with_state, routing::get};
        use tonic::transport::Channel;
        use tower::ServiceExt;

        let auth_client = Arc::new(Mutex::new(AuthServiceClient::new(
            Channel::from_static("http://127.0.0.1:1").connect_lazy(),
        )));
        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .route_layer(from_fn_with_state(auth_client, auth_middleware));

        let req = req_no_auth();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_middleware_bearer_grpc_error_returns_401() {
        use axum::{Router, middleware::from_fn_with_state, routing::get};
        use tonic::transport::Channel;
        use tower::ServiceExt;

        let auth_client = Arc::new(Mutex::new(AuthServiceClient::new(
            Channel::from_static("http://127.0.0.1:1").connect_lazy(),
        )));
        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .route_layer(from_fn_with_state(auth_client, auth_middleware));

        let req = req_with_bearer("mytoken");
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }
}
