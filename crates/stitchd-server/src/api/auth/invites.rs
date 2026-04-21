//! Invite flow handlers.
//!
//! # Endpoints
//! - `POST /v1/orgs/{org_id}/invites`            — create an invite (`OrgAdmin`)
//! - `GET  /v1/orgs/{org_id}/invites`            — list pending invites (`OrgAdmin`)
//! - `DELETE /v1/orgs/{org_id}/invites/{invite_id}` — revoke an invite (`OrgAdmin`)
//! - `POST /auth/invites/{token}/accept`         — accept an invite (public)

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stitchd_core::{
    auth::{OrgRole, jwt::JwtEngine},
    id::{InviteId, OrganisationId},
};

use crate::AppState;
use crate::api::auth::middleware::{AuthenticatedUser, RequireOrgRole};
use crate::api::auth::password::{AuthHandlerError, TokenResponse};

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// `POST /v1/orgs/{org_id}/invites` request body.
#[derive(Debug, Deserialize)]
pub struct CreateInviteRequest {
    /// Email of the invitee.
    pub email: String,
    /// Role to assign upon acceptance.
    pub org_role: OrgRole,
}

/// `POST /v1/orgs/{org_id}/invites` response.
#[derive(Debug, Serialize)]
pub struct CreateInviteResponse {
    /// The newly-created invite's ID.
    pub invite_id: uuid::Uuid,
    /// Present when SMTP is not configured — the raw accept URL to share manually.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline_link: Option<String>,
}

/// `POST /auth/invites/{token}/accept` request body.
#[derive(Debug, Deserialize)]
pub struct AcceptInviteRequest {
    /// Display name for new user registration.
    pub display_name: String,
    /// Password for new user registration (ignored for existing users).
    pub password: Option<String>,
}

/// `GET /v1/orgs/{org_id}/invites` response entry.
#[derive(Debug, Serialize)]
pub struct InviteEntry {
    /// Invite ID.
    pub id: uuid::Uuid,
    /// Invitee email.
    pub email: String,
    /// Assigned role.
    pub org_role: OrgRole,
    /// When the invite expires.
    pub expires_at: chrono::DateTime<Utc>,
    /// When the invite was sent.
    pub created_at: chrono::DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// POST /v1/orgs/{org_id}/invites
// ---------------------------------------------------------------------------

/// `POST /v1/orgs/{org_id}/invites` — create an invite for `email` to join `org_id`.
///
/// # Errors
/// Returns `403` if the caller is not an org admin, `500` on internal failures.
pub async fn create_invite(
    State(state): State<AppState>,
    _: AuthenticatedUser,
    RequireOrgRole(_): RequireOrgRole,
    Path(org_id): Path<uuid::Uuid>,
    Json(req): Json<CreateInviteRequest>,
) -> Result<Response, AuthHandlerError> {
    let org_id = OrganisationId::from_uuid(org_id);

    let (invite, raw_token) = state
        .invite_repo
        .create(org_id, &req.email, req.org_role, None, 72)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    let accept_url = format!("/auth/invites/{raw_token}/accept");

    let offline_link = state
        .email_service
        .send(
            &req.email,
            "You're invited",
            &format!(
                "You have been invited to join the organisation. Accept your invite: {accept_url}"
            ),
            &accept_url,
        )
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .map(|link| link.url);

    Ok((
        StatusCode::CREATED,
        Json(CreateInviteResponse {
            invite_id: invite.id.as_uuid(),
            offline_link,
        }),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// GET /v1/orgs/{org_id}/invites
// ---------------------------------------------------------------------------

/// `GET /v1/orgs/{org_id}/invites` — list pending (not-yet-accepted) invites.
///
/// # Errors
/// Returns `403` if the caller is not an org admin, `500` on internal failures.
pub async fn list_invites(
    State(state): State<AppState>,
    _: AuthenticatedUser,
    RequireOrgRole(_): RequireOrgRole,
    Path(org_id): Path<uuid::Uuid>,
) -> Result<Json<Vec<InviteEntry>>, AuthHandlerError> {
    let org_id = OrganisationId::from_uuid(org_id);

    let all = state
        .invite_repo
        .list_for_org(org_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    let pending: Vec<InviteEntry> = all
        .into_iter()
        .filter(|i| i.accepted_at.is_none())
        .map(|i| InviteEntry {
            id: i.id.as_uuid(),
            email: i.email,
            org_role: i.org_role,
            expires_at: i.expires_at,
            created_at: i.created_at,
        })
        .collect();

    Ok(Json(pending))
}

// ---------------------------------------------------------------------------
// DELETE /v1/orgs/{org_id}/invites/{invite_id}
// ---------------------------------------------------------------------------

/// `DELETE /v1/orgs/{org_id}/invites/{invite_id}` — revoke an invite.
///
/// # Errors
/// Returns `403` if the caller is not an org admin, `404` if not found,
/// `500` on internal failures.
pub async fn revoke_invite(
    State(state): State<AppState>,
    _: AuthenticatedUser,
    RequireOrgRole(_): RequireOrgRole,
    Path((_org_id, invite_id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, AuthHandlerError> {
    state
        .invite_repo
        .revoke(InviteId::from_uuid(invite_id))
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /auth/invites/{token}/accept
// ---------------------------------------------------------------------------

/// `POST /auth/invites/{token}/accept` — accept an invite and register or add the user.
///
/// # Errors
/// Returns `404` if the token is not found, `410 Gone` if expired or already used,
/// `500` on internal failures.
pub async fn accept_invite(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(req): Json<AcceptInviteRequest>,
) -> Result<Response, AcceptInviteError> {
    // 1. Hash the token to look it up.
    let token_hash = sha256_of_hex_token(&token);

    let invite = state
        .invite_repo
        .find_by_token_hash(&token_hash)
        .await
        .map_err(|e| AcceptInviteError::Internal(e.to_string()))?
        .ok_or(AcceptInviteError::NotFound)?;

    // 2. Check expiry and used status.
    if invite.expires_at <= Utc::now() || invite.accepted_at.is_some() {
        return Err(AcceptInviteError::Gone);
    }

    // 3. Find or create the user.
    let existing = state
        .auth_user_repo
        .find_by_email(&invite.email)
        .await
        .map_err(|e| AcceptInviteError::Internal(e.to_string()))?;

    let user = if let Some(u) = existing {
        // Existing user — just add membership.
        u
    } else {
        // New user — create account.
        let password_hash = if let Some(ref pw) = req.password {
            Some(
                stitchd_core::auth::crypto::hash_password(pw)
                    .map_err(|e| AcceptInviteError::Internal(e.to_string()))?,
            )
        } else {
            None
        };
        state
            .auth_user_repo
            .create(
                &invite.email,
                &req.display_name,
                password_hash.as_deref(),
            )
            .await
            .map_err(|e| AcceptInviteError::Internal(e.to_string()))?
    };

    // 4. Add membership.
    state
        .membership_repo
        .add_member(user.id, invite.org_id, invite.org_role)
        .await
        .map_err(|e| AcceptInviteError::Internal(e.to_string()))?;

    // 5. Mark invite as accepted.
    state
        .invite_repo
        .accept(invite.id)
        .await
        .map_err(|e| AcceptInviteError::Internal(e.to_string()))?;

    // 6. Issue JWT + refresh token.
    let access_token = JwtEngine::issue(user.id, invite.org_id, &user.email, invite.org_role, &user.token_secret)
        .map_err(|e| AcceptInviteError::Internal(e.to_string()))?;

    let (_refresh, raw_refresh) = state
        .refresh_token_repo
        .create(user.id, invite.org_id, None, 30)
        .await
        .map_err(|e| AcceptInviteError::Internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(TokenResponse {
            access_token,
            refresh_token: raw_refresh,
            token_type: "Bearer",
            expires_in: 900,
        }),
    )
        .into_response())
}

/// Compute SHA-256 of the raw opaque token.
///
/// The raw token is a 64-char hex string (encoding 32 bytes).
/// The hash stored in DB is SHA-256 of those 32 bytes (not the hex string).
fn sha256_of_hex_token(raw_hex: &str) -> String {
    hex::decode(raw_hex).map_or_else(
        |_| hex::encode(Sha256::digest(raw_hex.as_bytes())),
        |bytes| hex::encode(Sha256::digest(&bytes)),
    )
}

// ---------------------------------------------------------------------------
// Error type for accept_invite
// ---------------------------------------------------------------------------

/// Errors returned by [`accept_invite`].
#[derive(Debug)]
pub enum AcceptInviteError {
    /// 404 — token not found.
    NotFound,
    /// 410 — invite expired or already accepted.
    Gone,
    /// 500 — internal failure.
    Internal(String),
}

impl IntoResponse for AcceptInviteError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            Self::NotFound => (StatusCode::NOT_FOUND, "invite not found"),
            Self::Gone => (StatusCode::GONE, "invite expired or already used"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

