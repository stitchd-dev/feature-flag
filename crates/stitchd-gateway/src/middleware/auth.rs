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
    CredentialRequest, RbacContext,
    auth_service_client::AuthServiceClient,
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
        Request::builder()
            .body(axum::body::Body::empty())
            .unwrap()
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
        assert!(matches!(
            cred.credential,
            Some(Credential::BearerToken(_))
        ));
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
        assert!(matches!(
            cred.credential,
            Some(Credential::BearerToken(_))
        ));
    }
}
