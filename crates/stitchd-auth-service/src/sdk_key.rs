//! SDK key validation.
//!
//! Hashes the presented raw key with SHA-256 and looks it up in the
//! `auth.sdk_keys` table via [`SdkKeyRepository`].  Only active keys are
//! accepted.

use std::sync::Arc;

use sha2::{Digest as _, Sha256};
use stitchd_core::{
    id::{EnvironmentId, SdkKeyId},
    tenant::SdkKey,
};
use stitchd_db::{RepositoryError, SdkKeyRepository};
use tonic::Status;

/// Context returned after a successful SDK key validation.
#[derive(Debug, Clone)]
pub struct SdkKeyContext {
    /// The environment this SDK key is scoped to.
    pub environment_id: EnvironmentId,
    /// The SDK key's own identifier.
    pub sdk_key_id: SdkKeyId,
}

/// Error variants from SDK-key validation.
#[derive(Debug, thiserror::Error)]
pub enum SdkKeyValidationError {
    /// The key hash did not match any active key.
    #[error("invalid or revoked SDK key")]
    NotFound,

    /// An unexpected internal repository error occurred.
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<SdkKeyValidationError> for Status {
    fn from(e: SdkKeyValidationError) -> Self {
        match e {
            SdkKeyValidationError::NotFound => Self::unauthenticated(e.to_string()),
            SdkKeyValidationError::Internal(msg) => Self::internal(msg),
        }
    }
}

/// Hash a raw SDK key with SHA-256, returning a lowercase hex string.
#[must_use]
pub fn hash_sdk_key(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    format!("{digest:x}")
}

/// Validate a raw SDK key by:
/// 1. Hashing the raw key.
/// 2. Looking up the hash in `SdkKeyRepository::find_active_by_hash`.
///
/// # Errors
/// Returns [`SdkKeyValidationError`] if the key is not found or an error occurs.
pub async fn validate_sdk_key(
    raw_key: &str,
    sdk_key_repo: &Arc<dyn SdkKeyRepository>,
) -> Result<SdkKeyContext, SdkKeyValidationError> {
    let hash = hash_sdk_key(raw_key);
    let key: SdkKey = sdk_key_repo
        .find_active_by_hash(&hash)
        .await
        .map_err(|e| match e {
            RepositoryError::NotFound { .. } => SdkKeyValidationError::NotFound,
            other => SdkKeyValidationError::Internal(other.to_string()),
        })?;
    Ok(SdkKeyContext {
        environment_id: key.environment_id,
        sdk_key_id: key.id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Arc;
    use stitchd_core::{
        id::{EnvironmentId, SdkKeyId},
        tenant::SdkKey,
    };
    use stitchd_db::{RepositoryError, SdkKeyRepository};

    // ── Stub SdkKeyRepository ────────────────────────────────────────────────

    struct StubSdkKeyRepo {
        active_keys: Vec<SdkKey>,
    }

    impl StubSdkKeyRepo {
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                active_keys: vec![],
            })
        }

        fn with_key(raw: &str, env_id: EnvironmentId) -> Arc<Self> {
            let key = SdkKey {
                id: SdkKeyId::new(),
                environment_id: env_id,
                key_hash: hash_sdk_key(raw),
                is_active: true,
                created_at: Utc::now(),
                revoked_at: None,
            };
            Arc::new(Self {
                active_keys: vec![key],
            })
        }
    }

    #[tonic::async_trait]
    impl SdkKeyRepository for StubSdkKeyRepo {
        async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(vec![])
        }

        async fn create(&self, _key: &SdkKey) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_active_by_environment(
            &self,
            env_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(self
                .active_keys
                .iter()
                .filter(|k| k.environment_id == env_id)
                .cloned()
                .collect())
        }

        async fn find_active_by_hash(&self, key_hash: &str) -> Result<SdkKey, RepositoryError> {
            self.active_keys
                .iter()
                .find(|k| k.key_hash == key_hash)
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound {
                    id: key_hash.to_string(),
                })
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[test]
    fn hash_sdk_key_is_deterministic() {
        assert_eq!(hash_sdk_key("test-key"), hash_sdk_key("test-key"));
    }

    #[test]
    fn hash_sdk_key_returns_64_char_hex() {
        let h = hash_sdk_key("any-key");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_sdk_key_different_inputs_differ() {
        assert_ne!(hash_sdk_key("key-a"), hash_sdk_key("key-b"));
    }

    #[tokio::test]
    async fn valid_active_key_returns_sdk_key_context() {
        let env_id = EnvironmentId::new();
        let raw = "my-sdk-key-123";
        let repo = StubSdkKeyRepo::with_key(raw, env_id);

        let result = validate_sdk_key(raw, &(repo as Arc<dyn SdkKeyRepository>)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().environment_id, env_id);
    }

    #[tokio::test]
    async fn unknown_key_returns_not_found_error() {
        let repo = StubSdkKeyRepo::empty();

        let result = validate_sdk_key("unknown-key", &(repo as Arc<dyn SdkKeyRepository>)).await;
        assert!(matches!(result, Err(SdkKeyValidationError::NotFound)));
    }

    #[tokio::test]
    async fn sdk_key_not_found_converts_to_unauthenticated_status() {
        let err = SdkKeyValidationError::NotFound;
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn sdk_key_internal_error_converts_to_internal_status() {
        let err = SdkKeyValidationError::Internal("db error".to_string());
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::Internal);
    }
}
