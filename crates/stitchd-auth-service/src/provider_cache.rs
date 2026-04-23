//! On-the-fly provider cache with TTL eviction.
//!
//! [`ProviderCache`] holds built OIDC/SAML provider instances keyed by
//! [`AuthProviderId`]. Entries are evicted after a configurable TTL or
//! explicitly via [`ProviderCache::evict`].

use dashmap::DashMap;
use std::{
    future::Future,
    time::{Duration, Instant},
};
use stitchd_core::id::AuthProviderId;

// ---------------------------------------------------------------------------
// CacheEntry
// ---------------------------------------------------------------------------

struct CacheEntry<T> {
    value: T,
    expires_at: Instant,
}

impl<T> CacheEntry<T> {
    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

// ---------------------------------------------------------------------------
// ProviderCache
// ---------------------------------------------------------------------------

/// Thread-safe TTL cache for lazily-built provider instances.
///
/// `T` is the provider type stored (e.g. `Arc<OidcProvider>` or
/// `Arc<SamlProvider>`). The `factory` async closure is called on a cache miss
/// and its result is cached for `ttl`.
pub struct ProviderCache<T> {
    store: DashMap<AuthProviderId, CacheEntry<T>>,
    ttl: Duration,
}

impl<T: Clone + Send + Sync + 'static> ProviderCache<T> {
    /// Construct a new cache with the given TTL.
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            store: DashMap::new(),
            ttl,
        }
    }

    /// Construct a cache using `PROVIDER_CACHE_TTL_SECS` env var (default: 3600).
    #[must_use]
    pub fn from_env() -> Self {
        let secs: u64 = std::env::var("PROVIDER_CACHE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);
        Self::new(Duration::from_secs(secs))
    }

    /// Return cached value for `id` if still live, or call `factory` to build it.
    ///
    /// `factory` is only called when the entry is absent or has expired.
    ///
    /// # Errors
    /// Propagates any error returned by `factory`.
    pub async fn get_or_build<F, Fut, E>(&self, id: AuthProviderId, factory: F) -> Result<T, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        // Check cache hit first
        if let Some(entry) = self.store.get(&id) {
            if !entry.is_expired() {
                return Ok(entry.value.clone());
            }
        }

        // Cache miss or expired — build the provider
        let value = factory().await?;
        let expires_at = Instant::now() + self.ttl;
        self.store.insert(
            id,
            CacheEntry {
                value: value.clone(),
                expires_at,
            },
        );
        Ok(value)
    }

    /// Immediately evict the entry for `id`.
    pub fn evict(&self, id: AuthProviderId) {
        self.store.remove(&id);
    }

    /// Return the number of live (non-expired) entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.store.iter().filter(|e| !e.is_expired()).count()
    }

    /// Return true if there are no live entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn cache_hit_factory_not_called_again() {
        let cache: ProviderCache<String> = ProviderCache::new(Duration::from_secs(60));
        let id = AuthProviderId::new();
        let call_count = Arc::new(AtomicUsize::new(0));

        let cc = call_count.clone();
        let v1 = cache
            .get_or_build(id, || async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok::<_, String>("provider".to_string())
            })
            .await
            .unwrap();

        let cc2 = call_count.clone();
        let v2 = cache
            .get_or_build(id, || async move {
                cc2.fetch_add(1, Ordering::SeqCst);
                Ok::<_, String>("provider".to_string())
            })
            .await
            .unwrap();

        assert_eq!(v1, v2);
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "factory should only be called once on a hit"
        );
    }

    #[tokio::test]
    async fn cache_miss_after_ttl_expiry() {
        let cache: ProviderCache<String> = ProviderCache::new(Duration::from_millis(50));
        let id = AuthProviderId::new();
        let call_count = Arc::new(AtomicUsize::new(0));

        let cc = call_count.clone();
        cache
            .get_or_build(id, || async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok::<_, String>("v1".to_string())
            })
            .await
            .unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        let cc2 = call_count.clone();
        cache
            .get_or_build(id, || async move {
                cc2.fetch_add(1, Ordering::SeqCst);
                Ok::<_, String>("v2".to_string())
            })
            .await
            .unwrap();

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "factory should be called again after TTL expiry"
        );
    }

    #[tokio::test]
    async fn evict_removes_entry_immediately() {
        let cache: ProviderCache<String> = ProviderCache::new(Duration::from_secs(60));
        let id = AuthProviderId::new();
        let call_count = Arc::new(AtomicUsize::new(0));

        let cc = call_count.clone();
        cache
            .get_or_build(id, || async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok::<_, String>("provider".to_string())
            })
            .await
            .unwrap();

        cache.evict(id);

        let cc2 = call_count.clone();
        cache
            .get_or_build(id, || async move {
                cc2.fetch_add(1, Ordering::SeqCst);
                Ok::<_, String>("provider".to_string())
            })
            .await
            .unwrap();

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "factory should be called again after eviction"
        );
    }

    #[tokio::test]
    async fn concurrent_access_same_id_factory_may_race() {
        // This test verifies that concurrent access doesn't panic or produce
        // incorrect values. Multiple tasks may call factory (last-write-wins
        // is acceptable for a cache — correctness over perfect dedup).
        let cache = Arc::new(ProviderCache::<String>::new(Duration::from_secs(60)));
        let id = AuthProviderId::new();
        let call_count = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..10 {
            let cache = cache.clone();
            let cc = call_count.clone();
            handles.push(tokio::spawn(async move {
                cache
                    .get_or_build(id, || async move {
                        cc.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, String>("provider".to_string())
                    })
                    .await
                    .unwrap()
            }));
        }

        let results = futures::future::join_all(handles).await;
        for r in results {
            assert_eq!(r.unwrap(), "provider");
        }
        // Call count >= 1 (at least one build happened); no panics
        assert!(call_count.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    #[serial]
    async fn from_env_defaults_to_3600_secs() {
        // PROVIDER_CACHE_TTL_SECS not set → default 3600 s TTL
        let cache: ProviderCache<String> = ProviderCache::from_env();
        assert_eq!(cache.ttl, Duration::from_secs(3600));
    }

    #[tokio::test]
    #[serial]
    async fn from_env_reads_env_var() {
        // SAFETY: test-only, single-threaded context for env mutation
        unsafe { std::env::set_var("PROVIDER_CACHE_TTL_SECS", "120") };
        let cache: ProviderCache<String> = ProviderCache::from_env();
        unsafe { std::env::remove_var("PROVIDER_CACHE_TTL_SECS") };
        assert_eq!(cache.ttl, Duration::from_secs(120));
    }
}
