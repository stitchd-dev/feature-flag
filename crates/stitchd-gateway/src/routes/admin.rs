//! Admin route handlers — superadmin-only operations (org lifecycle).
//!
//! These routes are protected by two middleware layers:
//!   1. `auth_middleware` — validates the JWT
//!   2. `require_system_org` — rejects callers who are NOT System-org users
//!
//! Only the platform superadmin (a member of the System organisation) may call
//! these endpoints.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::management::v1::{CreateOrgRequest, CreateUserRequest};

use crate::error::GatewayError;
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateOrgBody {
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrgJson {
    pub org_id: String,
    pub org_name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SeedUserBody {
    pub email: String,
    pub display_name: String,
    pub password: String,
    pub org_role: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserJson {
    pub user_id: String,
    pub email: String,
    pub display_name: String,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `POST /v1/admin/orgs`
#[utoipa::path(
    post,
    path = "/v1/admin/orgs",
    tag = "admin",
    request_body = CreateOrgBody,
    responses(
        (status = 201, description = "Organisation created", body = OrgJson),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — caller is not a system-org member"),
        (status = 502, description = "Management service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_org(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<CreateOrgBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(CreateOrgRequest { name: body.name });
    let mut client = state.management_client.lock().await;
    let resp = client.create_org(req).await.map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok((
        StatusCode::CREATED,
        Json(OrgJson {
            org_id: r.org_id,
            org_name: r.org_name,
        }),
    ))
}

/// `POST /v1/admin/orgs/{org_id}/users` — seed the first user into a newly created org.
#[utoipa::path(
    post,
    path = "/v1/admin/orgs/{org_id}/users",
    tag = "admin",
    params(("org_id" = String, Path, description = "Organisation ID")),
    request_body = SeedUserBody,
    responses(
        (status = 201, description = "User seeded", body = UserJson),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — caller is not a system-org member"),
        (status = 502, description = "Management service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn seed_user(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
    Json(body): Json<SeedUserBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(CreateUserRequest {
        org_id,
        email: body.email,
        display_name: body.display_name,
        password: body.password,
        org_role: body.org_role.unwrap_or_else(|| "org_admin".into()),
    });
    let mut client = state.management_client.lock().await;
    let resp = client.create_user(req).await.map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok((
        StatusCode::CREATED,
        Json(UserJson {
            user_id: r.user_id,
            email: r.email,
            display_name: r.display_name,
        }),
    ))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::post;
    axum::Router::new()
        .route("/v1/admin/orgs", post(create_org))
        .route("/v1/admin/orgs/{org_id}/users", post(seed_user))
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
    async fn create_org_returns_201_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/orgs")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Acme Corp"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::CREATED || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }
}
