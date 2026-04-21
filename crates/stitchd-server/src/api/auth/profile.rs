//! User profile handlers.
//!
//! # Endpoints
//! - `GET  /v1/users/me`          — return own profile + memberships + MFA status
//! - `PUT  /v1/users/me`          — update `display_name` / `avatar_url`
//! - `PUT  /v1/users/me/password` — change own password

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::api::auth::middleware::AuthenticatedUser;
use crate::api::auth::password::AuthHandlerError;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Org membership entry in the profile response.
#[derive(Debug, Serialize)]
pub struct OrgEntry {
    /// Organisation ID.
    pub org_id: uuid::Uuid,
    /// User's role in that organisation.
    pub role: stitchd_core::auth::OrgRole,
}

/// `GET /v1/users/me` response.
#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    /// User ID.
    pub id: uuid::Uuid,
    /// Email address.
    pub email: String,
    /// Display name.
    pub display_name: String,
    /// Avatar URL.
    pub avatar_url: Option<String>,
    /// Whether TOTP MFA is enabled.
    pub totp_enabled: bool,
    /// Organisation memberships.
    pub orgs: Vec<OrgEntry>,
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// `PUT /v1/users/me` request body.
#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    /// New display name.
    pub display_name: Option<String>,
    /// New avatar URL.
    pub avatar_url: Option<String>,
}

/// `PUT /v1/users/me/password` request body.
#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    /// The user's current password (for verification).
    pub current_password: String,
    /// The desired new password.
    pub new_password: String,
}

// ---------------------------------------------------------------------------
// GET /v1/users/me
// ---------------------------------------------------------------------------

/// `GET /v1/users/me` — return the authenticated user's profile.
///
/// # Errors
/// Returns `401` if unauthenticated, `404` if the user record is missing, `500` on failures.
pub async fn get_me(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<Json<ProfileResponse>, AuthHandlerError> {
    let db_user = state
        .auth_user_repo
        .find_by_id(user.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or_else(|| AuthHandlerError::Internal("user not found".to_string()))?;

    let memberships = state
        .membership_repo
        .list_orgs_for_user(user.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    let orgs = memberships
        .into_iter()
        .map(|m| OrgEntry {
            org_id: m.org_id.as_uuid(),
            role: m.role,
        })
        .collect();

    Ok(Json(ProfileResponse {
        id: db_user.id.as_uuid(),
        email: db_user.email,
        display_name: db_user.display_name,
        avatar_url: db_user.avatar_url,
        totp_enabled: db_user.totp_enabled,
        orgs,
    }))
}

// ---------------------------------------------------------------------------
// PUT /v1/users/me
// ---------------------------------------------------------------------------

/// `PUT /v1/users/me` — update the authenticated user's display name and/or avatar.
///
/// # Errors
/// Returns `401` if unauthenticated, `500` on failures.
pub async fn update_me(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(req): Json<UpdateProfileRequest>,
) -> Result<StatusCode, AuthHandlerError> {
    // Fetch current values so we can keep unspecified fields unchanged.
    let db_user = state
        .auth_user_repo
        .find_by_id(user.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or_else(|| AuthHandlerError::Internal("user not found".to_string()))?;

    let new_display_name = req
        .display_name
        .as_deref()
        .unwrap_or(&db_user.display_name);
    let new_avatar_url = req.avatar_url.as_deref().or(db_user.avatar_url.as_deref());

    state
        .auth_user_repo
        .update_profile(user.user_id, new_display_name, new_avatar_url)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// PUT /v1/users/me/password
// ---------------------------------------------------------------------------

/// `PUT /v1/users/me/password` — change the authenticated user's password.
///
/// # Errors
/// Returns `400` if `current_password` is wrong, `401` if unauthenticated, `500` on failures.
pub async fn change_my_password(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Response, AuthHandlerError> {
    let db_user = state
        .auth_user_repo
        .find_by_id(user.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or_else(|| AuthHandlerError::Internal("user not found".to_string()))?;

    let current_hash = db_user
        .password_hash
        .as_deref()
        .ok_or(AuthHandlerError::BadRequest("no password set"))?;

    let ok = stitchd_core::auth::crypto::verify_password(&req.current_password, current_hash)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    if !ok {
        return Err(AuthHandlerError::BadRequest("current password is incorrect"));
    }

    let new_hash = stitchd_core::auth::crypto::hash_password(&req.new_password)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    state
        .auth_user_repo
        .update_password_hash(user.user_id, &new_hash)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "message": "Password updated." })),
    )
        .into_response())
}
