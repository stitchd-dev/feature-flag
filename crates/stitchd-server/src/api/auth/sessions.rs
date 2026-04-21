//! Session management handlers.
//!
//! # Endpoints
//! - `DELETE /auth/sessions` — sign out all sessions (rotate secret + revoke all tokens)
//! - `GET /auth/sessions` — list active sessions
//! - `DELETE /auth/sessions/{token_id}` — revoke a specific session

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use stitchd_core::id::RefreshTokenId;

use crate::AppState;
use crate::api::auth::middleware::AuthenticatedUser;
use crate::api::auth::password::AuthHandlerError;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// A single session entry returned by `GET /auth/sessions`.
#[derive(Debug, Serialize)]
pub struct SessionEntry {
    /// Opaque session (refresh token) identifier.
    pub id: uuid::Uuid,
    /// Optional human-readable device label.
    pub device_hint: Option<String>,
    /// When the session was issued.
    pub issued_at: chrono::DateTime<chrono::Utc>,
    /// When the session expires.
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// When the session was last used (if ever).
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ---------------------------------------------------------------------------
// DELETE /auth/sessions
// ---------------------------------------------------------------------------

/// `DELETE /auth/sessions` — rotate `token_secret` (invalidates all JWTs) and
/// revoke all active refresh tokens for the authenticated caller.
///
/// # Errors
///
/// Returns `401` if the caller is not authenticated and `500` on internal failures.
pub async fn revoke_all_sessions(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<StatusCode, AuthHandlerError> {
    // Rotate secret so any in-flight JWTs become invalid immediately.
    state
        .auth_user_repo
        .rotate_token_secret(user.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // Revoke all refresh tokens.
    state
        .refresh_token_repo
        .revoke_all_for_user(user.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// GET /auth/sessions
// ---------------------------------------------------------------------------

/// `GET /auth/sessions` — list active (non-revoked, non-expired) sessions for the caller.
///
/// # Errors
///
/// Returns `401` if the caller is not authenticated and `500` on internal failures.
pub async fn list_sessions(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<Response, AuthHandlerError> {
    let tokens = state
        .refresh_token_repo
        .list_active(user.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    let entries: Vec<SessionEntry> = tokens
        .into_iter()
        .map(|t| SessionEntry {
            id: t.id.as_uuid(),
            device_hint: t.device_hint,
            issued_at: t.issued_at,
            expires_at: t.expires_at,
            last_used_at: t.last_used_at,
        })
        .collect();

    Ok((StatusCode::OK, Json(entries)).into_response())
}

// ---------------------------------------------------------------------------
// DELETE /auth/sessions/{token_id}
// ---------------------------------------------------------------------------

/// `DELETE /auth/sessions/{token_id}` — revoke a specific session owned by the caller.
///
/// Returns 204 on success, 401 if the token doesn't exist or doesn't belong to the
/// caller, and 500 on internal failures.
///
/// # Errors
///
/// Returns `401` if the caller is not authenticated or the session is not found,
/// and `500` on internal failures.
pub async fn revoke_session(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(token_id): Path<uuid::Uuid>,
) -> Result<StatusCode, AuthHandlerError> {
    let refresh_token_id = RefreshTokenId::from_uuid(token_id);

    // Find the token among active sessions for this user.
    let active = state
        .refresh_token_repo
        .list_active(user.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    let token = active.iter().find(|t| t.id == refresh_token_id);

    match token {
        None => Err(AuthHandlerError::Unauthorized("session not found")),
        Some(t) => {
            state
                .refresh_token_repo
                .revoke(t.id)
                .await
                .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;
            Ok(StatusCode::NO_CONTENT)
        }
    }
}
