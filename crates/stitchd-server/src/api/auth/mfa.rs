//! MFA / TOTP handlers.
//!
//! # Endpoints
//! - `POST /v1/users/me/mfa/setup`               — generate TOTP secret, store (unconfirmed)
//! - `POST /v1/users/me/mfa/confirm`              — verify code, activate MFA, return recovery codes
//! - `POST /auth/mfa/verify`                      — verify challenge token + TOTP/recovery code, issue tokens
//! - `POST /v1/users/me/mfa/disable`              — disable MFA (requires current TOTP)
//! - `POST /v1/users/me/mfa/recovery-codes/regenerate` — re-generate recovery codes (requires current TOTP)

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stitchd_core::auth::{TotpEngine, UserStatus, crypto::CryptoKey, jwt::JwtEngine};

use crate::AppState;
use crate::api::auth::middleware::AuthenticatedUser;
use crate::api::auth::password::{AuthHandlerError, TokenResponse};

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// `POST /v1/users/me/mfa/setup` response.
#[derive(Debug, Serialize)]
pub struct MfaSetupResponse {
    /// `otpauth://` URI for scanning with an authenticator app.
    pub otpauth_uri: String,
    /// Base32-encoded secret (for manual entry).
    pub secret_base32: String,
}

/// `POST /v1/users/me/mfa/confirm` request body.
#[derive(Debug, Deserialize)]
pub struct MfaConfirmRequest {
    /// 6-digit TOTP code from the authenticator app.
    pub totp_code: String,
}

/// `POST /v1/users/me/mfa/confirm` response.
#[derive(Debug, Serialize)]
pub struct MfaConfirmResponse {
    /// One-time display of recovery codes.  Store them somewhere safe — they
    /// will never be shown again.
    pub recovery_codes: Vec<String>,
}

/// `POST /auth/mfa/verify` request body.
#[derive(Debug, Deserialize)]
pub struct MfaVerifyRequest {
    /// Short-lived challenge token issued by `POST /auth/login`.
    pub challenge_token: String,
    /// 6-digit TOTP code — provide this **or** `recovery_code`.
    pub totp_code: Option<String>,
    /// Single-use recovery code — provide this **or** `totp_code`.
    pub recovery_code: Option<String>,
}

/// `POST /v1/users/me/mfa/disable` request body.
#[derive(Debug, Deserialize)]
pub struct MfaDisableRequest {
    /// Current 6-digit TOTP code (required to confirm the caller's identity).
    pub totp_code: String,
}

/// `POST /v1/users/me/mfa/recovery-codes/regenerate` request body.
#[derive(Debug, Deserialize)]
pub struct MfaRegenerateRequest {
    /// Current 6-digit TOTP code.
    pub totp_code: String,
}

// ---------------------------------------------------------------------------
// Helper: SHA-256 hex of a raw token string
// ---------------------------------------------------------------------------

/// Compute the SHA-256 hash of a raw opaque token (same as `password.rs`).
fn sha256_hex_of_token(raw: &str) -> String {
    hex::decode(raw).map_or_else(
        |_| hex::encode(Sha256::digest(raw.as_bytes())),
        |bytes| hex::encode(Sha256::digest(&bytes)),
    )
}

// ---------------------------------------------------------------------------
// POST /v1/users/me/mfa/setup
// ---------------------------------------------------------------------------

/// `POST /v1/users/me/mfa/setup` — generate a TOTP secret for the caller.
///
/// The secret is stored in `users.totp_secret` (encrypted) but `totp_enabled`
/// stays `false` until the caller confirms ownership via `/mfa/confirm`.
///
/// # Errors
/// Returns `500` on cryptographic or database failure.
pub async fn setup(
    State(state): State<AppState>,
    caller: AuthenticatedUser,
) -> Result<Response, AuthHandlerError> {
    // 1. Load user to get their email for the TOTP URI.
    let user = state
        .auth_user_repo
        .find_by_id(caller.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or_else(|| AuthHandlerError::Internal("user not found".into()))?;

    // 2. Generate TOTP secret.
    let (secret_bytes, otpauth_uri) = TotpEngine::generate_secret(&user.email)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // 3. Encrypt the secret.
    let crypto = CryptoKey::from_env()
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;
    let encrypted = crypto
        .encrypt(&secret_bytes)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // 4. Encode secret as base32 for manual entry.
    let secret_base32 = TotpEngine::secret_to_base32(&secret_bytes)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // 5. Store encrypted secret (totp_enabled stays false until confirmed).
    state
        .mfa_repo
        .store_pending_totp_secret(caller.user_id, encrypted)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(MfaSetupResponse {
            otpauth_uri,
            secret_base32,
        }),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// POST /v1/users/me/mfa/confirm
// ---------------------------------------------------------------------------

/// `POST /v1/users/me/mfa/confirm` — verify the TOTP code and activate MFA.
///
/// # Errors
/// Returns `400` for invalid code, `500` on internal failure.
pub async fn confirm(
    State(state): State<AppState>,
    caller: AuthenticatedUser,
    Json(req): Json<MfaConfirmRequest>,
) -> Result<Response, AuthHandlerError> {
    // 1. Get the stored (encrypted) TOTP secret.
    let encrypted = state
        .mfa_repo
        .get_totp_secret(caller.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::BadRequest("MFA setup not started — call /mfa/setup first"))?;

    // 2. Decrypt.
    let crypto = CryptoKey::from_env()
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;
    let secret_bytes = crypto
        .decrypt(&encrypted)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // 3. Verify the TOTP code.
    let valid = TotpEngine::verify(&secret_bytes, &req.totp_code)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    if !valid {
        return Err(AuthHandlerError::BadRequest("invalid TOTP code"));
    }

    // 4. Generate 10 recovery codes.
    let pairs = TotpEngine::generate_recovery_codes(10);
    let plain_codes: Vec<String> = pairs.iter().map(|(p, _)| p.clone()).collect();
    let hashes: Vec<String> = pairs.into_iter().map(|(_, h)| h).collect();

    // 5. Enable TOTP (stores encrypted secret + recovery code hashes).
    state
        .mfa_repo
        .enable_totp(caller.user_id, encrypted, hashes)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(MfaConfirmResponse {
            recovery_codes: plain_codes,
        }),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// POST /auth/mfa/verify
// ---------------------------------------------------------------------------

/// Resolve the `user_id` stored in a challenge row (after the challenge has
/// already been consumed / marked used).
async fn challenge_user_id(
    state: &AppState,
    token_hash: &str,
) -> Result<stitchd_core::id::UserId, AuthHandlerError> {
    state
        .mfa_repo
        .get_user_id_for_challenge(token_hash)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::Unauthorized("challenge not found"))
}

/// Verify the TOTP or recovery code in `req` against `secret_bytes`.
async fn authenticate_mfa_code(
    state: &AppState,
    user_id: stitchd_core::id::UserId,
    secret_bytes: &[u8],
    req: &MfaVerifyRequest,
) -> Result<bool, AuthHandlerError> {
    if let Some(ref totp_code) = req.totp_code {
        TotpEngine::verify(secret_bytes, totp_code)
            .map_err(|e| AuthHandlerError::Internal(e.to_string()))
    } else if let Some(ref recovery_code) = req.recovery_code {
        let code_hash = stitchd_core::auth::crypto::hash_password(recovery_code)
            .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;
        state
            .mfa_repo
            .consume_recovery_code(user_id, &code_hash)
            .await
            .map_err(|e| AuthHandlerError::Internal(e.to_string()))
    } else {
        Err(AuthHandlerError::BadRequest(
            "provide either totp_code or recovery_code",
        ))
    }
}

/// `POST /auth/mfa/verify` — exchange a challenge token + TOTP/recovery code for tokens.
///
/// # Errors
/// Returns `401` for invalid/expired challenge or wrong code, `500` on internal failure.
pub async fn verify(
    State(state): State<AppState>,
    Json(req): Json<MfaVerifyRequest>,
) -> Result<Response, AuthHandlerError> {
    // 1. Consume the challenge (checks expiry and used_at).
    let token_hash = sha256_hex_of_token(&req.challenge_token);
    state
        .mfa_repo
        .consume_challenge(&token_hash)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::Unauthorized("challenge token expired or already used"))?;

    // 2. Look up the user_id from the challenge row.
    let user_id = challenge_user_id(&state, &token_hash).await?;

    // 3. Load user and check status.
    let user = state
        .auth_user_repo
        .find_by_id(user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::Unauthorized("user not found"))?;

    if user.status == UserStatus::Deactivated {
        return Err(AuthHandlerError::Forbidden("account deactivated"));
    }

    // 4. Get and decrypt TOTP secret.
    let encrypted = state
        .mfa_repo
        .get_totp_secret(user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::Unauthorized("MFA not configured for this user"))?;
    let crypto = CryptoKey::from_env().map_err(|e| AuthHandlerError::Internal(e.to_string()))?;
    let secret_bytes = crypto
        .decrypt(&encrypted)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // 5. Verify TOTP code or recovery code.
    if !authenticate_mfa_code(&state, user_id, &secret_bytes, &req).await? {
        return Err(AuthHandlerError::Unauthorized("invalid MFA code"));
    }

    // 6. Resolve org membership and issue tokens.
    let memberships = state
        .membership_repo
        .list_orgs_for_user(user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    if memberships.is_empty() {
        return Err(AuthHandlerError::Forbidden("no organisation membership"));
    }

    let membership = &memberships[0];
    let access_token =
        JwtEngine::issue(user.id, membership.org_id, &user.email, membership.role, &user.token_secret)
            .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;
    let (_token, raw_refresh) = state
        .refresh_token_repo
        .create(user.id, membership.org_id, None, 30)
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
// POST /v1/users/me/mfa/disable
// ---------------------------------------------------------------------------

/// `POST /v1/users/me/mfa/disable` — disable TOTP for the caller.
///
/// Requires a valid current TOTP code to confirm the caller's identity.
///
/// # Errors
/// Returns `400` for invalid code, `500` on internal failure.
pub async fn disable(
    State(state): State<AppState>,
    caller: AuthenticatedUser,
    Json(req): Json<MfaDisableRequest>,
) -> Result<Response, AuthHandlerError> {
    // 1. Get encrypted secret.
    let encrypted = state
        .mfa_repo
        .get_totp_secret(caller.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::BadRequest("MFA is not enabled"))?;

    // 2. Decrypt and verify.
    let crypto = CryptoKey::from_env()
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;
    let secret_bytes = crypto
        .decrypt(&encrypted)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    let valid = TotpEngine::verify(&secret_bytes, &req.totp_code)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    if !valid {
        return Err(AuthHandlerError::BadRequest("invalid TOTP code"));
    }

    // 3. Disable TOTP (clears secret + recovery codes).
    state
        .mfa_repo
        .disable_totp(caller.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// POST /v1/users/me/mfa/recovery-codes/regenerate
// ---------------------------------------------------------------------------

/// `POST /v1/users/me/mfa/recovery-codes/regenerate` — replace all recovery codes.
///
/// # Errors
/// Returns `400` for invalid code, `500` on internal failure.
pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    caller: AuthenticatedUser,
    Json(req): Json<MfaRegenerateRequest>,
) -> Result<Response, AuthHandlerError> {
    // 1. Get encrypted secret.
    let encrypted = state
        .mfa_repo
        .get_totp_secret(caller.user_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::BadRequest("MFA is not enabled"))?;

    // 2. Decrypt and verify TOTP code.
    let crypto = CryptoKey::from_env()
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;
    let secret_bytes = crypto
        .decrypt(&encrypted)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    let valid = TotpEngine::verify(&secret_bytes, &req.totp_code)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    if !valid {
        return Err(AuthHandlerError::BadRequest("invalid TOTP code"));
    }

    // 3. Generate new recovery codes.
    let pairs = TotpEngine::generate_recovery_codes(10);
    let plain_codes: Vec<String> = pairs.iter().map(|(p, _)| p.clone()).collect();
    let hashes: Vec<String> = pairs.into_iter().map(|(_, h)| h).collect();

    // 4. Store via enable_totp (which atomically replaces recovery codes).
    state
        .mfa_repo
        .enable_totp(caller.user_id, encrypted, hashes)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(MfaConfirmResponse {
            recovery_codes: plain_codes,
        }),
    )
        .into_response())
}
