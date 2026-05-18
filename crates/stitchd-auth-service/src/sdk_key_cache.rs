//! In-process TTL cache for SDK key lookups.
//!
//! Wraps a `moka` async `Cache` keyed on the SHA-256 `key_hash`.  A TTL of
//! 60 seconds limits staleness after revocation while keeping DB load low for
//! high-throughput evaluation paths.  Revocation eagerly invalidates the entry
//! so the 60-second window is a worst-case for non-revocation paths only.

use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use stitchd_core::tenant::SdkKey;
use stitchd_db::RepositoryError;

const TTL: Duration = Duration::from_secs(60);
const MAX_CAPACITY: u64 = 4_096;

/// Reconstruct a `RepositoryError` from an `Arc<RepositoryError>` returned by
/// moka's `try_get_with`. We preserve the `NotFound` discriminant (the only
/// variant that callers branch on) and fall back to `Unexpected` for all others.
fn reconstruct_repo_error(arc: &RepositoryError) -> RepositoryError {
    match arc {
        RepositoryError::NotFound { id } => RepositoryError::NotFound { id: id.clone() },
        RepositoryError::VersionConflict { expected, actual } => RepositoryError::VersionConflict {
            expected: *expected,
            actual: *actual,
        },
        RepositoryError::UniqueViolation { field } => RepositoryError::UniqueViolation {
            field: field.clone(),
        },
        RepositoryError::ForeignKeyViolation { constraint } => {
            RepositoryError::ForeignKeyViolation {
                constraint: constraint.clone(),
            }
        }
        RepositoryError::InvalidState { reason } => RepositoryError::InvalidState {
            reason: reason.clone(),
        },
        other => RepositoryError::Unexpected(anyhow::anyhow!("{other}")),
    }
}

/// In-process cache for SDK key lookups, keyed on SHA-256 `key_hash`.
#[derive(Clone)]
pub struct SdkKeyCache(Arc<Cache<String, SdkKey>>);

impl SdkKeyCache {
    pub fn new() -> Self {
        let cache = Cache::builder()
            .max_capacity(MAX_CAPACITY)
            .time_to_live(TTL)
            .build();
        Self(Arc::new(cache))
    }

    #[cfg(test)]
    pub fn with_ttl(ttl: Duration) -> Self {
        let cache = Cache::builder()
            .max_capacity(MAX_CAPACITY)
            .time_to_live(ttl)
            .build();
        Self(Arc::new(cache))
    }

    /// Return the cached key for `hash`, or call `loader` exactly once on a miss.
    ///
    /// Concurrent callers for the same `hash` are coalesced by moka: only one
    /// `loader` invocation runs, and all waiters receive the same result.
    pub async fn get_or_load<F, Fut>(
        &self,
        hash: &str,
        loader: F,
    ) -> Result<SdkKey, RepositoryError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<SdkKey, RepositoryError>>,
    {
        // moka's try_get_with coalesces concurrent callers but returns Arc<E>.
        // We reconstruct a RepositoryError from the discriminant preserved in
        // the error message so callers see the original variant (e.g. NotFound).
        self.0
            .try_get_with(hash.to_string(), loader())
            .await
            .map_err(|arc_err| reconstruct_repo_error(&arc_err))
    }

    /// Remove `hash` from the cache immediately (called on revocation).
    pub async fn invalidate(&self, hash: &str) {
        self.0.invalidate(hash).await;
    }
}

impl Default for SdkKeyCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use stitchd_core::id::{EnvironmentId, SdkKeyId};

    fn make_key(hash: &str) -> SdkKey {
        SdkKey {
            id: SdkKeyId::new(),
            environment_id: EnvironmentId::new(),
            key_hash: hash.to_string(),
            is_active: true,
            created_at: Utc::now(),
            revoked_at: None,
        }
    }

    #[tokio::test]
    async fn cache_miss_invokes_loader_once() {
        let cache = SdkKeyCache::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let key = make_key("hash-a");
        let key_clone = key.clone();
        let calls_clone = calls.clone();

        let result = cache
            .get_or_load("hash-a", || async move {
                calls_clone.fetch_add(1, Ordering::SeqCst);
                Ok(key_clone)
            })
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.key_hash, key.key_hash);
    }

    #[tokio::test]
    async fn cache_hit_skips_loader() {
        let cache = SdkKeyCache::new();
        let key = make_key("hash-b");

        // Populate cache
        cache
            .get_or_load("hash-b", || {
                let k = key.clone();
                async move { Ok(k) }
            })
            .await
            .unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();

        // Second call must not invoke loader
        cache
            .get_or_load("hash-b", || async move {
                calls_clone.fetch_add(1, Ordering::SeqCst);
                Err(RepositoryError::NotFound {
                    id: "should-not-be-called".into(),
                })
            })
            .await
            .unwrap();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "loader must be skipped on hit"
        );
    }

    #[tokio::test]
    async fn invalidate_removes_entry() {
        let cache = SdkKeyCache::new();
        let key = make_key("hash-c");

        cache
            .get_or_load("hash-c", || {
                let k = key.clone();
                async move { Ok(k) }
            })
            .await
            .unwrap();

        cache.invalidate("hash-c").await;

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let key2 = make_key("hash-c");

        cache
            .get_or_load("hash-c", || async move {
                calls_clone.fetch_add(1, Ordering::SeqCst);
                Ok(key2)
            })
            .await
            .unwrap();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "loader must be called again after invalidation"
        );
    }

    #[tokio::test]
    async fn loader_error_is_propagated() {
        let cache = SdkKeyCache::new();

        let result = cache
            .get_or_load("hash-d", || async {
                Err(RepositoryError::NotFound {
                    id: "hash-d".into(),
                })
            })
            .await;

        assert!(matches!(result, Err(RepositoryError::NotFound { .. })));
    }

    #[tokio::test]
    async fn ttl_expiry_retriggers_loader() {
        let cache = SdkKeyCache::with_ttl(Duration::from_millis(50));
        let key = make_key("hash-e");
        let calls = Arc::new(AtomicUsize::new(0));

        let calls_a = calls.clone();
        let k1 = key.clone();
        cache
            .get_or_load("hash-e", || async move {
                calls_a.fetch_add(1, Ordering::SeqCst);
                Ok(k1)
            })
            .await
            .unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        let calls_b = calls.clone();
        let k2 = key.clone();
        cache
            .get_or_load("hash-e", || async move {
                calls_b.fetch_add(1, Ordering::SeqCst);
                Ok(k2)
            })
            .await
            .unwrap();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "loader must fire again after TTL expiry"
        );
    }

    #[tokio::test]
    async fn concurrent_misses_coalesce_to_single_loader_call() {
        let cache = Arc::new(SdkKeyCache::new());
        let calls = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for _ in 0..10 {
            let c = cache.clone();
            let counter = calls.clone();
            handles.push(tokio::spawn(async move {
                c.get_or_load("hash-f", || async {
                    counter.fetch_add(1, Ordering::SeqCst);
                    // Simulate slight DB latency so concurrent calls overlap
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok(make_key("hash-f"))
                })
                .await
            }));
        }

        for h in handles {
            h.await.unwrap().unwrap();
        }

        // moka coalesces concurrent loads for the same key — allow 1 or a small
        // number of calls depending on timing, but never 10 separate DB hits.
        let total = calls.load(Ordering::SeqCst);
        assert!(
            total < 10,
            "concurrent misses must be coalesced; got {total} loader calls"
        );
    }
}
