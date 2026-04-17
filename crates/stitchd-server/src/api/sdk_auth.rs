//! Axum extractor for SDK key authentication on list-check routes.
//!
//! The raw key presented via `x-sdk-key` is SHA-256 hashed and compared
//! against the stored `key_hash` for active keys of the target environment.
//! Environments typically have 1–3 active keys, so the linear scan is fine.

use axum::{
    extract::{FromRequestParts, Path},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use sha2::{Digest as _, Sha256};
use stitchd_core::id::EnvironmentId;

use crate::AppState;

/// Successful SDK key authentication result, injected into handlers as an extractor.
///
/// Contains the authenticated environment ID so handlers don't need to re-parse the path.
#[derive(Debug, Clone)]
pub struct SdkAuth {
    /// The environment resolved from the SDK key.
    pub environment_id: EnvironmentId,
}

/// Error returned when SDK key authentication fails.
#[derive(Debug)]
pub enum SdkAuthError {
    /// No `x-sdk-key` header was present.
    MissingKey,
    /// The key was present but did not match any active key for the environment.
    Unauthorized,
    /// An internal error occurred during key verification.
    Internal(String),
}

impl IntoResponse for SdkAuthError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            Self::MissingKey => (StatusCode::UNAUTHORIZED, "missing x-sdk-key header"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "invalid or revoked SDK key"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "auth error"),
        };
        (status, axum::Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

/// Hash a raw SDK key with SHA-256, returning a lowercase hex string.
pub fn hash_sdk_key(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    format!("{digest:x}")
}

impl FromRequestParts<AppState> for SdkAuth {
    type Rejection = SdkAuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Extract the raw key from the header. Copy to owned String to release
        // the borrow on `parts` before the mutable borrow for path extraction.
        let raw_key: String = parts
            .headers
            .get("x-sdk-key")
            .and_then(|v| v.to_str().ok())
            .ok_or(SdkAuthError::MissingKey)?
            .to_owned();

        // Extract env_id from the path — list-check routes are nested under
        // `/v1/environments/{env_id}/...`
        let Path(env_id) = Path::<EnvironmentId>::from_request_parts(parts, state)
            .await
            .map_err(|_| SdkAuthError::Internal("failed to parse env_id from path".to_string()))?;

        // Fetch all active keys for this environment.
        let active_keys = state
            .sdk_key_repo
            .find_active_by_environment(env_id)
            .await
            .map_err(|e| SdkAuthError::Internal(e.to_string()))?;

        // SHA-256 hash the presented key and compare against stored hashes.
        let presented_hash = hash_sdk_key(&raw_key);
        let authenticated = active_keys
            .iter()
            .any(|k| k.key_hash == presented_hash);

        if authenticated {
            Ok(Self {
                environment_id: env_id,
            })
        } else {
            Err(SdkAuthError::Unauthorized)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_sdk_key_is_deterministic() {
        let hash1 = hash_sdk_key("test-key-abc123");
        let hash2 = hash_sdk_key("test-key-abc123");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn hash_sdk_key_different_inputs_differ() {
        let hash1 = hash_sdk_key("key-a");
        let hash2 = hash_sdk_key("key-b");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn hash_sdk_key_is_lowercase_hex() {
        let hash = hash_sdk_key("any-key");
        // SHA-256 output is 64 hex chars
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
