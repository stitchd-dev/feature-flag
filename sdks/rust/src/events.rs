//! Flag-evaluation event queue + background flush task.
//!
//! See `sdks/spec/docs/05-events.md` for the canonical behaviour.
//!
//! ## Pipeline
//!
//! ```text
//!  evaluate()  ──send──▶  EventQueue (bounded VecDeque)
//!                              │
//!                              │  every flush_interval OR when len >= batch_size:
//!                              ▼
//!                         FlushTask drains up to batch_size events
//!                              │
//!                              ▼
//!                         EventSink::flush(batch) ──▶ gateway REST
//! ```
//!
//! ## Invariants
//!
//! - **`send()` never blocks** the caller. `evaluate()` is on the hot path
//!   and must return promptly. If the queue is at `capacity`, the OLDEST
//!   event is dropped (per spec — back-pressure on the producer is not
//!   acceptable).
//! - **Flush triggers** are interval-based OR size-based, whichever fires
//!   first. Hitting `batch_size` immediately wakes the flush task via
//!   `Notify`; otherwise it wakes on the interval tick.
//! - **Shutdown drains.** `FlushTask::shutdown()` sends the stop signal and
//!   then awaits one final drain so no events are lost on a clean exit.
//! - **At-least-once delivery.** A failed sink flush re-enqueues the batch
//!   at the head; the producer keeps appending to the tail. Worst case is
//!   duplicate delivery; never silent drop on transient failure.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

use crate::error::SdkError;

// ============================================================================
// Event payload
// ============================================================================

/// SDK-internal event payload. Conversion to wire-format (proto or JSON)
/// happens at the [`EventSink`] boundary so the queue can stay
/// transport-agnostic.
///
/// Field set matches `sdks/spec/schemas/flag_evaluation_event.schema.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagEvaluationEvent {
    pub flag_key: String,
    /// UUID string. Empty for `outcome = "flag_not_found"`.
    pub flag_id: String,
    pub variant_key: String,
    pub context_type: String,
    pub context_key: String,
    /// RFC3339 UTC, millisecond precision.
    pub evaluated_at: String,
    /// UUID string of the matched rule, or empty for non-matched outcomes.
    pub matched_rule_id: String,
    /// One of: "matched", "default_rule", "disabled", "flag_not_found".
    pub outcome: String,
    pub reasoning_included: bool,
    /// Already-redacted context parameters (private_parameters removed by
    /// the caller before this event was constructed).
    pub context_parameters: std::collections::HashMap<String, ParameterValue>,
}

/// Typed parameter value mirroring `stitchd.common.v1.ParameterValue` proto.
/// We carry our own enum (rather than the proto type) so the events module
/// is decoupled from `stitchd-proto` and can be unit-tested without the
/// proto crate's generated code.
#[derive(Debug, Clone, PartialEq)]
pub enum ParameterValue {
    Bool(bool),
    Int(i64),
    Double(f64),
    String(String),
    Semver(String),
}

// ============================================================================
// EventSink trait
// ============================================================================

/// Pluggable transport for flushed event batches. Implementations:
///
/// - Phase 5 Task 7: real HTTP-backed sink that POSTs to
///   `POST /v1/sdk/events:batch` on the gateway.
/// - Tests: a recording sink that captures every batch for inspection.
#[async_trait]
pub trait EventSink: Send + Sync + 'static {
    /// Deliver a batch of events. On error the [`FlushTask`] re-enqueues
    /// the batch at the head of the queue (at-least-once delivery).
    async fn flush(&self, batch: Vec<FlagEvaluationEvent>) -> Result<(), SdkError>;
}

// ============================================================================
// EventQueue
// ============================================================================

/// Bounded in-memory queue. `Clone`-cheap (wraps an `Arc`).
#[derive(Clone)]
pub struct EventQueue {
    inner: Arc<EventQueueInner>,
}

struct EventQueueInner {
    buffer: Mutex<VecDeque<FlagEvaluationEvent>>,
    /// Notified when `len() >= batch_size`. Watched by the flush task to
    /// fire a size-triggered flush ahead of the interval tick.
    batch_ready: Notify,
    capacity: usize,
    batch_size: usize,
}

impl EventQueue {
    /// Construct a new queue with the given capacity and batch-size
    /// notification threshold. `capacity` MUST be ≥ `batch_size`
    /// (validated by `SdkConfig::validate`).
    #[must_use]
    pub fn new(capacity: usize, batch_size: usize) -> Self {
        Self {
            inner: Arc::new(EventQueueInner {
                buffer: Mutex::new(VecDeque::with_capacity(capacity.min(4096))),
                batch_ready: Notify::new(),
                capacity,
                batch_size,
            }),
        }
    }

    /// Enqueue an event. Sync — never blocks. If the queue is at capacity,
    /// the oldest event is dropped before the new one is appended.
    /// Notifies the flush task if the post-push length is ≥ `batch_size`.
    ///
    /// Returns `true` if an old event was dropped to make room, `false`
    /// otherwise. (Used by tests + diagnostics; the caller never blocks.)
    pub fn send(&self, event: FlagEvaluationEvent) -> bool {
        let mut buf = self.inner.buffer.lock().expect("event queue mutex poisoned");
        let dropped = if buf.len() >= self.inner.capacity {
            buf.pop_front();
            true
        } else {
            false
        };
        buf.push_back(event);
        let len = buf.len();
        drop(buf);
        if len >= self.inner.batch_size {
            self.inner.batch_ready.notify_one();
        }
        dropped
    }

    /// Drain up to `n` events in FIFO order.
    #[must_use]
    pub fn drain_up_to(&self, n: usize) -> Vec<FlagEvaluationEvent> {
        let mut buf = self.inner.buffer.lock().expect("event queue mutex poisoned");
        let take = n.min(buf.len());
        buf.drain(..take).collect()
    }

    /// Drain ALL events. Used by the flush task on shutdown.
    #[must_use]
    pub fn drain_all(&self) -> Vec<FlagEvaluationEvent> {
        let mut buf = self.inner.buffer.lock().expect("event queue mutex poisoned");
        buf.drain(..).collect()
    }

    /// Re-enqueue a failed batch at the head of the buffer (at-least-once
    /// delivery on transient sink errors). If re-enqueuing would exceed
    /// capacity, the OLDEST already-buffered events are dropped — failed
    /// flushes take priority over fresh events, but the cap is absolute.
    pub fn requeue_at_head(&self, batch: Vec<FlagEvaluationEvent>) {
        let mut buf = self.inner.buffer.lock().expect("event queue mutex poisoned");
        // Make room: drop oldest tail entries until (existing - dropped) + batch.len() <= capacity.
        let needed = batch.len();
        let max_existing = self.inner.capacity.saturating_sub(needed);
        while buf.len() > max_existing {
            buf.pop_back();
        }
        // Push the batch back to the front in original order.
        for ev in batch.into_iter().rev() {
            buf.push_front(ev);
        }
    }

    /// Current queue length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.buffer.lock().expect("event queue mutex poisoned").len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

}

impl std::fmt::Debug for EventQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventQueue")
            .field("len", &self.len())
            .field("capacity", &self.inner.capacity)
            .field("batch_size", &self.inner.batch_size)
            .finish()
    }
}

// ============================================================================
// FlushTask
// ============================================================================

/// Handle to the background flush task. Owns the shutdown signal + the
/// spawned tokio JoinHandle.
pub struct FlushTask {
    handle: JoinHandle<()>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl FlushTask {
    /// Spawn the flush task.
    ///
    /// The task loops:
    /// 1. Wait on either the interval tick OR the queue's `batch_ready` signal.
    /// 2. Drain up to `batch_size` events.
    /// 3. If non-empty, call `sink.flush(batch)`. On error, re-enqueue the
    ///    batch at the head and continue.
    ///
    /// On shutdown signal, the task does ONE final drain of EVERYTHING in
    /// the queue (not just `batch_size`) before exiting.
    pub fn spawn(
        queue: EventQueue,
        sink: Arc<dyn EventSink>,
        flush_interval: Duration,
    ) -> Self {
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let batch_size = queue.inner.batch_size;
        let batch_ready = Arc::clone(&queue.inner);

        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(flush_interval);
            // First tick fires immediately; skip it so we don't flush on
            // startup with an empty queue.
            ticker.tick().await;
            loop {
                tokio::select! {
                    biased;
                    _ = &mut shutdown_rx => {
                        // Final drain — get EVERYTHING, not just batch_size.
                        let final_batch = queue.drain_all();
                        if !final_batch.is_empty() {
                            // Best-effort: log+swallow errors on shutdown.
                            if let Err(e) = sink.flush(final_batch).await {
                                tracing::warn!(error = %e, "shutdown flush failed");
                            }
                        }
                        return;
                    }
                    _ = batch_ready.batch_ready.notified() => {
                        Self::flush_batch(&queue, sink.as_ref(), batch_size).await;
                    }
                    _ = ticker.tick() => {
                        Self::flush_batch(&queue, sink.as_ref(), batch_size).await;
                    }
                }
            }
        });

        Self {
            handle,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Single-flush helper. Drains up to `batch_size` events and calls
    /// `sink.flush`; on error re-enqueues at the head.
    async fn flush_batch(queue: &EventQueue, sink: &dyn EventSink, batch_size: usize) {
        let batch = queue.drain_up_to(batch_size);
        if batch.is_empty() {
            return;
        }
        if let Err(e) = sink.flush(batch.clone()).await {
            tracing::warn!(
                error = %e,
                count = batch.len(),
                "event sink flush failed; re-enqueuing batch at head"
            );
            queue.requeue_at_head(batch);
        }
    }

    /// Signal the task to stop. Awaits the final drain + task exit.
    /// Idempotent: subsequent calls are no-ops.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            // If the receiver is already gone (task panicked), this returns
            // Err; we don't care — the await on the handle still completes.
            let _ = tx.send(());
        }
        // Even on shutdown_tx drop, the handle will resolve.
        let _ = self.handle.await;
    }

    /// Whether the task has completed (used by tests for assertions).
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn ev(flag: &str) -> FlagEvaluationEvent {
        FlagEvaluationEvent {
            flag_key: flag.into(),
            flag_id: "00000000-0000-0000-0000-000000000001".into(),
            variant_key: "v".into(),
            context_type: "user".into(),
            context_key: "alice".into(),
            evaluated_at: "2026-05-16T00:00:00.000Z".into(),
            matched_rule_id: String::new(),
            outcome: "matched".into(),
            reasoning_included: false,
            context_parameters: Default::default(),
        }
    }

    // ── EventQueue: basic ───────────────────────────────────────────────────

    #[test]
    fn new_queue_is_empty() {
        let q = EventQueue::new(10, 5);
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn send_appends_in_fifo_order() {
        let q = EventQueue::new(10, 5);
        q.send(ev("a"));
        q.send(ev("b"));
        q.send(ev("c"));
        let drained = q.drain_all();
        let keys: Vec<&str> = drained.iter().map(|e| e.flag_key.as_str()).collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn send_drops_oldest_on_capacity_overflow() {
        let q = EventQueue::new(2, 1);
        assert_eq!(q.send(ev("a")), false);
        assert_eq!(q.send(ev("b")), false);
        assert_eq!(q.send(ev("c")), true, "should drop oldest");
        let drained = q.drain_all();
        let keys: Vec<&str> = drained.iter().map(|e| e.flag_key.as_str()).collect();
        // "a" dropped to make room for "c"; queue holds [b, c].
        assert_eq!(keys, vec!["b", "c"]);
    }

    #[test]
    fn drain_up_to_caps_at_n() {
        let q = EventQueue::new(10, 5);
        for k in &["a", "b", "c", "d", "e"] {
            q.send(ev(k));
        }
        let first = q.drain_up_to(2);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].flag_key, "a");
        assert_eq!(first[1].flag_key, "b");
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn drain_up_to_handles_n_greater_than_len() {
        let q = EventQueue::new(10, 5);
        q.send(ev("a"));
        let drained = q.drain_up_to(100);
        assert_eq!(drained.len(), 1);
        assert!(q.is_empty());
    }

    #[test]
    fn requeue_at_head_preserves_order() {
        let q = EventQueue::new(10, 5);
        q.send(ev("c"));
        q.send(ev("d"));
        // Requeue [a, b] at head → queue becomes [a, b, c, d].
        q.requeue_at_head(vec![ev("a"), ev("b")]);
        let drained = q.drain_all();
        let keys: Vec<&str> = drained.iter().map(|e| e.flag_key.as_str()).collect();
        assert_eq!(keys, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn requeue_at_head_caps_at_capacity_dropping_tail() {
        // Capacity 3; existing [x, y, z]; requeue [a, b] at head.
        // Final queue must be size 3 with [a, b, x] — failed-flush takes
        // priority over fresh events but capacity is absolute.
        let q = EventQueue::new(3, 1);
        q.send(ev("x"));
        q.send(ev("y"));
        q.send(ev("z"));
        q.requeue_at_head(vec![ev("a"), ev("b")]);
        let keys: Vec<String> = q.drain_all().into_iter().map(|e| e.flag_key).collect();
        assert_eq!(keys, vec!["a", "b", "x"]);
    }

    // ── EventQueue: clone ───────────────────────────────────────────────────

    #[test]
    fn clones_share_underlying_state() {
        let a = EventQueue::new(10, 5);
        let b = a.clone();
        a.send(ev("via-a"));
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn queue_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EventQueue>();
        assert_send_sync::<FlagEvaluationEvent>();
    }

    // ── FlushTask: interval + shutdown drain ────────────────────────────────

    /// Recording sink — captures every batch in an Arc<Mutex<Vec<_>>>.
    struct RecordingSink {
        batches: Arc<Mutex<Vec<Vec<FlagEvaluationEvent>>>>,
        fail_n_times: AtomicUsize,
    }

    impl RecordingSink {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                batches: Arc::new(Mutex::new(Vec::new())),
                fail_n_times: AtomicUsize::new(0),
            })
        }
        fn fail_next(self: &Arc<Self>, n: usize) {
            self.fail_n_times.store(n, Ordering::SeqCst);
        }
        fn batches(self: &Arc<Self>) -> Vec<Vec<FlagEvaluationEvent>> {
            self.batches.lock().unwrap().clone()
        }
        fn flat_keys(self: &Arc<Self>) -> Vec<String> {
            self.batches()
                .into_iter()
                .flat_map(|b| b.into_iter().map(|e| e.flag_key))
                .collect()
        }
    }

    #[async_trait]
    impl EventSink for RecordingSink {
        async fn flush(&self, batch: Vec<FlagEvaluationEvent>) -> Result<(), SdkError> {
            if self.fail_n_times.load(Ordering::SeqCst) > 0 {
                self.fail_n_times.fetch_sub(1, Ordering::SeqCst);
                return Err(SdkError::Network("simulated transient failure".into()));
            }
            self.batches.lock().unwrap().push(batch);
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn flush_task_fires_on_interval() {
        let q = EventQueue::new(100, 50);
        let sink = RecordingSink::new();
        let task = FlushTask::spawn(q.clone(), sink.clone(), Duration::from_millis(100));

        q.send(ev("a"));
        q.send(ev("b"));
        // Advance time past one interval (we skipped the first immediate tick).
        tokio::time::advance(Duration::from_millis(150)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        task.shutdown().await;
        // Could land either in the interval-triggered batch or the
        // shutdown-final-drain — either way both events end up at the sink.
        let keys = sink.flat_keys();
        assert!(keys.contains(&"a".to_string()));
        assert!(keys.contains(&"b".to_string()));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn flush_task_fires_on_batch_size_threshold() {
        let q = EventQueue::new(100, 3);
        let sink = RecordingSink::new();
        let task = FlushTask::spawn(q.clone(), sink.clone(), Duration::from_secs(60));

        // Fill exactly batch_size events → triggers notify (no interval wait).
        q.send(ev("1"));
        q.send(ev("2"));
        q.send(ev("3"));
        // Let the flush task wake on the Notify.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;

        task.shutdown().await;
        assert!(!sink.batches().is_empty(), "sink should have received at least one batch");
        let keys = sink.flat_keys();
        assert_eq!(keys, vec!["1", "2", "3"]);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn flush_task_caps_batch_at_batch_size() {
        let q = EventQueue::new(100, 5);
        let sink = RecordingSink::new();
        let task = FlushTask::spawn(q.clone(), sink.clone(), Duration::from_millis(10));

        // Fill 11 events. Each batch flush should pull at most 5.
        for i in 0..11 {
            q.send(ev(&format!("e{i}")));
        }
        // Run several intervals worth of time.
        for _ in 0..5 {
            tokio::time::advance(Duration::from_millis(15)).await;
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
        }

        task.shutdown().await;
        let batches = sink.batches();
        for b in &batches {
            assert!(
                b.len() <= 5,
                "every batch must respect batch_size; got {}",
                b.len()
            );
        }
        // All 11 events delivered exactly once across the batches.
        assert_eq!(sink.flat_keys().len(), 11);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shutdown_drains_remaining_events() {
        let q = EventQueue::new(100, 50);
        let sink = RecordingSink::new();
        let task = FlushTask::spawn(q.clone(), sink.clone(), Duration::from_secs(60));

        // 3 events << batch_size 50 — won't trigger size flush.
        q.send(ev("a"));
        q.send(ev("b"));
        q.send(ev("c"));
        assert_eq!(q.len(), 3);

        // Shutdown — the task's final-drain block must flush these.
        task.shutdown().await;
        assert!(q.is_empty(), "queue empty after shutdown");
        let keys = sink.flat_keys();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn failed_flush_requeues_at_head_for_retry() {
        let q = EventQueue::new(100, 2);
        let sink = RecordingSink::new();
        // Fail the FIRST flush, succeed thereafter.
        sink.fail_next(1);
        let task = FlushTask::spawn(q.clone(), sink.clone(), Duration::from_millis(10));

        q.send(ev("a"));
        q.send(ev("b"));
        // First tick triggers flush → fails → re-enqueue [a, b].
        tokio::time::advance(Duration::from_millis(15)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        // Second tick triggers flush → succeeds.
        tokio::time::advance(Duration::from_millis(15)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        task.shutdown().await;
        let keys = sink.flat_keys();
        assert!(keys.contains(&"a".to_string()));
        assert!(keys.contains(&"b".to_string()));
    }

    // ── Debug ───────────────────────────────────────────────────────────────

    #[test]
    fn queue_debug_includes_len_and_caps() {
        let q = EventQueue::new(50, 10);
        q.send(ev("a"));
        let s = format!("{q:?}");
        assert!(s.contains("len: 1"));
        assert!(s.contains("capacity: 50"));
        assert!(s.contains("batch_size: 10"));
    }
}
