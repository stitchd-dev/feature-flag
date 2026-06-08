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
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::management::v1::{
    CreateOrgRequest, CreateUserRequest, GetOrgRequest, ListOrgUsersRequest, ListOrgsRequest,
    RemoveOrgUserRequest,
};

use crate::error::GatewayError;
use crate::pagination::{CursorPage, CursorParams};
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

/// Query parameters for listing org users.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListOrgUsersQuery {
    #[serde(flatten)]
    pub cursor: CursorParams,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateOrgBody {
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrgJson {
    pub org_id: String,
    pub org_name: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListOrgsJson {
    pub orgs: Vec<OrgJson>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SeedUserBody {
    pub email: String,
    /// Optional for existing users — omit or send null/empty when adding a
    /// platform user who already has a display name.
    pub display_name: Option<String>,
    /// Optional for existing users — omit or send null/empty. The backend
    /// skips user creation and just adds the org membership.
    pub password: Option<String>,
    pub org_role: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UserJson {
    pub user_id: String,
    pub email: String,
    pub display_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrgUserJson {
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListOrgUsersJson {
    pub users: Vec<OrgUserJson>,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `POST /v1/superadmin/orgs`
#[utoipa::path(
    post,
    path = "/v1/superadmin/orgs",
    tag = "superadmin",
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
            created_at: Some(r.created_at),
        }),
    ))
}

/// `POST /v1/superadmin/orgs/{org_id}/users` — seed the first user into a newly created org.
#[utoipa::path(
    post,
    path = "/v1/superadmin/orgs/{org_id}/users",
    tag = "superadmin",
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
        display_name: body.display_name.unwrap_or_default(),
        password: body.password.unwrap_or_default(),
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

/// `GET /v1/superadmin/orgs/{org_id}`
pub async fn get_org(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let mut client = state.management_client.lock().await;
    let resp = client
        .get_org(tonic::Request::new(GetOrgRequest { org_id }))
        .await
        .map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok(Json(OrgJson {
        org_id: r.org_id,
        org_name: r.org_name,
        created_at: Some(r.created_at),
    }))
}

/// `DELETE /v1/superadmin/orgs/{org_id}/users/{user_id}`
pub async fn remove_org_user(
    State(state): State<Arc<GatewayState>>,
    Path((org_id, user_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let mut client = state.management_client.lock().await;
    client
        .remove_org_user(tonic::Request::new(RemoveOrgUserRequest {
            org_id,
            user_id,
        }))
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/superadmin/orgs/{org_id}/users`
pub async fn list_org_users(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
    Query(query): Query<ListOrgUsersQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let mut client = state.management_client.lock().await;
    let resp = client
        .list_org_users(tonic::Request::new(ListOrgUsersRequest {
            org_id,
            cursor: query.cursor.cursor.clone().unwrap_or_default(),
            limit: query.cursor.effective_limit(),
        }))
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let users: Vec<OrgUserJson> = inner
        .users
        .into_iter()
        .map(|u| OrgUserJson {
            user_id: u.user_id,
            email: u.email,
            display_name: u.display_name,
            role: u.role,
            created_at: u.created_at,
        })
        .collect();
    Ok(Json(CursorPage::from_token(users, inner.next_cursor)))
}

/// `GET /v1/superadmin/orgs`
pub async fn list_orgs(
    State(state): State<Arc<GatewayState>>,
) -> Result<impl IntoResponse, GatewayError> {
    let mut client = state.management_client.lock().await;
    let resp = client
        .list_orgs(tonic::Request::new(ListOrgsRequest {}))
        .await
        .map_err(GatewayError::from)?;
    let orgs = resp
        .into_inner()
        .orgs
        .into_iter()
        .map(|o| OrgJson {
            org_id: o.org_id,
            org_name: o.org_name,
            created_at: Some(o.created_at),
        })
        .collect();
    Ok(Json(ListOrgsJson { orgs }))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/v1/superadmin/orgs", get(list_orgs).post(create_org))
        .route("/v1/superadmin/orgs/{org_id}/users", post(seed_user))
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
                    .uri("/v1/superadmin/orgs")
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
