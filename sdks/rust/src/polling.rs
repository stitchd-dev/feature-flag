//! Background definition-sync polling task.
//!
//! See `sdks/spec/docs/04-polling.md` "Loop 1 — Definition Sync" for the
//! canonical behaviour.
//!
//! ## Responsibilities
//!
//! - Wake every `definition_poll_interval` (or sooner on backoff overrun).
//! - Call [`DefinitionFetcher::fetch`] to retrieve a fresh snapshot.
//! - On success: atomically swap the snapshot in `DefinitionStore`; reset
//!   the failure counter; carry on at the configured interval.
//! - On transient failure: log; back off exponentially (1×, 2×, 4×, capped
//!   at 5× the interval); retry on the next wakeup. Never give up — the
//!   application continues serving evaluations from the last-known
//!   snapshot.
//! - On shutdown signal: stop cleanly without producing a partial swap.
//!
//! ## Why a [`DefinitionFetcher`] trait?
//!
//! Decouples the task from the gRPC transport so unit tests can verify the
//! loop semantics (interval timing, backoff, snapshot swapping) without
//! spinning up a real tonic server. Phase 5 Task 7 plugs in the real
//! tonic-based fetcher that calls `SdkService::SyncDefinitions`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::error::SdkError;
use crate::snapshot::{DefinitionSnapshot, DefinitionStore};

/// Pluggable source of fresh [`DefinitionSnapshot`]s. Implementations:
///
/// - Phase 5 Task 7: real fetcher wrapping a tonic gRPC client to
///   `SdkService::SyncDefinitions`.
/// - Tests: stub fetchers that yield programmed snapshots / errors.
#[async_trait]
pub trait DefinitionFetcher: Send + Sync + 'static {
    /// Perform one `SyncDefinitions` call. Errors are treated as transient
    /// — the polling task logs + backs off but never propagates them to
    /// the application (per spec, evaluations keep serving the last-known
    /// snapshot).
    async fn fetch(&self) -> Result<DefinitionSnapshot, SdkError>;
}

/// Handle to the spawned polling task. Drop, or call `shutdown()`, to stop.
pub struct PollTask {
    handle: JoinHandle<()>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl PollTask {
    /// Spawn the polling task.
    ///
    /// Note: the FIRST sync is the responsibility of `SdkClient::init`
    /// (Phase 5 Task 7) which calls `fetcher.fetch()` directly and only
    /// spawns this task if the first call succeeded. This task picks up
    /// from there with periodic refreshes.
    pub fn spawn(
        fetcher: Arc<dyn DefinitionFetcher>,
        store: DefinitionStore,
        interval: Duration,
    ) -> Self {
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        let handle = tokio::spawn(async move {
            let mut consecutive_failures: u32 = 0;
            // `SdkClient::init` has already done the first sync
            // synchronously and stored a snapshot. We use `tokio::time::sleep`
            // in the loop (rather than `tokio::time::interval`) for
            // deterministic behaviour under `tokio::test(start_paused = true)`
            // and explicit control over backoff wait.
            loop {
                let wait = interval.saturating_mul(backoff_multiplier(consecutive_failures));
                tokio::select! {
                    biased;
                    _ = &mut shutdown_rx => return,
                    _ = tokio::time::sleep(wait) => {}
                }

                match fetcher.fetch().await {
                    Ok(snapshot) => {
                        let was_empty = snapshot.flag_count() == 0
                            && snapshot.rule_segment_count() == 0
                            && snapshot.list_segment_count() == 0;
                        store.store(snapshot);
                        consecutive_failures = 0;
                        if was_empty {
                            tracing::debug!(
                                "SyncDefinitions returned empty snapshot (legal — env may have no flags configured)"
                            );
                        }
                    }
                    Err(e) => {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        if consecutive_failures == 1 {
                            tracing::info!(error = %e, "definition sync failed (1/N)");
                        } else {
                            tracing::warn!(
                                error = %e,
                                consecutive_failures,
                                next_wait_multiplier = backoff_multiplier(consecutive_failures),
                                "definition sync still failing; backing off"
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

/// Wait multiplier for the Nth consecutive failure.
///
/// Spec table (`04-polling.md`):
/// | # failures | wait |
/// |---|---|
/// | 1 | 1 × interval |
/// | 2 | 2 × interval |
/// | 3 | 4 × interval |
/// | 4 | 5 × interval (capped) |
/// | 5+ | 5 × interval |
#[must_use]
pub const fn backoff_multiplier(consecutive_failures: u32) -> u32 {
    match consecutive_failures {
        0 | 1 => 1,
        2 => 2,
        3 => 4,
        _ => 5,
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

    use stitchd_proto::flags::v1::FeatureFlag;
    use stitchd_proto::sdk::v1::SyncDefinitionsResponse;

    fn flag(key: &str) -> FeatureFlag {
        FeatureFlag {
            key: key.to_string(),
            ..Default::default()
        }
    }

    fn snapshot_with_keys(keys: &[&str]) -> DefinitionSnapshot {
        DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags: keys.iter().map(|k| flag(k)).collect(),
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: "env".into(),
        })
    }

    /// Recording fetcher — programs a sequence of fetch outcomes.
    /// Returns the n-th programmed outcome on the n-th call.
    struct StubFetcher {
        calls: AtomicUsize,
        outcomes: Mutex<Vec<Result<DefinitionSnapshot, SdkError>>>,
    }

    impl StubFetcher {
        fn new(outcomes: Vec<Result<DefinitionSnapshot, SdkError>>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                outcomes: Mutex::new(outcomes),
            })
        }
        fn call_count(self: &Arc<Self>) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl DefinitionFetcher for StubFetcher {
        async fn fetch(&self) -> Result<DefinitionSnapshot, SdkError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let mut outcomes = self.outcomes.lock().unwrap();
            // Return the n-th outcome, OR the last one repeatedly if we've
            // exhausted the programmed sequence.
            let idx = n.min(outcomes.len().saturating_sub(1));
            // Replace with placeholder so we don't move out of the Mutex.
            std::mem::replace(
                &mut outcomes[idx],
                Err(SdkError::Network("already consumed".into())),
            )
        }
    }

    // ── backoff_multiplier table ────────────────────────────────────────────

    #[test]
    fn backoff_table_matches_spec() {
        assert_eq!(backoff_multiplier(0), 1);
        assert_eq!(backoff_multiplier(1), 1);
        assert_eq!(backoff_multiplier(2), 2);
        assert_eq!(backoff_multiplier(3), 4);
        assert_eq!(backoff_multiplier(4), 5);
        assert_eq!(backoff_multiplier(5), 5);
        assert_eq!(backoff_multiplier(100), 5);
    }

    // ── PollTask: success path ──────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn poll_task_fetches_on_interval_and_swaps_snapshot() {
        let store = DefinitionStore::from_snapshot(snapshot_with_keys(&["initial"]));
        let fetcher = StubFetcher::new(vec![Ok(snapshot_with_keys(&["fresh"]))]);

        let task = PollTask::spawn(fetcher.clone(), store.clone(), Duration::from_millis(100));

        // In start_paused mode, sleeping auto-advances the clock AND drives
        // the runtime — much more reliable than advance() + yield_now().
        tokio::time::sleep(Duration::from_millis(200)).await;

        task.shutdown().await;

        let snap = store.load();
        assert!(snap.flag("fresh").is_some(), "snapshot should have been swapped");
        assert!(snap.flag("initial").is_none(), "old snapshot must be gone");
        assert!(fetcher.call_count() >= 1);
    }

    // ── PollTask: empty snapshot accepted ───────────────────────────────────

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn empty_snapshot_is_accepted_not_error() {
        let store = DefinitionStore::from_snapshot(snapshot_with_keys(&["initial"]));
        let fetcher = StubFetcher::new(vec![Ok(snapshot_with_keys(&[]))]);

        let task = PollTask::spawn(fetcher, store.clone(), Duration::from_millis(100));
        tokio::time::sleep(Duration::from_millis(200)).await;
        task.shutdown().await;

        let snap = store.load();
        // Snapshot WAS swapped — empty is a legal state (env with no flags).
        assert_eq!(snap.flag_count(), 0);
        assert!(snap.flag("initial").is_none());
    }

    // ── PollTask: failure → backoff ─────────────────────────────────────────

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn last_known_snapshot_served_when_fetch_fails() {
        let store = DefinitionStore::from_snapshot(snapshot_with_keys(&["initial"]));
        let fetcher = StubFetcher::new(vec![
            Err(SdkError::Network("first failure".into())),
            Err(SdkError::Network("second failure".into())),
        ]);

        let task = PollTask::spawn(fetcher, store.clone(), Duration::from_millis(100));
        tokio::time::sleep(Duration::from_millis(250)).await;
        task.shutdown().await;

        // Original snapshot still serving evaluations.
        let snap = store.load();
        assert!(snap.flag("initial").is_some());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn success_after_failures_resets_counter() {
        // Programmed: fail, fail, succeed, succeed.
        let store = DefinitionStore::from_snapshot(snapshot_with_keys(&["initial"]));
        let fetcher = StubFetcher::new(vec![
            Err(SdkError::Network("fail 1".into())),
            Err(SdkError::Network("fail 2".into())),
            Ok(snapshot_with_keys(&["fresh-1"])),
            Ok(snapshot_with_keys(&["fresh-2"])),
        ]);

        let task = PollTask::spawn(fetcher.clone(), store.clone(), Duration::from_millis(50));

        // Plenty of time for all 4 calls + backoffs.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        task.shutdown().await;

        assert!(fetcher.call_count() >= 4);
        // Final snapshot is the latest success.
        let snap = store.load();
        assert!(snap.flag("fresh-2").is_some() || snap.flag("fresh-1").is_some());
    }

    // ── PollTask: shutdown ──────────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shutdown_stops_the_task_cleanly() {
        let store = DefinitionStore::new();
        let fetcher = StubFetcher::new(vec![Ok(snapshot_with_keys(&[]))]);

        let task = PollTask::spawn(fetcher, store, Duration::from_secs(60));
        // Don't advance time — just shut down immediately.
        task.shutdown().await;
        // shutdown awaits the join handle; if we reach here without hanging,
        // the test passed.
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shutdown_during_backoff_sleep_exits_promptly() {
        // The task is in extra-backoff sleep (longer than the interval).
        // shutdown signal must wake it.
        let store = DefinitionStore::from_snapshot(snapshot_with_keys(&["initial"]));
        let fetcher = StubFetcher::new(vec![Err(SdkError::Network("always fail".into()))]);

        let task = PollTask::spawn(fetcher, store, Duration::from_millis(100));
        // Let several backoff cycles run.
        tokio::time::sleep(Duration::from_millis(900)).await;

        // Shutdown should not hang waiting for the long backoff sleep —
        // the biased select! prioritises the shutdown signal over sleep.
        tokio::time::timeout(Duration::from_secs(5), task.shutdown())
            .await
            .expect("shutdown must not hang during backoff sleep");
    }

    #[test]
    fn poll_task_does_not_panic_on_shutdown_drop() {
        // Drop without await — must not panic. Tokio runtime handles the
        // dangling JoinHandle.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let fetcher = StubFetcher::new(vec![Err(SdkError::Network("e".into()))]);
            let _task = PollTask::spawn(fetcher, DefinitionStore::new(), Duration::from_secs(60));
            // Drop without explicit shutdown — should not panic.
        });
    }
}
