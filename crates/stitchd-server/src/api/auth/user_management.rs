//! Org/project/env user management handlers.
//!
//! # Endpoints
//! - `GET    /v1/orgs/{org_id}/users`                          — list users in org (`OrgAdmin`)
//! - `PUT    /v1/orgs/{org_id}/users/{user_id}`                — update user status (`OrgAdmin`)
//! - `DELETE /v1/orgs/{org_id}/users/{user_id}`                — remove from org (`OrgAdmin`)
//! - `PUT    /v1/orgs/{org_id}/users/{user_id}/role`           — update org role (`OrgAdmin`)
//! - `PUT    /v1/projects/{project_id}/members/{user_id}/role` — upsert project role (`ProjectAdmin`)
//! - `PUT    /v1/environments/{env_id}/members/{user_id}/role` — upsert env role (`EnvPublisher`)

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use stitchd_core::{
    auth::{EnvRole, OrgRole, ProjectRole, UserStatus},
    id::{OrganisationId, UserId},
};

use crate::AppState;
use crate::api::auth::middleware::{AuthenticatedUser, RequireEnvPublisher, RequireOrgAdmin, RequireProjectAdmin};
use crate::api::auth::password::AuthHandlerError;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// User summary returned in org user lists.
#[derive(Debug, Serialize)]
pub struct OrgUserEntry {
    /// User ID.
    pub id: uuid::Uuid,
    /// Email address.
    pub email: String,
    /// Display name.
    pub display_name: String,
    /// Account lifecycle status.
    pub status: UserStatus,
    /// Role in this organisation.
    pub org_role: OrgRole,
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// `PUT /v1/orgs/{org_id}/users/{user_id}` request body — update status.
#[derive(Debug, Deserialize)]
pub struct UpdateUserStatusRequest {
    /// New status.
    pub status: UserStatus,
}

/// `PUT /v1/orgs/{org_id}/users/{user_id}/role` request body.
#[derive(Debug, Deserialize)]
pub struct UpdateOrgRoleRequest {
    /// New org role.
    pub role: OrgRole,
}

/// `PUT /v1/projects/{project_id}/members/{user_id}/role` request body.
#[derive(Debug, Deserialize)]
pub struct UpdateProjectRoleRequest {
    /// New project role.
    pub role: ProjectRole,
}

/// `PUT /v1/environments/{env_id}/members/{user_id}/role` request body.
#[derive(Debug, Deserialize)]
pub struct UpdateEnvRoleRequest {
    /// New environment role.
    pub role: EnvRole,
}

// ---------------------------------------------------------------------------
// GET /v1/orgs/{org_id}/users
// ---------------------------------------------------------------------------

/// `GET /v1/orgs/{org_id}/users` — list all users in the organisation.
///
/// # Errors
/// Returns `403` if the caller is not an org admin, `500` on failures.
pub async fn list_org_users(
    State(state): State<AppState>,
    _: AuthenticatedUser,
    RequireOrgAdmin(_): RequireOrgAdmin,
    Path(org_id): Path<uuid::Uuid>,
) -> Result<Json<Vec<OrgUserEntry>>, AuthHandlerError> {
    let org_id = OrganisationId::from_uuid(org_id);

    let users = state
        .auth_user_repo
        .list_org_users(org_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok(Json(users.into_iter().map(|(u, role)| OrgUserEntry {
        id: u.id.as_uuid(),
        email: u.email,
        display_name: u.display_name,
        status: u.status,
        org_role: role,
    }).collect()))
}

// ---------------------------------------------------------------------------
// PUT /v1/orgs/{org_id}/users/{user_id}
// ---------------------------------------------------------------------------

/// `PUT /v1/orgs/{org_id}/users/{user_id}` — activate or deactivate a user account.
///
/// # Errors
/// Returns `403` if the caller is not an org admin, `404` if user not found, `500` on failures.
pub async fn update_user_status(
    State(state): State<AppState>,
    _: AuthenticatedUser,
    RequireOrgAdmin(_): RequireOrgAdmin,
    Path((_org_id, user_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Json(req): Json<UpdateUserStatusRequest>,
) -> Result<StatusCode, AuthHandlerError> {
    state
        .auth_user_repo
        .update_status(UserId::from_uuid(user_id), req.status)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// DELETE /v1/orgs/{org_id}/users/{user_id}
// ---------------------------------------------------------------------------

/// `DELETE /v1/orgs/{org_id}/users/{user_id}` — remove a user from the organisation.
///
/// Does not delete the platform user, only removes the membership.
///
/// # Errors
/// Returns `403` if the caller is not an org admin, `500` on failures.
pub async fn remove_org_user(
    State(state): State<AppState>,
    _: AuthenticatedUser,
    RequireOrgAdmin(_): RequireOrgAdmin,
    Path((org_id, user_id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, AuthHandlerError> {
    state
        .membership_repo
        .remove_member(UserId::from_uuid(user_id), OrganisationId::from_uuid(org_id))
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// PUT /v1/orgs/{org_id}/users/{user_id}/role
// ---------------------------------------------------------------------------

/// `PUT /v1/orgs/{org_id}/users/{user_id}/role` — change a user's org role.
///
/// # Errors
/// Returns `403` if the caller is not an org admin, `404` if membership not found, `500` on failures.
pub async fn update_org_user_role(
    State(state): State<AppState>,
    _: AuthenticatedUser,
    RequireOrgAdmin(_): RequireOrgAdmin,
    Path((org_id, user_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Json(req): Json<UpdateOrgRoleRequest>,
) -> Result<StatusCode, AuthHandlerError> {
    state
        .membership_repo
        .update_role(
            UserId::from_uuid(user_id),
            OrganisationId::from_uuid(org_id),
            req.role,
        )
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// PUT /v1/projects/{project_id}/members/{user_id}/role
// ---------------------------------------------------------------------------

/// `PUT /v1/projects/{project_id}/members/{user_id}/role` — upsert a project role.
///
/// # Errors
/// Returns `403` if the caller is not a project admin, `500` on failures.
pub async fn update_project_member_role(
    State(state): State<AppState>,
    _: AuthenticatedUser,
    RequireProjectAdmin(_): RequireProjectAdmin,
    Path((project_id, user_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Json(req): Json<UpdateProjectRoleRequest>,
) -> Result<StatusCode, AuthHandlerError> {
    let role_str = match req.role {
        ProjectRole::ProjectAdmin => "project_admin",
        ProjectRole::ProjectViewer => "project_viewer",
    };

    sqlx::query!(
        r#"
        INSERT INTO user_project_roles (user_id, project_id, role)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, project_id) DO UPDATE SET role = EXCLUDED.role
        "#,
        user_id,
        project_id,
        role_str,
    )
    .execute(&state.db)
    .await
    .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// PUT /v1/environments/{env_id}/members/{user_id}/role
// ---------------------------------------------------------------------------

/// `PUT /v1/environments/{env_id}/members/{user_id}/role` — upsert an env role.
///
/// # Errors
/// Returns `403` if the caller is not an env publisher, `500` on failures.
pub async fn update_env_member_role(
    State(state): State<AppState>,
    _: AuthenticatedUser,
    RequireEnvPublisher(_): RequireEnvPublisher,
    Path((env_id, user_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Json(req): Json<UpdateEnvRoleRequest>,
) -> Result<Response, AuthHandlerError> {
    let role_str = match req.role {
        EnvRole::EnvPublisher => "env_publisher",
        EnvRole::EnvViewer => "env_viewer",
    };

    sqlx::query!(
        r#"
        INSERT INTO user_env_roles (user_id, env_id, role)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, env_id) DO UPDATE SET role = EXCLUDED.role
        "#,
        user_id,
        env_id,
        role_str,
    )
    .execute(&state.db)
    .await
    .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT.into_response())
}
