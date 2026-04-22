//! Auth route handlers — login.

use axum::{Json, extract::State, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::auth::v1::LoginRequest;

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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::post;
    axum::Router::new()
        .route("/v1/auth/login", post(login))
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
