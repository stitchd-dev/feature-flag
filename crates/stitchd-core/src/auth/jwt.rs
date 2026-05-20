//! JWT engine: issue and verify short-lived access tokens.
//!
//! Tokens are signed with HMAC-SHA256 using the per-user `token_secret` UUID.
//! This means rotating `token_secret` in the database immediately invalidates
//! all previously-issued tokens for that user.

use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    auth::types::{OrgRole, UserStatus},
    id::{OrganisationId, UserId},
};

/// Access token lifetime: 15 minutes.
const ACCESS_TOKEN_TTL_SECS: u64 = 15 * 60;

/// Claims embedded in an access JWT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessTokenClaims {
    /// Subject — user ID as hyphenated UUID string.
    pub sub: String,
    /// Organisation ID as hyphenated UUID string.
    pub org_id: String,
    /// User's email address.
    pub email: String,
    /// User's role within the organisation.
    pub org_role: OrgRole,
    /// `true` when the token was issued for a user in the platform System org.
    /// Defaults to `false` for tokens issued before this field was added.
    #[serde(default)]
    pub is_system: bool,
    /// Expiry as Unix timestamp (seconds).
    pub exp: usize,
    /// Issued-at as Unix timestamp (seconds).
    pub iat: usize,
}

/// Stateless JWT engine — all methods are `fn` with explicit key input.
pub struct JwtEngine;

impl JwtEngine {
    /// Issue a 15-minute JWT signed with HMAC-SHA256 using `token_secret`.
    ///
    /// # Errors
    /// Returns [`JwtError::Encode`] on failure.
    pub fn issue(
        user_id: UserId,
        org_id: OrganisationId,
        email: &str,
        org_role: OrgRole,
        is_system: bool,
        token_secret: &uuid::Uuid,
    ) -> Result<String, JwtError> {
        let now = jsonwebtoken::get_current_timestamp() as usize;
        let claims = AccessTokenClaims {
            sub: user_id.to_string(),
            org_id: org_id.to_string(),
            email: email.to_owned(),
            org_role,
            is_system,
            iat: now,
            exp: now + ACCESS_TOKEN_TTL_SECS as usize,
        };
        let key = EncodingKey::from_secret(token_secret.as_bytes());
        encode(&Header::default(), &claims, &key).map_err(|e| JwtError::Encode(e.to_string()))
    }

    /// Verify `token` using `token_secret` and return the decoded claims.
    ///
    /// Validates signature, expiry, and required claims automatically.
    ///
    /// # Errors
    /// Returns [`JwtError::Decode`] if the token is invalid, expired, or signed
    /// with a different secret.
    pub fn verify(token: &str, token_secret: &uuid::Uuid) -> Result<AccessTokenClaims, JwtError> {
        let key = DecodingKey::from_secret(token_secret.as_bytes());
        let mut validation = Validation::default();
        validation.required_spec_claims = ["exp", "sub"].iter().map(|s| s.to_string()).collect();
        decode::<AccessTokenClaims>(token, &key, &validation)
            .map(|data| data.claims)
            .map_err(|e| JwtError::Decode(e.to_string()))
    }

    /// Decode a JWT *without* verifying the signature.
    ///
    /// Used during auth middleware to extract `sub` (user_id) before fetching
    /// `token_secret` from the database for full verification.
    ///
    /// # Errors
    /// Returns [`JwtError::Decode`] if the token is structurally invalid.
    pub fn decode_unverified(token: &str) -> Result<AccessTokenClaims, JwtError> {
        // jsonwebtoken 10 deprecated `Validation::insecure_disable_signature_validation`
        // in favour of `dangerous::insecure_decode`, which doesn't need a
        // DecodingKey or a Validation at all — same semantic, cleaner API.
        jsonwebtoken::dangerous::insecure_decode::<AccessTokenClaims>(token)
            .map(|data| data.claims)
            .map_err(|e| JwtError::Decode(e.to_string()))
    }
}

/// Errors from JWT operations.
#[derive(Debug, Error)]
pub enum JwtError {
    /// Token could not be encoded.
    #[error("JWT encode error: {0}")]
    Encode(String),

    /// Token could not be decoded or failed validation.
    #[error("JWT decode error: {0}")]
    Decode(String),
}

// Make JwtError usable as a clippy-clean unused import suppressor for UserStatus
// (referenced only in the middleware, not directly here — suppress warning)
#[allow(dead_code)]
fn _use_user_status(_: UserStatus) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ids() -> (UserId, OrganisationId, uuid::Uuid) {
        (UserId::new(), OrganisationId::new(), uuid::Uuid::new_v4())
    }

    // -----------------------------------------------------------------------
    // Issue + verify round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn issue_and_verify_roundtrip() {
        let (user_id, org_id, secret) = test_ids();
        let token = JwtEngine::issue(
            user_id,
            org_id,
            "alice@example.com",
            OrgRole::OrgAdmin,
            false,
            &secret,
        )
        .unwrap();
        let claims = JwtEngine::verify(&token, &secret).unwrap();

        assert_eq!(claims.sub, user_id.to_string());
        assert_eq!(claims.org_id, org_id.to_string());
        assert_eq!(claims.email, "alice@example.com");
        assert_eq!(claims.org_role, OrgRole::OrgAdmin);
    }

    // -----------------------------------------------------------------------
    // Expired token
    // -----------------------------------------------------------------------

    #[test]
    fn expired_token_returns_error() {
        let (user_id, org_id, secret) = test_ids();

        // Manually craft claims with exp in the past
        let claims = AccessTokenClaims {
            sub: user_id.to_string(),
            org_id: org_id.to_string(),
            email: "bob@example.com".to_owned(),
            org_role: OrgRole::OrgMember,
            is_system: false,
            iat: 1_000_000,
            exp: 1_000_001, // far in the past
        };
        let key = EncodingKey::from_secret(secret.as_bytes());
        let token = jsonwebtoken::encode(&Header::default(), &claims, &key).unwrap();

        let result = JwtEngine::verify(&token, &secret);
        assert!(result.is_err(), "expired token should fail");
    }

    // -----------------------------------------------------------------------
    // Wrong secret
    // -----------------------------------------------------------------------

    #[test]
    fn wrong_secret_returns_error() {
        let (user_id, org_id, secret) = test_ids();
        let token = JwtEngine::issue(
            user_id,
            org_id,
            "carol@example.com",
            OrgRole::OrgMember,
            false,
            &secret,
        )
        .unwrap();

        let wrong_secret = uuid::Uuid::new_v4();
        let result = JwtEngine::verify(&token, &wrong_secret);
        assert!(result.is_err(), "wrong secret should fail verification");
    }

    // -----------------------------------------------------------------------
    // Token secret rotation
    // -----------------------------------------------------------------------

    #[test]
    fn rotated_secret_invalidates_old_token() {
        let (user_id, org_id, original_secret) = test_ids();
        let token = JwtEngine::issue(
            user_id,
            org_id,
            "dave@example.com",
            OrgRole::OrgAdmin,
            false,
            &original_secret,
        )
        .unwrap();

        // Simulate rotation: new UUID
        let new_secret = uuid::Uuid::new_v4();
        let result = JwtEngine::verify(&token, &new_secret);
        assert!(
            result.is_err(),
            "old token should be invalid after secret rotation"
        );
    }

    // -----------------------------------------------------------------------
    // Decode unverified
    // -----------------------------------------------------------------------

    #[test]
    fn decode_unverified_extracts_sub() {
        let (user_id, org_id, secret) = test_ids();
        let token = JwtEngine::issue(
            user_id,
            org_id,
            "eve@example.com",
            OrgRole::OrgMember,
            false,
            &secret,
        )
        .unwrap();

        let claims = JwtEngine::decode_unverified(&token).unwrap();
        assert_eq!(claims.sub, user_id.to_string());
    }
}
