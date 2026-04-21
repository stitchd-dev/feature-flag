//! Password reset handlers.
//!
//! # Endpoints
//! - `POST /auth/password/reset-request` — request a 6-digit OTP
//! - `POST /auth/password/reset`          — submit OTP + new password

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::api::auth::password::AuthHandlerError;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// `POST /auth/password/reset-request` request body.
#[derive(Debug, Deserialize)]
pub struct ResetRequestBody {
    /// Email of the account to reset.
    pub email: String,
}

/// `POST /auth/password/reset-request` response.
#[derive(Debug, Serialize)]
pub struct ResetRequestResponse {
    /// Informational message — never reveals whether the email exists.
    pub message: &'static str,
    /// Present when SMTP is unavailable — the raw OTP code for out-of-band delivery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline_link: Option<String>,
}

/// `POST /auth/password/reset` request body.
#[derive(Debug, Deserialize)]
pub struct ResetBody {
    /// Email of the account being reset.
    pub email: String,
    /// The 6-digit OTP previously sent.
    pub otp: String,
    /// The new password to set.
    pub new_password: String,
}

// ---------------------------------------------------------------------------
// POST /auth/password/reset-request
// ---------------------------------------------------------------------------

/// `POST /auth/password/reset-request` — send a 6-digit OTP if the email is registered.
///
/// Always returns 200; never reveals whether the email is registered.
///
/// # Errors
/// Returns `500` on internal failures.
pub async fn reset_request(
    State(state): State<AppState>,
    Json(req): Json<ResetRequestBody>,
) -> Result<Json<ResetRequestResponse>, AuthHandlerError> {
    // Silently ignore unknown emails — don't reveal account existence.
    let user_opt = state
        .auth_user_repo
        .find_by_email(&req.email)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    let offline_link = if user_opt.is_some() {
        // Create an OTP.
        let (_otp_id, raw_otp) = state
            .otp_repo
            .create(&req.email)
            .await
            .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

        let offline_url = format!("/auth/password/reset?email={}&otp={raw_otp}", req.email);

        state
            .email_service
            .send(
                &req.email,
                "Your password reset code",
                &format!("Your 6-digit reset code is: {raw_otp}\nIt expires in 10 minutes."),
                &offline_url,
            )
            .await
            .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
            .map(|link| link.url)
    } else {
        None
    };

    Ok(Json(ResetRequestResponse {
        message: "If that email is registered, a reset code was sent.",
        offline_link,
    }))
}

// ---------------------------------------------------------------------------
// POST /auth/password/reset
// ---------------------------------------------------------------------------

/// `POST /auth/password/reset` — verify OTP and update password.
///
/// # Errors
/// Returns `400` for invalid/expired OTP, `500` on internal failures.
pub async fn reset_password(
    State(state): State<AppState>,
    Json(req): Json<ResetBody>,
) -> Result<Response, AuthHandlerError> {
    // 1. Find valid OTP for this email.
    let otp_record = state
        .otp_repo
        .find_valid_by_email(&req.email)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::BadRequest("invalid or expired reset code"))?;

    let (otp_id, otp_hash) = otp_record;

    // 2. Verify the submitted OTP against the stored hash.
    let ok = stitchd_core::auth::crypto::verify_password(&req.otp, &otp_hash)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    if !ok {
        return Err(AuthHandlerError::BadRequest("invalid or expired reset code"));
    }

    // 3. Consume (mark used) the OTP.
    state
        .otp_repo
        .consume(otp_id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // 4. Find the user and update the password.
    let user = state
        .auth_user_repo
        .find_by_email(&req.email)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?
        .ok_or(AuthHandlerError::BadRequest("invalid or expired reset code"))?;

    let new_hash = stitchd_core::auth::crypto::hash_password(&req.new_password)
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    state
        .auth_user_repo
        .update_password_hash(user.id, &new_hash)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    // 5. Rotate token secret + revoke all sessions to invalidate everything.
    state
        .auth_user_repo
        .rotate_token_secret(user.id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    state
        .refresh_token_repo
        .revoke_all_for_user(user.id)
        .await
        .map_err(|e| AuthHandlerError::Internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "message": "Password updated. Please log in again." })),
    )
        .into_response())
}
