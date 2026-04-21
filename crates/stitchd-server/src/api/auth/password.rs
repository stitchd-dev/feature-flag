//! Password-based authentication handlers.
//!
//! # Endpoints
//! - `POST /auth/login` — verify credentials, issue JWT + refresh token (or MFA challenge)
//! - `POST /auth/refresh` — rotate a refresh token, issue new pair
//! - `POST /auth/logout` — revoke a specific refresh token
//! - `POST /auth/switch-org` — issue a new token pair for a different organisation

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stitchd_core::{
    auth::{UserStatus, jwt::JwtEngine},
    id::OrganisationId,
};

use crate::AppState;
use crate::api::auth::middleware::AuthenticatedUser;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// `POST /auth/login` request body.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    /// User email address.
    pub email: String,
    /// Plain-text password.
    pub password: String,
    /// Optional device hint for session labelling.
    pub device_hint: Option<String>,
    /// Optional `org_id` — required when the user is a member of multiple orgs.
    pub org_id: Option<uuid::Uuid>,
}

/// `POST /auth/login` success response (no MFA).
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    /// Short-lived JWT access token.
    pub access_token: String,
    /// Opaque refresh token (raw hex).
    pub refresh_token: String,
    /// Always `"Bearer"`.
    pub token_type: &'static str,
    /// Access token lifetime in seconds (900 = 15 minutes).
    pub expires_in: u64,
}

/// `POST /auth/login` response when MFA is required.
#[derive(Debug, Serialize)]
pub struct MfaRequiredResponse {
    /// Always `true` — indicates client must complete MFA.
    pub mfa_required: bool,
    /// Short-lived challenge token — pass to `POST /auth/mfa/verify`.
    pub challenge_token: String,
}

/// `POST /auth/refresh` request body.
#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    /// The raw refresh token previously issued.
    pub refresh_token: String,
}

/// `POST /auth/logout` request body.
#[derive(Debug, Deserialize)]
pub struct LogoutRequest {
    /// The raw refresh token to revoke.
    pub refresh_token: String,
}

/// `POST /auth/switch-org` request body.
#[derive(Debug, Deserialize)]
pub struct SwitchOrgRequest {
    /// Target organisation ID.
    pub org_id: uuid::Uuid,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by auth handlers.
#[derive(Debug)]
pub enum AuthHandlerError {
    /// 401 — credentials invalid or token expired.
    Unauthorized(&'static str),
    /// 403 — account deactivated or insufficient membership.
    Forbidden(&'static str),
    /// 400 — request body is semantically invalid.
    BadRequest(&'static str),
    /// 500 — unexpected internal failure.
    Internal(String),
}

impl IntoResponse for AuthHandlerError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            Self::Unauthorized(m) => (StatusCode::UNAUTHORIZED, *m),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, *m),
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, *m),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

// ---------------------------------------------------------------------------
// Helper: SHA-256 hex of a raw token string
// ---------------------------------------------------------------------------

fn sha256_hex(raw: &str) -> String {
    // The raw token is a lowercase hex string produced by `generate_opaque_token`,
    // which stores SHA-256 of the *underlying bytes* (not the hex string).
    // Decode back to bytes so the hash matches what the DB has.
    hex::decode(raw).map_or_else(
        |_| {
            // Fallback for tokens that aren't valid hex (will never match).
            hex::encode(Sha256::digest(raw.as_bytes()))
        },
        |bytes| hex::encode(Sha256::digest(&bytes)),
    )
}

// ---------------------------------------------------------------------------
// POST /auth/login
// ---------------------------------------------------------------------------

/// `POST /auth/login` — verify Argon2id password, resolve org context, issue tokens.
///
/// # Errors
///
/// Returns `401` for invalid credentials, `403` for deactivated accounts or missing
/// org membership, `400` when `org_id` is required but absent, and `500` on internal
/// failures.
pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, AuthHandlerError> {
    // 1. Find user by email.
    let user = state
        .auth_user_repo
        .find_by_email(&req.email)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::Unauthorized("invalid credentials"))?;

    // 2. Verify password.
    let hash = user
        .password_hash
        .as_deref()
        .ok_or(AuthHandlerError::Unauthorized("invalid credentials"))?;

    let ok = stitchd_core::auth::crypto::verify_password(&req.password, hash)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    if !ok {
        return Err(AuthHandlerError::Unauthorized("invalid credentials"));
    }

    // 3. Check account status.
    if user.status == UserStatus::Deactivated {
        return Err(AuthHandlerError::Forbidden("account deactivated"));
    }

    // 4. If TOTP enabled, create a challenge (full flow is Phase 4).
    if user.totp_enabled {
        let (raw, _hash) = stitchd_core::auth::crypto::generate_opaque_token();
        return Ok((
            StatusCode::OK,
            Json(MfaRequiredResponse {
                mfa_required: true,
                challenge_token: raw,
            }),
        )
            .into_response());
    }

    // 5. Resolve org context.
    let memberships = state
        .membership_repo
        .list_orgs_for_user(user.id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    let org_id = match memberships.len() {
        0 => return Err(AuthHandlerError::Forbidden("no organisation membership")),
        1 => memberships[0].org_id,
        _ => {
            let requested = req
                .org_id
                .ok_or(AuthHandlerError::BadRequest("org_id required for multi-org accounts"))?;
            let org_id = OrganisationId::from_uuid(requested);
            if !memberships.iter().any(|m| m.org_id == org_id) {
                return Err(AuthHandlerError::Forbidden("not a member of that organisation"));
            }
            org_id
        }
    };

    let membership = memberships
        .iter()
        .find(|m| m.org_id == org_id)
        .ok_or_else(|| AuthHandlerError::Internal("membership vanished after verification".into()))?;

    // 6. Issue JWT.
    let access_token = JwtEngine::issue(user.id, org_id, &user.email, membership.role, &user.token_secret)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // 7. Create refresh token.
    let (_token, raw_refresh) = state
        .refresh_token_repo
        .create(user.id, org_id, req.device_hint.as_deref(), 30)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

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

// ---------------------------------------------------------------------------
// POST /auth/refresh
// ---------------------------------------------------------------------------

/// `POST /auth/refresh` — rotate a refresh token and issue a new JWT + token pair.
///
/// # Errors
///
/// Returns `401` if the refresh token is invalid or expired, `403` if the account
/// is deactivated or no longer a member of the token's org, and `500` on internal
/// failures.
pub async fn refresh(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> Result<Response, AuthHandlerError> {
    // 1. Hash the raw refresh token, look it up.
    let token_hash = sha256_hex(&req.refresh_token);

    let existing = state
        .refresh_token_repo
        .find_by_hash(&token_hash)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::Unauthorized("invalid or expired refresh token"))?;

    // 2. Consume (revoke) the old token.
    state
        .refresh_token_repo
        .consume(existing.id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // 3. Load user to get current token_secret.
    let user = state
        .auth_user_repo
        .find_by_id(existing.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::Unauthorized("user not found"))?;

    if user.status == UserStatus::Deactivated {
        return Err(AuthHandlerError::Forbidden("account deactivated"));
    }

    // 4. Resolve org membership for this token's org.
    let membership = state
        .membership_repo
        .find_membership(user.id, existing.org_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::Forbidden("no membership in token's organisation"))?;

    // 5. Issue new JWT.
    let access_token = JwtEngine::issue(
        user.id,
        existing.org_id,
        &user.email,
        membership.role,
        &user.token_secret,
    )
    .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // 6. Create new refresh token.
    let (_new_token, new_raw) = state
        .refresh_token_repo
        .create(user.id, existing.org_id, existing.device_hint.as_deref(), 30)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(TokenResponse {
            access_token,
            refresh_token: new_raw,
            token_type: "Bearer",
            expires_in: 900,
        }),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// POST /auth/logout
// ---------------------------------------------------------------------------

/// `POST /auth/logout` — revoke the supplied refresh token. Requires authentication.
///
/// # Errors
///
/// Returns `401` if the caller is not authenticated and `500` on internal failures.
pub async fn logout(
    State(state): State<AppState>,
    _user: AuthenticatedUser,
    Json(req): Json<LogoutRequest>,
) -> Result<StatusCode, AuthHandlerError> {
    let token_hash = sha256_hex(&req.refresh_token);

    let existing = state
        .refresh_token_repo
        .find_by_hash(&token_hash)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    if let Some(token) = existing {
        state
            .refresh_token_repo
            .revoke(token.id)
            .await
            .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /auth/switch-org
// ---------------------------------------------------------------------------

/// `POST /auth/switch-org` — validate membership in target org, issue new token pair.
///
/// # Errors
///
/// Returns `401` if the caller is not authenticated or the user record is missing,
/// `403` if not a member of the target org, and `500` on internal failures.
pub async fn switch_org(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(req): Json<SwitchOrgRequest>,
) -> Result<Response, AuthHandlerError> {
    let org_id = OrganisationId::from_uuid(req.org_id);

    // Verify membership.
    let membership = state
        .membership_repo
        .find_membership(user.user_id, org_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::Forbidden("not a member of that organisation"))?;

    // Load user for current token_secret.
    let db_user = state
        .auth_user_repo
        .find_by_id(user.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::Unauthorized("user not found"))?;

    // Issue JWT for new org.
    let access_token = JwtEngine::issue(
        user.user_id,
        org_id,
        &user.email,
        membership.role,
        &db_user.token_secret,
    )
    .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // Create refresh token.
    let (_token, raw_refresh) = state
        .refresh_token_repo
        .create(user.user_id, org_id, None, 30)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

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
