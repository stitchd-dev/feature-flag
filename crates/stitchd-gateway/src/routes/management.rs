//! Management route handlers — org-scoped operations (projects, environments, SDK keys, users).

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::management::v1::{
    CreateEnvironmentRequest, CreateProjectRequest, CreateSdkKeyRequest, CreateUserRequest,
};

use crate::error::GatewayError;
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
    tag = "admin",
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
    tag = "admin",
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

/// `POST /v1/management/environments/{environment_id}/sdk-keys`
#[utoipa::path(
    post,
    path = "/v1/management/environments/{environment_id}/sdk-keys",
    tag = "admin",
    params(("environment_id" = String, Path, description = "Environment ID")),
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
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(CreateSdkKeyRequest { environment_id });
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
        }),
    ))
}

/// `POST /v1/management/orgs/{org_id}/users`
#[utoipa::path(
    post,
    path = "/v1/management/orgs/{org_id}/users",
    tag = "admin",
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::post;
    axum::Router::new()
        .route(
            "/v1/management/orgs/{org_id}/projects",
            post(create_project),
        )
        .route(
            "/v1/management/projects/{project_id}/environments",
            post(create_environment),
        )
        .route(
            "/v1/management/environments/{environment_id}/sdk-keys",
            post(create_sdk_key),
        )
        .route("/v1/management/orgs/{org_id}/users", post(create_user))
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
}
