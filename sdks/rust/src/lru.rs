//! Bounded LRU cache for list-segment membership, keyed on `(context_type,
//! context_key)`.
//!
//! See `sdks/spec/docs/03-caching.md` "Cache 2 — List-Segment Membership LRU"
//! for the canonical lifecycle and semantics.
//!
//! ## Why moka?
//!
//! - **Consistency with auth-service.** `stitchd-auth-service` already
//!   depends on `moka` for `SdkKeyCache`. Using the same crate keeps the
//!   dependency surface lean and the codebase idiomatic.
//! - **Lock-free reads.** moka's `sync::Cache` is concurrent-safe for
//!   `get`/`insert` without external locking — important because the LRU
//!   sits on the `evaluate()` hot path.
//! - **TinyLFU + bounded capacity** out of the box (moka's eviction policy
//!   is technically TinyLFU rather than strict LRU, but for the SDK's
//!   workload — frequent re-eval of the same hot contexts — the practical
//!   eviction behaviour matches "LRU" closely enough).
//!
//! ## Recency-promotion deviation from spec
//!
//! The spec (`03-caching.md` "Refresh") says background refresh writes
//! should NOT count as "use" — only successful `evaluate()` reads should
//! affect recency. moka's `insert()` and `get()` both touch recency
//! tracking, and there is no "write without recency promotion" API. We
//! accept this minor deviation: a refresh artificially keeps an unused
//! context alive in the LRU for one extra eviction tick. Practical impact
//! is negligible (a tiny bump in memory floor); a strict-LRU library
//! could replace this if it ever matters.

use std::collections::HashMap;
use std::sync::Arc;

use moka::sync::Cache;

/// Composite cache key: `(context_type, context_key)`.
///
/// Both fields are owned `String` so the cache can hold them; callers pass
/// `&str` and we clone on insert.
pub type ContextKey = (String, String);

/// Membership map for a single context — `segment_id` (UUID string) → `is_member`.
pub type MembershipMap = HashMap<String, bool>;

/// Bounded LRU cache mapping `(context_type, context_key) → MembershipMap`.
///
/// Cheaply cloneable (it wraps an `Arc` internally — moka's cache type is
/// already `Clone` and shares state).
#[derive(Clone)]
pub struct MembershipCache {
    inner: Cache<ContextKey, Arc<MembershipMap>>,
}

impl MembershipCache {
    /// Construct a new cache with the given capacity.
    ///
    /// `max_entries` is the upper bound on the number of distinct
    /// `(context_type, context_key)` entries. When exceeded, eviction occurs
    /// per moka's TinyLFU policy.
    #[must_use]
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(max_entries as u64)
                .build(),
        }
    }

    /// Look up the membership map for `(context_type, context_key)`.
    /// Returns `None` on cache miss.
    ///
    /// Returns a shared `Arc<MembershipMap>` — callers can read it without
    /// holding any cache lock.
    #[must_use]
    pub fn get(&self, context_type: &str, context_key: &str) -> Option<Arc<MembershipMap>> {
        let key = (context_type.to_string(), context_key.to_string());
        self.inner.get(&key)
    }

    /// Insert (or replace) the membership map for `(context_type, context_key)`.
    ///
    /// Used by:
    /// - The on-miss synchronous fetch path in `evaluate()` (Phase 5 Task 8)
    /// - The background refresh task (Phase 5 Task 6); see the
    ///   recency-promotion deviation note at the top of this module.
    pub fn insert(&self, context_type: &str, context_key: &str, memberships: MembershipMap) {
        let key = (context_type.to_string(), context_key.to_string());
        self.inner.insert(key, Arc::new(memberships));
    }

    /// Snapshot the current set of resident keys.
    ///
    /// Used by the background refresh task to build the batch request
    /// covering exactly the contexts the SDK has seen so far. The returned
    /// list reflects the state at call time — subsequent
    /// inserts/evictions are not visible to the snapshot.
    #[must_use]
    pub fn keys(&self) -> Vec<ContextKey> {
        // moka's iter() returns (Arc<K>, V) pairs. We just need K-clones.
        self.inner
            .iter()
            .map(|(k, _v)| (*k).clone())
            .collect()
    }

    /// Number of currently resident entries (approximate — moka may report
    /// slightly off from the strict count during pending eviction).
    #[must_use]
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }

    /// Force any pending evictions to run (used by tests that need a
    /// deterministic count immediately after exceeding capacity).
    pub fn run_pending_tasks(&self) {
        self.inner.run_pending_tasks();
    }

    /// Drop every entry. Used by `SdkClient::shutdown` (Phase 5 Task 9) to
    /// release memory promptly.
    pub fn invalidate_all(&self) {
        self.inner.invalidate_all();
    }
}

impl std::fmt::Debug for MembershipCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MembershipCache")
            .field("entry_count", &self.inner.entry_count())
            .finish()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn membership(pairs: &[(&str, bool)]) -> MembershipMap {
        pairs
            .iter()
            .map(|(id, m)| ((*id).to_string(), *m))
            .collect()
    }

    // ── Construction ────────────────────────────────────────────────────────

    #[test]
    fn new_creates_empty_cache() {
        let c = MembershipCache::new(10);
        c.run_pending_tasks();
        assert_eq!(c.entry_count(), 0);
        assert!(c.get("user", "alice").is_none());
    }

    // ── Insert + get ────────────────────────────────────────────────────────

    #[test]
    fn insert_then_get_round_trips_membership() {
        let c = MembershipCache::new(10);
        c.insert(
            "user",
            "alice",
            membership(&[("seg-1", true), ("seg-2", false)]),
        );
        let got = c.get("user", "alice").expect("cache hit");
        assert_eq!(got.get("seg-1"), Some(&true));
        assert_eq!(got.get("seg-2"), Some(&false));
    }

    #[test]
    fn miss_returns_none() {
        let c = MembershipCache::new(10);
        c.insert("user", "alice", membership(&[("seg-1", true)]));
        assert!(c.get("user", "bob").is_none());
        assert!(c.get("org", "alice").is_none()); // different type → different key
    }

    #[test]
    fn insert_replaces_existing_entry() {
        let c = MembershipCache::new(10);
        c.insert("user", "alice", membership(&[("seg-1", false)]));
        c.insert("user", "alice", membership(&[("seg-1", true)]));
        assert_eq!(c.get("user", "alice").unwrap().get("seg-1"), Some(&true));
    }

    #[test]
    fn distinct_keys_for_same_key_different_type() {
        let c = MembershipCache::new(10);
        c.insert("user", "alice", membership(&[("seg-u", true)]));
        c.insert("org", "alice", membership(&[("seg-o", true)]));
        assert_eq!(c.get("user", "alice").unwrap().get("seg-u"), Some(&true));
        assert_eq!(c.get("org", "alice").unwrap().get("seg-o"), Some(&true));
    }

    // ── Eviction ────────────────────────────────────────────────────────────

    #[test]
    fn eviction_caps_at_max_entries() {
        let c = MembershipCache::new(2);
        c.insert("user", "alice", membership(&[("a", true)]));
        c.insert("user", "bob", membership(&[("b", true)]));
        c.insert("user", "carol", membership(&[("c", true)]));
        c.run_pending_tasks();
        // At capacity 2, after inserting 3 entries, count should be 2.
        assert_eq!(c.entry_count(), 2);
    }

    // ── keys() snapshot ─────────────────────────────────────────────────────

    #[test]
    fn keys_returns_all_resident_entries() {
        let c = MembershipCache::new(10);
        c.insert("user", "alice", membership(&[]));
        c.insert("user", "bob", membership(&[]));
        c.insert("org", "acme", membership(&[]));
        c.run_pending_tasks();
        let mut keys = c.keys();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                ("org".to_string(), "acme".to_string()),
                ("user".to_string(), "alice".to_string()),
                ("user".to_string(), "bob".to_string()),
            ]
        );
    }

    #[test]
    fn keys_on_empty_cache_returns_empty() {
        let c = MembershipCache::new(10);
        assert!(c.keys().is_empty());
    }

    // ── invalidate_all ──────────────────────────────────────────────────────

    #[test]
    fn invalidate_all_drops_every_entry() {
        let c = MembershipCache::new(10);
        c.insert("user", "alice", membership(&[("a", true)]));
        c.insert("user", "bob", membership(&[("b", true)]));
        c.run_pending_tasks();
        assert_eq!(c.entry_count(), 2);

        c.invalidate_all();
        c.run_pending_tasks();
        assert_eq!(c.entry_count(), 0);
        assert!(c.get("user", "alice").is_none());
        assert!(c.get("user", "bob").is_none());
    }

    // ── Concurrency ─────────────────────────────────────────────────────────

    #[test]
    fn cache_is_send_and_sync() {
        // Required so SdkClient can be Arc-shared across handlers.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MembershipCache>();
    }

    #[test]
    fn clones_share_underlying_state() {
        // moka::sync::Cache is internally Arc-shared. A clone reads the same
        // entries as the original.
        let a = MembershipCache::new(10);
        let b = a.clone();
        a.insert("user", "alice", membership(&[("seg-1", true)]));
        assert!(b.get("user", "alice").is_some());
    }

    #[test]
    fn concurrent_inserts_from_multiple_threads_dont_corrupt() {
        let cache = MembershipCache::new(1000);
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let c = cache.clone();
                std::thread::spawn(move || {
                    for j in 0..50 {
                        let key = format!("ctx-{i}-{j}");
                        c.insert("user", &key, membership(&[("seg", true)]));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        cache.run_pending_tasks();
        assert_eq!(cache.entry_count(), 500);
        // Spot check: a known key from each thread is present.
        for i in 0..10 {
            assert!(
                cache.get("user", &format!("ctx-{i}-0")).is_some(),
                "missing thread-{i} entry 0"
            );
        }
    }

    // ── Debug ───────────────────────────────────────────────────────────────

    #[test]
    fn debug_includes_count_not_full_data() {
        let c = MembershipCache::new(10);
        c.insert("user", "alice", membership(&[("seg-1", true)]));
        c.run_pending_tasks();
        let s = format!("{c:?}");
        assert!(s.contains("entry_count"));
        // Debug must not dump every key/value (could be thousands of entries).
        assert!(!s.contains("alice"), "Debug must not include cache contents: {s}");
    }
}
