//! Auth route handlers — login, org listing, org switching.

use axum::{
    Json,
    extract::{Extension, State},
    http::HeaderMap,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::auth::v1::{
    ListUserOrgsRequest, LoginRequest, RbacContext, RefreshTokenRequest, SwitchOrgRequest,
};

use crate::error::GatewayError;
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginBody {
    pub email: String,
    pub password: String,
    pub org_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginJson {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user_id: String,
    pub org_id: String,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `POST /v1/auth/login`
#[utoipa::path(
    post,
    path = "/v1/auth/login",
    tag = "auth",
    request_body = LoginBody,
    responses(
        (status = 200, description = "Login successful", body = LoginJson),
        (status = 401, description = "Invalid credentials"),
        (status = 502, description = "Auth service unavailable"),
    )
)]
pub async fn login(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<LoginBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(LoginRequest {
        email: body.email,
        password: body.password,
        org_id: body.org_id.unwrap_or_default(),
    });
    let mut client = state.auth_client.lock().await;
    let resp = client
        .login_with_password(req)
        .await
        .map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok(Json(LoginJson {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_in: r.expires_in,
        user_id: r.user_id,
        org_id: r.org_id,
    }))
}

// ─── Org helpers ─────────────────────────────────────────────────────────────

fn extract_bearer(headers: &HeaderMap) -> Result<String, GatewayError> {
    let val = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| GatewayError::Unauthorized("missing Authorization header".to_string()))?;
    let token = val
        .strip_prefix("Bearer ")
        .ok_or_else(|| GatewayError::Unauthorized("expected Bearer token".to_string()))?
        .to_string();
    Ok(token)
}

#[derive(Debug, Serialize)]
pub struct OrgEntryJson {
    pub org_id: String,
    pub org_name: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct SwitchOrgBody {
    pub org_id: String,
}

#[derive(Debug, Serialize)]
pub struct SwitchOrgJson {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub org_id: String,
}

/// `GET /v1/auth/me/orgs`
pub async fn list_user_orgs(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, GatewayError> {
    let token = extract_bearer(&headers)?;
    let req = tonic::Request::new(ListUserOrgsRequest {
        current_token: token,
    });
    let mut client = state.auth_client.lock().await;
    let resp = client
        .list_user_orgs(req)
        .await
        .map_err(GatewayError::from)?;
    let orgs: Vec<OrgEntryJson> = resp
        .into_inner()
        .orgs
        .into_iter()
        .map(|e| OrgEntryJson {
            org_id: e.org_id,
            org_name: e.org_name,
            role: e.role,
        })
        .collect();
    Ok(Json(orgs))
}

/// `POST /v1/auth/switch-org`
pub async fn switch_org(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(body): Json<SwitchOrgBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let token = extract_bearer(&headers)?;
    let req = tonic::Request::new(SwitchOrgRequest {
        current_token: token,
        target_org_id: body.org_id,
    });
    let mut client = state.auth_client.lock().await;
    let resp = client.switch_org(req).await.map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok(Json(SwitchOrgJson {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_in: r.expires_in,
        org_id: r.org_id,
    }))
}

#[derive(Debug, Serialize)]
pub struct PermissionsJson {
    pub roles: Vec<String>,
    pub permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct RefreshBody {
    pub refresh_token: String,
}

#[derive(Debug, Serialize)]
pub struct RefreshJson {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

/// `POST /v1/auth/refresh`
///
/// Exchange a valid refresh token for a fresh access + refresh token pair.
pub async fn refresh(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<RefreshBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(RefreshTokenRequest {
        refresh_token: body.refresh_token,
    });
    let mut client = state.auth_client.lock().await;
    let resp = client
        .refresh_token(req)
        .await
        .map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok(Json(RefreshJson {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_in: r.expires_in,
    }))
}

/// `GET /v1/auth/me/permissions`
///
/// Returns the roles and permissions embedded in the caller's validated JWT.
/// Requires `auth_middleware` to have injected an [`RbacContext`] extension.
pub async fn get_my_permissions(Extension(rbac): Extension<RbacContext>) -> impl IntoResponse {
    Json(PermissionsJson {
        roles: rbac.roles,
        permissions: rbac.permissions,
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/v1/auth/login", post(login))
        .route("/v1/auth/me/permissions", get(get_my_permissions))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::helpers::make_stub_state;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt as _;

    #[tokio::test]
    async fn get_my_permissions_with_rbac_context_returns_200() {
        use axum::body::to_bytes;
        use stitchd_proto::auth::v1::RbacContext;

        // Build a router that inserts RbacContext via an extension layer
        let state = make_stub_state();
        let app = axum::Router::new()
            .route(
                "/v1/auth/me/permissions",
                axum::routing::get(get_my_permissions),
            )
            .layer(axum::Extension(RbacContext {
                roles: vec!["org_admin".to_string()],
                permissions: vec!["project:create".to_string()],
                is_system: false,
                ..Default::default()
            }))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/auth/me/permissions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["roles"][0], "org_admin");
        assert_eq!(json["permissions"][0], "project:create");
    }

    #[tokio::test]
    async fn get_my_permissions_without_rbac_context_returns_500() {
        let state = make_stub_state();
        let app = axum::Router::new()
            .route(
                "/v1/auth/me/permissions",
                axum::routing::get(get_my_permissions),
            )
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/auth/me/permissions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Axum returns 500 when an Extension extractor finds nothing
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn login_returns_200_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"email":"admin@example.com","password":"secret"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }
}
