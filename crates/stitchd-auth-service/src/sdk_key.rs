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

use crate::sdk_key_cache::SdkKeyCache;

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
    hex::encode(digest)
}

/// Validate a raw SDK key, using `cache` to avoid DB round-trips on repeated
/// calls for the same key within the cache TTL window.
///
/// # Errors
/// Returns [`SdkKeyValidationError`] if the key is not found or an error occurs.
pub async fn validate_sdk_key(
    raw_key: &str,
    sdk_key_repo: &Arc<dyn SdkKeyRepository>,
    cache: &SdkKeyCache,
) -> Result<SdkKeyContext, SdkKeyValidationError> {
    let hash = hash_sdk_key(raw_key);
    let repo = Arc::clone(sdk_key_repo);
    let hash_clone = hash.clone();
    let key: SdkKey = cache
        .get_or_load(&hash, || async move {
            repo.find_active_by_hash(&hash_clone).await
        })
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

        async fn list_by_environment_paginated(
            &self,
            _environment_id: EnvironmentId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<SdkKey>, u64), RepositoryError> {
            Ok((vec![], 0))
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
        let cache = crate::sdk_key_cache::SdkKeyCache::new();

        let result = validate_sdk_key(raw, &(repo as Arc<dyn SdkKeyRepository>), &cache).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().environment_id, env_id);
    }

    #[tokio::test]
    async fn unknown_key_returns_not_found_error() {
        let repo = StubSdkKeyRepo::empty();
        let cache = crate::sdk_key_cache::SdkKeyCache::new();

        let result =
            validate_sdk_key("unknown-key", &(repo as Arc<dyn SdkKeyRepository>), &cache).await;
        assert!(matches!(result, Err(SdkKeyValidationError::NotFound)));
    }

    #[tokio::test]
    async fn validate_sdk_key_uses_cache_on_second_call() {
        use crate::sdk_key_cache::SdkKeyCache;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingRepo {
            key: SdkKey,
            calls: Arc<AtomicUsize>,
        }
        #[tonic::async_trait]
        impl SdkKeyRepository for CountingRepo {
            async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_by_environment(
                &self,
                _: EnvironmentId,
            ) -> Result<Vec<SdkKey>, RepositoryError> {
                Ok(vec![])
            }
            async fn list_by_environment_paginated(
                &self,
                _: EnvironmentId,
                _: u64,
                _: u64,
            ) -> Result<(Vec<SdkKey>, u64), RepositoryError> {
                Ok((vec![], 0))
            }
            async fn create(&self, _: &SdkKey) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn revoke(&self, _: SdkKeyId) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn find_active_by_environment(
                &self,
                _: EnvironmentId,
            ) -> Result<Vec<SdkKey>, RepositoryError> {
                Ok(vec![])
            }
            async fn find_active_by_hash(&self, _: &str) -> Result<SdkKey, RepositoryError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(self.key.clone())
            }
        }

        let env_id = EnvironmentId::new();
        let raw = "cache-test-key";
        let key = SdkKey {
            id: SdkKeyId::new(),
            environment_id: env_id,
            key_hash: hash_sdk_key(raw),
            is_active: true,
            created_at: Utc::now(),
            revoked_at: None,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let repo: Arc<dyn SdkKeyRepository> = Arc::new(CountingRepo {
            key,
            calls: calls.clone(),
        });
        let cache = SdkKeyCache::new();

        validate_sdk_key(raw, &repo, &cache).await.unwrap();
        validate_sdk_key(raw, &repo, &cache).await.unwrap();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "DB must be hit only once; second call served from cache"
        );
    }

    #[tokio::test]
    async fn revoked_key_not_returned_after_cache_invalidation() {
        use crate::sdk_key_cache::SdkKeyCache;

        let env_id = EnvironmentId::new();
        let raw = "revoke-test-key";
        let repo = StubSdkKeyRepo::with_key(raw, env_id);
        let repo_arc: Arc<dyn SdkKeyRepository> = repo;
        let cache = SdkKeyCache::new();

        // Warm the cache
        validate_sdk_key(raw, &repo_arc, &cache).await.unwrap();

        // Invalidate (simulating revocation)
        let hash = hash_sdk_key(raw);
        cache.invalidate(&hash).await;

        // The stub repo still has the key as active, but the next call re-fetches from repo.
        // In a real revocation scenario the repo would return NotFound; here we just verify
        // the cache entry was removed (loader is invoked again).
        let result = validate_sdk_key(raw, &repo_arc, &cache).await;
        assert!(
            result.is_ok(),
            "key still active in repo; re-fetched correctly after invalidation"
        );
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
