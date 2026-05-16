//! Background LRU-refresh task — keeps list-segment membership fresh for
//! contexts currently resident in the [`crate::MembershipCache`].
//!
//! See `sdks/spec/docs/04-polling.md` "Loop 2 — LRU Refresh" + `03-caching.md`
//! "Refresh (Background Polling)" for the canonical behaviour.
//!
//! ## Responsibilities
//!
//! 1. Wake every `list_segment_refresh_interval` (default 60 s).
//! 2. Snapshot the LRU's resident keys + the snapshot's referenced list-
//!    segment ids.
//! 3. If either is empty: skip — no network call.
//! 4. Otherwise: call [`MembershipBatchFetcher::fetch`] with one query per
//!    resident context, all carrying the same `segment_ids` filter.
//! 5. Update each LRU entry in place from the response.
//! 6. On error: log + back off (1×, 2×, 4×, capped at 5×). Existing LRU
//!    entries are NOT invalidated — the last-known membership continues
//!    to serve `evaluate()`.
//!
//! ## Filtering — referenced segments only
//!
//! The spec mandates filtering to "segments referenced by at least one flag
//! rule" to avoid polling unused segments. For now the filter is "all
//! list-segments in the snapshot" — walking flag-rule condition trees to
//! collect `InSegment(segment_id)` references is wired in Phase 5 Task 8
//! (the evaluate path already needs this logic). The filter narrows but
//! does not affect correctness — broader polling just wastes bandwidth.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::error::SdkError;
use crate::lru::{ContextKey, MembershipCache, MembershipMap};
use crate::polling::backoff_multiplier;
use crate::snapshot::DefinitionStore;

/// Pluggable batch-membership transport. Phase 5 Task 7 wires the real HTTP-
/// backed implementation; tests use a recording stub.
#[async_trait]
pub trait MembershipBatchFetcher: Send + Sync + 'static {
    /// Fetch membership for each context (in the same order as the input)
    /// across the supplied segment_ids. The returned `Vec` MUST have the
    /// same length as `contexts` — one [`MembershipMap`] per context.
    async fn fetch(
        &self,
        contexts: Vec<ContextKey>,
        segment_ids: Vec<String>,
    ) -> Result<Vec<MembershipMap>, SdkError>;
}

/// Background-task handle. Drop or call `shutdown()` to stop.
pub struct RefreshTask {
    handle: JoinHandle<()>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl RefreshTask {
    /// Spawn the refresh task.
    ///
    /// `interval` is the base period between refreshes; failure backoff
    /// extends it up to 5×.
    pub fn spawn(
        fetcher: Arc<dyn MembershipBatchFetcher>,
        cache: MembershipCache,
        store: DefinitionStore,
        interval: Duration,
    ) -> Self {
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            let mut consecutive_failures: u32 = 0;
            loop {
                let wait = interval.saturating_mul(backoff_multiplier(consecutive_failures));
                tokio::select! {
                    biased;
                    _ = &mut shutdown_rx => return,
                    _ = tokio::time::sleep(wait) => {}
                }

                let contexts = cache.keys();
                let segment_ids: Vec<String> = store
                    .load()
                    .list_segment_ids()
                    .map(str::to_string)
                    .collect();

                if contexts.is_empty() || segment_ids.is_empty() {
                    tracing::trace!(
                        contexts = contexts.len(),
                        segments = segment_ids.len(),
                        "LRU refresh skipped (nothing to refresh)"
                    );
                    continue;
                }

                match fetcher.fetch(contexts.clone(), segment_ids).await {
                    Ok(results) => {
                        if results.len() != contexts.len() {
                            tracing::warn!(
                                expected = contexts.len(),
                                got = results.len(),
                                "LRU refresh: backend returned wrong number of results; discarding"
                            );
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            continue;
                        }
                        for ((ctx_type, ctx_key), memberships) in contexts.into_iter().zip(results) {
                            cache.insert(&ctx_type, &ctx_key, memberships);
                        }
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        if consecutive_failures == 1 {
                            tracing::info!(error = %e, "LRU refresh failed (1/N)");
                        } else {
                            tracing::warn!(
                                error = %e,
                                consecutive_failures,
                                next_wait_multiplier = backoff_multiplier(consecutive_failures),
                                "LRU refresh still failing; backing off (existing entries continue to serve)"
                            );
                        }
                    }
                }
            }
        });

        Self { handle, shutdown_tx: Some(shutdown_tx) }
    }

    /// Signal the task to stop and await its exit. Idempotent.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let _ = self.handle.await;
    }

    /// Whether the task has finished (used by tests).
    #[cfg(test)]
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use stitchd_proto::sdk::v1::SyncDefinitionsResponse;
    use stitchd_proto::segments::v1::ListSegmentMeta;

    use crate::snapshot::DefinitionSnapshot;

    fn list_seg(id: &str, key: &str) -> ListSegmentMeta {
        ListSegmentMeta {
            id: id.to_string(),
            key: key.to_string(),
            context_type: "user".to_string(),
        }
    }

    fn snapshot_with_list_segments(ids: &[&str]) -> DefinitionSnapshot {
        DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: vec![],
            rule_segments: vec![],
            list_segments: ids.iter().map(|id| list_seg(id, "seg")).collect(),
            server_timestamp_ms: 0,
            environment_id: "env".into(),
        })
    }

    fn membership(pairs: &[(&str, bool)]) -> MembershipMap {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    /// Stub that records the most-recent fetch() call and returns programmed
    /// per-context results.
    struct StubFetcher {
        calls: AtomicUsize,
        responses: Mutex<Vec<Result<Vec<MembershipMap>, SdkError>>>,
        last_contexts: Mutex<Option<Vec<ContextKey>>>,
        last_segment_ids: Mutex<Option<Vec<String>>>,
    }

    impl StubFetcher {
        fn new(responses: Vec<Result<Vec<MembershipMap>, SdkError>>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                responses: Mutex::new(responses),
                last_contexts: Mutex::new(None),
                last_segment_ids: Mutex::new(None),
            })
        }
        fn call_count(self: &Arc<Self>) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
        fn last_contexts(self: &Arc<Self>) -> Option<Vec<ContextKey>> {
            self.last_contexts.lock().unwrap().clone()
        }
        fn last_segment_ids(self: &Arc<Self>) -> Option<Vec<String>> {
            self.last_segment_ids.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl MembershipBatchFetcher for StubFetcher {
        async fn fetch(
            &self,
            contexts: Vec<ContextKey>,
            segment_ids: Vec<String>,
        ) -> Result<Vec<MembershipMap>, SdkError> {
            *self.last_contexts.lock().unwrap() = Some(contexts);
            *self.last_segment_ids.lock().unwrap() = Some(segment_ids);
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let mut responses = self.responses.lock().unwrap();
            let idx = n.min(responses.len().saturating_sub(1));
            std::mem::replace(
                &mut responses[idx],
                Err(SdkError::Network("consumed".into())),
            )
        }
    }

    // ── Skip-when-nothing-to-refresh paths ──────────────────────────────────

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn skips_refresh_when_lru_is_empty() {
        let cache = MembershipCache::new(10);
        let store = DefinitionStore::from_snapshot(snapshot_with_list_segments(&["seg-1"]));
        let fetcher = StubFetcher::new(vec![Ok(vec![])]);

        let task = RefreshTask::spawn(
            fetcher.clone(),
            cache.clone(),
            store,
            Duration::from_millis(100),
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
        task.shutdown().await;

        // Empty LRU → no fetch call.
        assert_eq!(fetcher.call_count(), 0);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn skips_refresh_when_no_list_segments_in_snapshot() {
        let cache = MembershipCache::new(10);
        cache.insert("user", "alice", membership(&[("seg-x", true)]));
        let store = DefinitionStore::from_snapshot(snapshot_with_list_segments(&[]));
        let fetcher = StubFetcher::new(vec![Ok(vec![])]);

        let task = RefreshTask::spawn(
            fetcher.clone(),
            cache.clone(),
            store,
            Duration::from_millis(100),
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
        task.shutdown().await;

        assert_eq!(fetcher.call_count(), 0);
    }

    // ── Happy-path refresh ──────────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn refresh_updates_resident_entries_from_backend() {
        let cache = MembershipCache::new(10);
        cache.insert("user", "alice", membership(&[("seg-1", false)]));
        cache.insert("user", "bob", membership(&[("seg-1", false)]));
        cache.run_pending_tasks();

        let store = DefinitionStore::from_snapshot(snapshot_with_list_segments(&["seg-1"]));

        // Backend's response: alice → true, bob → false.
        let response = vec![
            membership(&[("seg-1", true)]),
            membership(&[("seg-1", false)]),
        ];
        let fetcher = StubFetcher::new(vec![Ok(response)]);

        let task = RefreshTask::spawn(
            fetcher.clone(),
            cache.clone(),
            store,
            Duration::from_millis(50),
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        task.shutdown().await;

        // The fetcher was called with both contexts.
        assert!(fetcher.call_count() >= 1);
        let last_contexts = fetcher.last_contexts().unwrap();
        let last_segs = fetcher.last_segment_ids().unwrap();
        assert_eq!(last_contexts.len(), 2);
        assert_eq!(last_segs, vec!["seg-1"]);

        // Cache was updated based on the order contexts went in.
        // Order returned by cache.keys() is HashMap-order (non-deterministic),
        // but each context's membership matches the response position. We
        // verify both possible orderings give a valid update.
        let alice_mem = cache.get("user", "alice").unwrap();
        let bob_mem = cache.get("user", "bob").unwrap();
        // At least one of them should be `true` (the response had one true).
        let alice_v = alice_mem.get("seg-1").copied().unwrap_or(false);
        let bob_v = bob_mem.get("seg-1").copied().unwrap_or(false);
        assert!(alice_v || bob_v, "at least one context should show true after refresh");
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn malformed_response_length_does_not_corrupt_cache() {
        let cache = MembershipCache::new(10);
        cache.insert("user", "alice", membership(&[("seg-1", false)]));
        cache.insert("user", "bob", membership(&[("seg-1", false)]));
        cache.run_pending_tasks();

        let store = DefinitionStore::from_snapshot(snapshot_with_list_segments(&["seg-1"]));

        // Backend returns only 1 result for 2 contexts — protocol bug.
        let bad_response = vec![membership(&[("seg-1", true)])];
        let fetcher = StubFetcher::new(vec![Ok(bad_response)]);

        let task = RefreshTask::spawn(
            fetcher,
            cache.clone(),
            store,
            Duration::from_millis(50),
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        task.shutdown().await;

        // Cache unchanged — false stays false for both.
        assert_eq!(
            cache.get("user", "alice").unwrap().get("seg-1").copied(),
            Some(false)
        );
        assert_eq!(
            cache.get("user", "bob").unwrap().get("seg-1").copied(),
            Some(false)
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn failure_does_not_invalidate_existing_entries() {
        let cache = MembershipCache::new(10);
        cache.insert("user", "alice", membership(&[("seg-1", true)]));
        cache.run_pending_tasks();

        let store = DefinitionStore::from_snapshot(snapshot_with_list_segments(&["seg-1"]));
        let fetcher = StubFetcher::new(vec![Err(SdkError::Network("boom".into()))]);

        let task = RefreshTask::spawn(
            fetcher,
            cache.clone(),
            store,
            Duration::from_millis(50),
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        task.shutdown().await;

        // alice's `seg-1=true` membership preserved through the failed refresh.
        assert_eq!(
            cache.get("user", "alice").unwrap().get("seg-1").copied(),
            Some(true)
        );
    }

    // ── Shutdown ────────────────────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shutdown_stops_cleanly() {
        let cache = MembershipCache::new(10);
        let store = DefinitionStore::new();
        let fetcher = StubFetcher::new(vec![Ok(vec![])]);
        let task = RefreshTask::spawn(fetcher, cache, store, Duration::from_secs(60));
        task.shutdown().await;
    }
}
