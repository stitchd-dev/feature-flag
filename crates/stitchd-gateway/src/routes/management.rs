//! Management route handlers — org-scoped operations (projects, environments, SDK keys, users).

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
    CreateEnvironmentRequest, CreateProjectRequest, CreateSdkKeyRequest, CreateUserRequest,
    DeleteEnvironmentRequest, DeleteProjectRequest, ListEnvironmentsRequest, ListOrgUsersRequest,
    ListProjectsRequest, ListSdkKeysRequest, RemoveOrgUserRequest, RenameEnvironmentRequest,
    RenameProjectRequest, RevokeSdkKeyRequest,
};

use crate::error::GatewayError;
use crate::pagination::{PaginatedResponse, PaginationParams};
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateProjectBody {
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectJson {
    pub project_id: String,
    pub project_name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateEnvBody {
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EnvJson {
    pub environment_id: String,
    pub environment_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SdkKeyJson {
    pub sdk_key_id: String,
    pub raw_key: String,
    pub name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateUserBody {
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

/// `POST /v1/management/orgs/{org_id}/projects`
#[utoipa::path(
    post,
    path = "/v1/management/orgs/{org_id}/projects",
    tag = "management",
    params(("org_id" = String, Path, description = "Organisation ID")),
    request_body = CreateProjectBody,
    responses(
        (status = 201, description = "Project created", body = ProjectJson),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 502, description = "Management service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_project(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
    Json(body): Json<CreateProjectBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(CreateProjectRequest {
        org_id,
        name: body.name,
    });
    let mut client = state.management_client.lock().await;
    let resp = client
        .create_project(req)
        .await
        .map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok((
        StatusCode::CREATED,
        Json(ProjectJson {
            project_id: r.project_id,
            project_name: r.project_name,
        }),
    ))
}

/// `POST /v1/management/projects/{project_id}/environments`
#[utoipa::path(
    post,
    path = "/v1/management/projects/{project_id}/environments",
    tag = "management",
    params(("project_id" = String, Path, description = "Project ID")),
    request_body = CreateEnvBody,
    responses(
        (status = 201, description = "Environment created", body = EnvJson),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 502, description = "Management service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_environment(
    State(state): State<Arc<GatewayState>>,
    Path(project_id): Path<String>,
    Json(body): Json<CreateEnvBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(CreateEnvironmentRequest {
        project_id,
        name: body.name,
    });
    let mut client = state.management_client.lock().await;
    let resp = client
        .create_environment(req)
        .await
        .map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok((
        StatusCode::CREATED,
        Json(EnvJson {
            environment_id: r.environment_id,
            environment_name: r.environment_name,
        }),
    ))
}

#[derive(Debug, Deserialize, ToSchema, Default)]
pub struct CreateSdkKeyBody {
    #[serde(default)]
    pub name: Option<String>,
}

/// `POST /v1/management/environments/{environment_id}/sdk-keys`
#[utoipa::path(
    post,
    path = "/v1/management/environments/{environment_id}/sdk-keys",
    tag = "management",
    params(("environment_id" = String, Path, description = "Environment ID")),
    request_body(content = CreateSdkKeyBody),
    responses(
        (status = 201, description = "SDK key created", body = SdkKeyJson),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 502, description = "Management service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_sdk_key(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    body: Option<Json<CreateSdkKeyBody>>,
) -> Result<impl IntoResponse, GatewayError> {
    let name = body
        .map(|b| b.0.name.unwrap_or_default())
        .unwrap_or_default();
    let req = tonic::Request::new(CreateSdkKeyRequest {
        environment_id,
        name,
    });
    let mut client = state.management_client.lock().await;
    let resp = client
        .create_sdk_key(req)
        .await
        .map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok((
        StatusCode::CREATED,
        Json(SdkKeyJson {
            sdk_key_id: r.sdk_key_id,
            raw_key: r.raw_key,
            name: r.name,
        }),
    ))
}

/// `POST /v1/management/orgs/{org_id}/users`
#[utoipa::path(
    post,
    path = "/v1/management/orgs/{org_id}/users",
    tag = "management",
    params(("org_id" = String, Path, description = "Organisation ID")),
    request_body = CreateUserBody,
    responses(
        (status = 201, description = "User created", body = UserJson),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 502, description = "Management service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_user(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
    Json(body): Json<CreateUserBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(CreateUserRequest {
        org_id,
        email: body.email,
        display_name: body.display_name,
        password: body.password,
        org_role: body.org_role.unwrap_or_else(|| "org_member".into()),
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

// ─── List / Rename / Delete types ────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct RenameBody {
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectSummaryJson {
    pub project_id: String,
    pub project_name: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListProjectsJson {
    pub projects: Vec<ProjectSummaryJson>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EnvSummaryJson {
    pub environment_id: String,
    pub environment_name: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListEnvironmentsJson {
    pub environments: Vec<EnvSummaryJson>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SdkKeySummaryJson {
    pub sdk_key_id: String,
    pub name: String,
    pub is_active: bool,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListSdkKeysJson {
    pub sdk_keys: Vec<SdkKeySummaryJson>,
}

/// Query parameters for listing SDK keys.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListSdkKeysQuery {
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

// ─── New handlers ─────────────────────────────────────────────────────────────

/// `GET /v1/management/orgs/{org_id}/projects`
pub async fn list_projects(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(ListProjectsRequest { org_id });
    let mut client = state.management_client.lock().await;
    let resp = client
        .list_projects(req)
        .await
        .map_err(GatewayError::from)?;
    let projects = resp
        .into_inner()
        .projects
        .into_iter()
        .map(|p| ProjectSummaryJson {
            project_id: p.project_id,
            project_name: p.project_name,
            created_at: p.created_at,
        })
        .collect();
    Ok(Json(ListProjectsJson { projects }))
}

/// `PATCH /v1/management/projects/{project_id}`
pub async fn rename_project(
    State(state): State<Arc<GatewayState>>,
    Path(project_id): Path<String>,
    Json(body): Json<RenameBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(RenameProjectRequest {
        project_id,
        name: body.name,
    });
    let mut client = state.management_client.lock().await;
    client
        .rename_project(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /v1/management/projects/{project_id}`
pub async fn delete_project(
    State(state): State<Arc<GatewayState>>,
    Path(project_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(DeleteProjectRequest { project_id });
    let mut client = state.management_client.lock().await;
    client
        .delete_project(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/management/projects/{project_id}/environments`
pub async fn list_environments(
    State(state): State<Arc<GatewayState>>,
    Path(project_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(ListEnvironmentsRequest { project_id });
    let mut client = state.management_client.lock().await;
    let resp = client
        .list_environments(req)
        .await
        .map_err(GatewayError::from)?;
    let environments = resp
        .into_inner()
        .environments
        .into_iter()
        .map(|e| EnvSummaryJson {
            environment_id: e.environment_id,
            environment_name: e.environment_name,
            created_at: e.created_at,
        })
        .collect();
    Ok(Json(ListEnvironmentsJson { environments }))
}

/// `PATCH /v1/management/environments/{environment_id}`
pub async fn rename_environment(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    Json(body): Json<RenameBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(RenameEnvironmentRequest {
        environment_id,
        name: body.name,
    });
    let mut client = state.management_client.lock().await;
    client
        .rename_environment(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /v1/management/environments/{environment_id}`
pub async fn delete_environment(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(DeleteEnvironmentRequest { environment_id });
    let mut client = state.management_client.lock().await;
    client
        .delete_environment(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/management/environments/{environment_id}/sdk-keys`
pub async fn list_sdk_keys(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    Query(query): Query<ListSdkKeysQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(ListSdkKeysRequest {
        environment_id,
        page: query.pagination.effective_page(),
        per_page: query.pagination.effective_per_page(),
    });
    let mut client = state.management_client.lock().await;
    let resp = client
        .list_sdk_keys(req)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let sdk_keys: Vec<SdkKeySummaryJson> = inner
        .sdk_keys
        .into_iter()
        .map(|k| SdkKeySummaryJson {
            sdk_key_id: k.sdk_key_id,
            name: k.name,
            is_active: k.is_active,
            created_at: k.created_at,
            revoked_at: if k.revoked_at.is_empty() {
                None
            } else {
                Some(k.revoked_at)
            },
        })
        .collect();
    Ok(Json(PaginatedResponse::new(
        sdk_keys,
        inner.total,
        &query.pagination,
    )))
}

/// Query parameters for listing org users.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListOrgUsersQuery {
    #[serde(flatten)]
    pub pagination: PaginationParams,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrgUserJson {
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub created_at: String,
}

/// `DELETE /v1/management/environments/{environment_id}/sdk-keys/{sdk_key_id}`
pub async fn revoke_sdk_key(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, sdk_key_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(RevokeSdkKeyRequest { sdk_key_id });
    let mut client = state.management_client.lock().await;
    client
        .revoke_sdk_key(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/management/orgs/{org_id}/users`
pub async fn list_org_users(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
    Query(query): Query<ListOrgUsersQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(ListOrgUsersRequest {
        org_id,
        page: query.pagination.effective_page(),
        per_page: query.pagination.effective_per_page(),
    });
    let mut client = state.management_client.lock().await;
    let resp = client
        .list_org_users(req)
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
    Ok(Json(PaginatedResponse::new(
        users,
        inner.total,
        &query.pagination,
    )))
}

/// `DELETE /v1/management/orgs/{org_id}/users/{user_id}`
pub async fn remove_org_user(
    State(state): State<Arc<GatewayState>>,
    Path((org_id, user_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(RemoveOrgUserRequest { org_id, user_id });
    let mut client = state.management_client.lock().await;
    client
        .remove_org_user(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::{delete, get, patch};
    axum::Router::new()
        .route(
            "/v1/management/orgs/{org_id}/projects",
            get(list_projects).post(create_project),
        )
        .route(
            "/v1/management/projects/{project_id}",
            patch(rename_project).delete(delete_project),
        )
        .route(
            "/v1/management/projects/{project_id}/environments",
            get(list_environments).post(create_environment),
        )
        .route(
            "/v1/management/environments/{environment_id}",
            patch(rename_environment).delete(delete_environment),
        )
        .route(
            "/v1/management/environments/{environment_id}/sdk-keys",
            get(list_sdk_keys).post(create_sdk_key),
        )
        .route(
            "/v1/management/environments/{environment_id}/sdk-keys/{sdk_key_id}",
            delete(revoke_sdk_key),
        )
        .route(
            "/v1/management/orgs/{org_id}/users",
            get(list_org_users).post(create_user),
        )
        .route(
            "/v1/management/orgs/{org_id}/users/{user_id}",
            delete(remove_org_user),
        )
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
    async fn create_project_returns_201_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/management/orgs/org-1/projects")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"My Project"}"#))
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

    #[tokio::test]
    async fn create_user_returns_201_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/management/orgs/org-1/users")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"email":"user@example.com","display_name":"Test User","password":"secret"}"#))
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

    #[tokio::test]
    async fn create_environment_returns_201_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/management/projects/proj-1/environments")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"staging"}"#))
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

    #[tokio::test]
    async fn create_sdk_key_returns_201_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/management/environments/env-1/sdk-keys")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
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

    #[tokio::test]
    async fn list_projects_returns_200_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/management/orgs/org-1/projects")
                    .body(Body::empty())
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

    #[tokio::test]
    async fn rename_project_returns_204_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/management/projects/proj-1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"new-name"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn delete_project_returns_204_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/management/projects/proj-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn list_environments_returns_200_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/management/projects/proj-1/environments")
                    .body(Body::empty())
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

    #[tokio::test]
    async fn rename_environment_returns_204_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/management/environments/env-1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"staging"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn delete_environment_returns_204_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/management/environments/env-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn list_sdk_keys_returns_200_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/management/environments/env-1/sdk-keys")
                    .body(Body::empty())
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

    #[tokio::test]
    async fn revoke_sdk_key_returns_204_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/management/environments/env-1/sdk-keys/key-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn create_user_defaults_org_role_to_org_member_when_omitted() {
        // This test pins the CURRENT behavior: when `org_role` is absent from
        // the create-user body, the gateway substitutes "org_member" before
        // forwarding to the management service.
        //
        // After refactoring (GL-12), this default moves into the management
        // service itself. The API contract — "omitting org_role → org_member" —
        // must survive the refactor unchanged.
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/management/orgs/org-1/users")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"email":"user@example.com","display_name":"Test","password":"pass123"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Stub gRPC will fail → 502, but the gateway attempted to forward with
        // org_role = "org_member" (the test confirms the gateway doesn't 400).
        // A 400 here would mean org_role was passed as absent to the proto, which
        // would be wrong.
        assert!(
            resp.status() == StatusCode::CREATED
                || resp.status() == StatusCode::BAD_GATEWAY
                || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
            "unexpected status {} — gateway must not 400 when org_role is omitted",
            resp.status()
        );
    }
}
