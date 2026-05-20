//! Client-side event buffer for `Client::track()`.
//!
//! See `conductor/tracks/events_metrics_20260519/spec.md` § F2 for the
//! canonical behaviour. Companion to (the future) `Client::track()` —
//! `track()` calls [`EventBuffer::enqueue`], which buffers events and
//! batch-POSTs them to the gateway's `POST /v1/events/track` endpoint.
//!
//! ## Pipeline
//!
//! ```text
//!  Client::track()  ──enqueue──▶  EventBuffer (Mutex<VecDeque>)
//!                                      │
//!                                      │  flush triggers:
//!                                      │    1. size:     len >= flush_at_size
//!                                      │    2. interval: every flush_interval
//!                                      │    3. explicit: EventBuffer::flush()
//!                                      │    4. shutdown: EventBuffer::shutdown()
//!                                      ▼
//!                                   drain ALL events
//!                                      │
//!                                      ▼
//!                            POST /v1/events/track (with retries)
//! ```
//!
//! ## Wire format — must match `crates/stitchd-gateway/src/routes/events.rs`
//!
//! ```json
//! { "events": [
//!     { "event_key": "checkout_completed",
//!       "context_type": "user", "context_key": "alice",
//!       "value": { "int": 1 },              // tag = snake_case enum variant
//!       "properties": { "country": "US" },
//!       "occurred_at": "2026-05-19T12:00:00Z" }
//! ]}
//! ```
//!
//! `TypedValue` uses `#[serde(rename_all = "snake_case")]` to match the
//! gateway's `TypedValue` (which is also `snake_case`). Drift here would
//! cause every event to be rejected with `value_type_mismatch`.
//!
//! ## Invariants
//!
//! - **`enqueue()` is non-blocking.** Lock is held only long enough to
//!   push. If the post-push length crosses `flush_at_size`, we spawn an
//!   immediate-flush task (fire-and-forget); the caller never awaits.
//! - **Flush retries: 3 attempts** with exponential backoff
//!   (`backoff_base`, `2× backoff_base`, `4× backoff_base`). After 3
//!   consecutive 5xx/network failures the batch is dropped and
//!   `tracing::warn!` records the drop count.
//! - **HTTP 202 with per-event rejections is success at the transport
//!   layer.** Each rejected event counts toward `FlushReport.rejected`,
//!   but the batch as a whole is NOT retried.
//! - **HTTP 4xx (other than 401 retry semantics — Phase 5.4 concern) is
//!   permanent.** No retry; events go to `dropped` count.
//! - **Shutdown drains.** `shutdown(timeout)` aborts the interval flusher
//!   then does one final flush bounded by `timeout` (default 5s).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::metrics::{
    record_buffer_size, record_buffered, record_dropped_permanent,
    record_dropped_retry_exhausted, record_dropped_shutdown_timeout, record_flushed_accepted,
    record_flushed_rejected,
};

// ============================================================================
// Configuration
// ============================================================================

/// Configuration knobs for an [`EventBuffer`]. All defaults match spec F2.2.
#[derive(Debug, Clone)]
pub struct EventBufferConfig {
    /// Trigger an immediate flush when the buffer reaches this many events.
    /// Default 100.
    pub flush_at_size: usize,
    /// Background interval between flushes. Default 5s.
    pub flush_interval: Duration,
    /// Maximum retries per flush before dropping the batch. Default 3.
    pub max_retries: u32,
    /// Initial backoff between retries; doubles each retry
    /// (`200ms → 400ms → 800ms` by default).
    pub backoff_base: Duration,
    /// Base URL of the gateway, e.g. `"https://gateway.example.com"`.
    /// The trailing `/v1/events/track` is appended internally.
    pub gateway_base_url: String,
    /// SDK key sent as `x-sdk-key` on every flush.
    pub sdk_key: String,
}

impl EventBufferConfig {
    /// New config with the spec-default flush + retry knobs filled in.
    /// Caller must supply gateway URL and SDK key.
    #[must_use]
    pub fn new(gateway_base_url: impl Into<String>, sdk_key: impl Into<String>) -> Self {
        Self {
            flush_at_size: 100,
            flush_interval: Duration::from_secs(5),
            max_retries: 3,
            backoff_base: Duration::from_millis(200),
            gateway_base_url: gateway_base_url.into(),
            sdk_key: sdk_key.into(),
        }
    }

    /// Compute the wait between the Nth and (N+1)th retry attempt.
    /// `attempt = 0` returns `backoff_base`, `attempt = 1` returns `2 ×
    /// backoff_base`, etc. (200ms, 400ms, 800ms with the default base.)
    #[must_use]
    pub(crate) fn backoff_for(&self, attempt: u32) -> Duration {
        self.backoff_base.saturating_mul(1u32 << attempt.min(16))
    }
}

// ============================================================================
// Wire-format types — MUST match `crates/stitchd-gateway/src/routes/events.rs`
// ============================================================================

/// Typed event value mirroring proto `MetricValue.oneof value`. The
/// discriminator is `snake_case` to byte-match the gateway's `TypedValue`.
///
/// JSON shape:
/// - `{"bool": true}`   → `TypedValue::Bool`
/// - `{"int": 42}`      → `TypedValue::Int`
/// - `{"double": 1.5}`  → `TypedValue::Double`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedValue {
    Bool(bool),
    Int(i64),
    Double(f64),
}

/// One event waiting to be flushed.
#[derive(Debug, Clone)]
pub struct BufferedEvent {
    pub event_key: String,
    pub context_type: String,
    pub context_key: String,
    pub value: Option<TypedValue>,
    pub properties: Option<HashMap<String, String>>,
    pub occurred_at: Option<DateTime<Utc>>,
}

/// Internal wire shape — what we serialize and POST.
///
/// `properties` is `skip_serializing_if = "Option::is_none"` so we send
/// `{}`-clean JSON for events with no metadata, exactly matching the
/// gateway's `Option<HashMap<...>>`.
#[derive(Debug, Serialize)]
struct WireEvent<'a> {
    event_key: &'a str,
    context_type: &'a str,
    context_key: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a TypedValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<&'a HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    occurred_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct WireBatch<'a> {
    events: Vec<WireEvent<'a>>,
}

#[derive(Debug, Deserialize)]
struct WireResponse {
    accepted_count: u32,
    #[serde(default)]
    rejected: Vec<WireRejection>,
}

#[derive(Debug, Deserialize)]
struct WireRejection {
    #[allow(dead_code)] // surfaced via tracing only for now
    event_key: String,
    #[allow(dead_code)]
    reason: String,
}

// ============================================================================
// FlushReport / FlushError
// ============================================================================

/// Summary returned by a single [`EventBuffer::flush`] call.
///
/// - `accepted` — server returned 202 and counted these toward
///   `accepted_count`.
/// - `rejected` — server returned 202 but listed these as per-event
///   rejections (unknown event_key, value_type_mismatch, ...).
/// - `retried` — number of retry attempts performed before success
///   (0 when the first attempt succeeded).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlushReport {
    pub accepted: u32,
    pub rejected: u32,
    pub retried: u32,
}

/// Errors returned by [`EventBuffer::flush`]. Transient failures are
/// retried internally; this variant only surfaces if all retries are
/// exhausted OR the request is fundamentally malformed.
#[derive(Debug)]
pub enum FlushError {
    /// Network / 5xx / timeout. All retries were exhausted.
    /// `count` is the number of events that were dropped.
    Network { count: u32, last_error: String },
    /// HTTP 4xx (excluding 408/429 which we treat as transient). The
    /// batch is dropped; the caller's wire format is wrong.
    Permanent { status: u16, message: String },
}

impl std::fmt::Display for FlushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network { count, last_error } => write!(
                f,
                "flush failed after retries: dropped {count} events ({last_error})"
            ),
            Self::Permanent { status, message } => {
                write!(f, "flush rejected: HTTP {status} — {message}")
            }
        }
    }
}

impl std::error::Error for FlushError {}

// ============================================================================
// EventBuffer
// ============================================================================

/// Per-`Client` async buffer that batches events between flushes.
///
/// Spawns one background tokio task at construction (the interval
/// flusher) and one short-lived task per size-triggered flush (so
/// `enqueue` returns synchronously).
///
/// Cheaply cloneable — all state lives behind `Arc<Mutex<_>>` /
/// `Arc<...>`. Each clone shares the same buffer.
pub struct EventBuffer {
    inner: Arc<EventBufferInner>,
    /// Handle to the interval flusher. `Some` while running; taken on
    /// `shutdown()`.
    flusher_handle: Mutex<Option<JoinHandle<()>>>,
}

struct EventBufferInner {
    /// The actual queue. `Mutex` (sync) is fine — every critical section
    /// is a push/drain, no awaits while holding.
    buffer: std::sync::Mutex<VecDeque<BufferedEvent>>,
    config: EventBufferConfig,
    http_client: reqwest::Client,
    endpoint: String,
}

impl EventBuffer {
    /// Build a new buffer and spawn its interval-flusher task.
    ///
    /// The interval task wakes every `config.flush_interval` and calls
    /// `flush()` if the buffer is non-empty.
    pub fn new(config: EventBufferConfig) -> Arc<Self> {
        let http_client = reqwest::Client::new();
        Self::with_client(config, http_client)
    }

    /// Like [`Self::new`] but accepts a custom reqwest client (for
    /// timeouts, TLS config, or mock injection in tests).
    pub fn with_client(config: EventBufferConfig, http_client: reqwest::Client) -> Arc<Self> {
        let endpoint = format!(
            "{}/v1/events/track",
            config.gateway_base_url.trim_end_matches('/')
        );
        let inner = Arc::new(EventBufferInner {
            buffer: std::sync::Mutex::new(VecDeque::new()),
            config,
            http_client,
            endpoint,
        });

        let me = Arc::new(Self {
            inner: Arc::clone(&inner),
            flusher_handle: Mutex::new(None),
        });

        // Spawn the interval-flusher with a weak ref so the task exits
        // when the last strong handle drops (in case the caller doesn't
        // call `shutdown`).
        let weak = Arc::downgrade(&me);
        let interval = inner.config.flush_interval;
        let handle = tokio::spawn(async move {
            // First tick fires immediately; skip it so we don't flush an
            // empty buffer right after construction.
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let Some(strong) = weak.upgrade() else {
                    // Buffer dropped — exit cleanly.
                    return;
                };
                if !strong.is_empty_inner() {
                    let _ = strong.flush_internal().await;
                }
            }
        });
        // Stash the handle for later abort on `shutdown`.
        if let Ok(mut slot) = me.flusher_handle.try_lock() {
            *slot = Some(handle);
        }
        me
    }

    // ── public surface ──────────────────────────────────────────────────────

    /// Push one event onto the buffer. Non-blocking.
    ///
    /// If the post-push length is `>= flush_at_size`, a size-triggered
    /// flush is spawned (fire-and-forget). The caller does NOT wait on
    /// it — the next size-flush starts immediately even if a previous
    /// flush is still in flight, since both will compete for the inner
    /// mutex but operate on disjoint event sets after the first drain.
    pub fn enqueue(self: &Arc<Self>, event: BufferedEvent) {
        let len_after = {
            let mut buf = self.inner.buffer.lock().expect("event buffer poisoned");
            buf.push_back(event);
            buf.len()
        };
        // Phase 5 Task 5.4 — counter + gauge.
        record_buffered();
        record_buffer_size(len_after);
        if len_after >= self.inner.config.flush_at_size {
            let me = Arc::clone(self);
            tokio::spawn(async move {
                let _ = me.flush_internal().await;
            });
        }
    }

    /// Drain the buffer, POST to the gateway with retries, and report
    /// how many events were accepted / rejected by the server. Returns
    /// `Ok(empty report)` immediately if the buffer is empty.
    pub async fn flush(self: &Arc<Self>) -> Result<FlushReport, FlushError> {
        self.flush_internal().await
    }

    /// Abort the interval flusher and drain remaining events with one
    /// final flush bounded by `timeout`. Events still pending when the
    /// timeout elapses are dropped with a `tracing::warn!`.
    ///
    /// Idempotent: calling twice is a no-op the second time.
    pub async fn shutdown(self: &Arc<Self>, timeout: Duration) {
        // Abort the interval task first so it doesn't race the final flush.
        let handle_opt = {
            let mut slot = self.flusher_handle.lock().await;
            slot.take()
        };
        if let Some(h) = handle_opt {
            h.abort();
            // The aborted future returns Err(JoinError::cancelled) — we
            // don't care; we just wanted to stop the loop.
            let _ = h.await;
        }

        let pending = self.len_inner();
        if pending == 0 {
            return;
        }
        match tokio::time::timeout(timeout, self.flush_internal()).await {
            Ok(_) => {}
            Err(_) => {
                let dropped = self.drain_all_inner();
                tracing::warn!(
                    dropped = dropped.len(),
                    timeout_ms = timeout.as_millis() as u64,
                    "EventBuffer shutdown timed out — dropping pending events"
                );
                record_dropped_shutdown_timeout(dropped.len() as u64);
                record_buffer_size(0);
            }
        }
    }

    // ── internal helpers ────────────────────────────────────────────────────

    fn len_inner(&self) -> usize {
        self.inner
            .buffer
            .lock()
            .expect("event buffer poisoned")
            .len()
    }

    fn is_empty_inner(&self) -> bool {
        self.len_inner() == 0
    }

    fn drain_all_inner(&self) -> Vec<BufferedEvent> {
        let mut buf = self.inner.buffer.lock().expect("event buffer poisoned");
        buf.drain(..).collect()
    }

    /// Core flush loop: drain, POST, retry up to `max_retries`, drop on
    /// exhaustion. Always consumes the drained batch (no re-enqueue on
    /// drop — the caller's contract says dropped events are dropped).
    async fn flush_internal(self: &Arc<Self>) -> Result<FlushReport, FlushError> {
        let batch = self.drain_all_inner();
        if batch.is_empty() {
            return Ok(FlushReport::default());
        }
        // The buffer was just emptied — keep the gauge in sync. (We
        // update again on any post-drop re-enqueue path; there isn't
        // one today, but this keeps the gauge truthful even if a future
        // refactor adds one.)
        record_buffer_size(0);
        let dropped_count = batch.len() as u32;

        // Serialize once — retries replay the same bytes.
        let wire = WireBatch {
            events: batch
                .iter()
                .map(|e| WireEvent {
                    event_key: &e.event_key,
                    context_type: &e.context_type,
                    context_key: &e.context_key,
                    value: e.value.as_ref(),
                    properties: e.properties.as_ref(),
                    occurred_at: e.occurred_at.map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                })
                .collect(),
        };
        let body = match serde_json::to_vec(&wire) {
            Ok(b) => b,
            Err(e) => {
                return Err(FlushError::Permanent {
                    status: 0,
                    message: format!("serialize: {e}"),
                });
            }
        };

        let mut last_err: String = String::new();
        for attempt in 0..=self.inner.config.max_retries {
            if attempt > 0 {
                let wait = self.inner.config.backoff_for(attempt - 1);
                tokio::time::sleep(wait).await;
            }

            let send_res = self
                .inner
                .http_client
                .post(&self.inner.endpoint)
                .header("x-sdk-key", &self.inner.config.sdk_key)
                .header("content-type", "application/json")
                .body(body.clone())
                .send()
                .await;

            match send_res {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if (200..300).contains(&status) {
                        // Parse {accepted_count, rejected[]}.
                        let parsed = match resp.json::<WireResponse>().await {
                            Ok(p) => p,
                            Err(_) => WireResponse {
                                accepted_count: dropped_count,
                                rejected: vec![],
                            },
                        };
                        let rejected_n = parsed.rejected.len() as u32;
                        // Phase 5 Task 5.4 — split accepted/rejected
                        // into the `result` label.
                        record_flushed_accepted(u64::from(parsed.accepted_count));
                        record_flushed_rejected(u64::from(rejected_n));
                        return Ok(FlushReport {
                            accepted: parsed.accepted_count,
                            rejected: rejected_n,
                            retried: attempt,
                        });
                    }
                    if Self::is_retryable_status(status) {
                        last_err = format!("HTTP {status}");
                        continue;
                    }
                    // Permanent — don't retry.
                    let msg = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        status,
                        count = dropped_count,
                        "EventBuffer flush rejected permanently — dropping batch"
                    );
                    record_dropped_permanent(u64::from(dropped_count));
                    return Err(FlushError::Permanent {
                        status,
                        message: msg,
                    });
                }
                Err(e) => {
                    last_err = e.to_string();
                    // Network errors are always transient.
                }
            }
        }

        // All retries exhausted — drop and warn.
        tracing::warn!(
            "dropped {} events after {} retries",
            dropped_count,
            self.inner.config.max_retries
        );
        record_dropped_retry_exhausted(u64::from(dropped_count));
        Err(FlushError::Network {
            count: dropped_count,
            last_error: last_err,
        })
    }

    /// Whether an HTTP status warrants a retry. 5xx, 408 (request
    /// timeout), 429 (rate limited) are retried. Everything else 4xx is
    /// permanent.
    fn is_retryable_status(status: u16) -> bool {
        status >= 500 || status == 408 || status == 429
    }
}

impl std::fmt::Debug for EventBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBuffer")
            .field("len", &self.len_inner())
            .field("flush_at_size", &self.inner.config.flush_at_size)
            .field("flush_interval", &self.inner.config.flush_interval)
            .field("endpoint", &self.inner.endpoint)
            .finish_non_exhaustive()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    fn ev(key: &str) -> BufferedEvent {
        BufferedEvent {
            event_key: key.to_string(),
            context_type: "user".into(),
            context_key: "u1".into(),
            value: Some(TypedValue::Int(1)),
            properties: None,
            occurred_at: None,
        }
    }

    fn config_for(server: &MockServer) -> EventBufferConfig {
        // Override defaults so tests don't have to wait the real 5s
        // interval / 200ms backoffs.
        EventBufferConfig {
            flush_at_size: 100,
            flush_interval: Duration::from_secs(60), // long — only explicit/size flushes
            max_retries: 3,
            backoff_base: Duration::from_millis(10),
            gateway_base_url: server.uri(),
            sdk_key: "test-sdk-key".to_string(),
        }
    }

    // ── Pure helpers ────────────────────────────────────────────────────────

    #[test]
    fn typed_value_serde_matches_gateway_shape() {
        // Must byte-match `crates/stitchd-gateway/src/routes/events.rs`.
        let cases = [
            (TypedValue::Bool(true), r#"{"bool":true}"#),
            (TypedValue::Int(42), r#"{"int":42}"#),
            (TypedValue::Double(1.5), r#"{"double":1.5}"#),
        ];
        for (val, expected) in cases {
            let s = serde_json::to_string(&val).unwrap();
            assert_eq!(s, expected);
            let parsed: TypedValue = serde_json::from_str(&s).unwrap();
            assert_eq!(parsed, val);
        }
    }

    #[test]
    fn backoff_doubles_each_attempt() {
        let cfg = EventBufferConfig {
            backoff_base: Duration::from_millis(200),
            ..EventBufferConfig::new("http://x", "k")
        };
        assert_eq!(cfg.backoff_for(0), Duration::from_millis(200));
        assert_eq!(cfg.backoff_for(1), Duration::from_millis(400));
        assert_eq!(cfg.backoff_for(2), Duration::from_millis(800));
    }

    #[test]
    fn wire_event_omits_optional_fields() {
        // Pure helper — verify the optional skip_serializing rules. An
        // event with no value/properties/occurred_at must serialize as a
        // minimal object.
        let e = BufferedEvent {
            event_key: "k".into(),
            context_type: "user".into(),
            context_key: "u".into(),
            value: None,
            properties: None,
            occurred_at: None,
        };
        let w = WireBatch {
            events: vec![WireEvent {
                event_key: &e.event_key,
                context_type: &e.context_type,
                context_key: &e.context_key,
                value: e.value.as_ref(),
                properties: e.properties.as_ref(),
                occurred_at: None,
            }],
        };
        let s = serde_json::to_string(&w).unwrap();
        assert!(
            !s.contains("value"),
            "expected 'value' to be omitted; got {s}"
        );
        assert!(
            !s.contains("properties"),
            "expected 'properties' to be omitted; got {s}"
        );
        assert!(
            !s.contains("occurred_at"),
            "expected 'occurred_at' to be omitted; got {s}"
        );
    }

    // ── 1. Enqueue below threshold ──────────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn test_enqueue_under_size_does_not_flush() {
        let server = MockServer::start().await;
        // Mount a Mock that EXPECTS zero calls — any POST will fail
        // verification on drop.
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 0,
                "rejected": []
            })))
            .expect(0)
            .mount(&server)
            .await;

        let cfg = EventBufferConfig {
            flush_at_size: 100,
            ..config_for(&server)
        };
        let buffer = EventBuffer::new(cfg);
        for _ in 0..50 {
            buffer.enqueue(ev("k"));
        }
        // Yield so any (incorrectly) spawned tasks get a chance to run.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        // No HTTP call should have been made — wiremock's `expect(0)`
        // will panic on drop if any occurred.
        assert_eq!(buffer.len_inner(), 50);
        // Drain the buffer BEFORE shutdown so the final-flush drain
        // doesn't violate expect(0). This test is specifically about
        // "enqueue alone doesn't flush"; shutdown-flush is covered by
        // test_shutdown_drains_pending.
        drop(buffer.drain_all_inner());
        buffer.shutdown(Duration::from_millis(50)).await;
    }

    // ── 2. Size-triggered flush ─────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn test_enqueue_at_size_triggers_flush() {
        let server = MockServer::start().await;
        let captured: Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        struct CaptureResponder {
            captured: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        }
        impl Respond for CaptureResponder {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                self.captured.lock().unwrap().push(body);
                ResponseTemplate::new(202).set_body_json(serde_json::json!({
                    "accepted_count": 100,
                    "rejected": []
                }))
            }
        }

        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .and(header("x-sdk-key", "test-sdk-key"))
            .respond_with(CaptureResponder {
                captured: Arc::clone(&captured),
            })
            .expect(1)
            .mount(&server)
            .await;

        let cfg = EventBufferConfig {
            flush_at_size: 100,
            ..config_for(&server)
        };
        let buffer = EventBuffer::new(cfg);
        for i in 0..100 {
            buffer.enqueue(ev(&format!("k{i}")));
        }
        // Let the size-triggered flush task complete.
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if !captured.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        {
            let calls = captured.lock().unwrap();
            assert_eq!(calls.len(), 1, "expected one POST");
            assert_eq!(calls[0]["events"].as_array().unwrap().len(), 100);
        }
        buffer.shutdown(Duration::from_secs(1)).await;
    }

    // ── 3. Interval flush ───────────────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_interval_flush_drains_buffer() {
        // Real-time test: paused-time + wiremock + reqwest don't play
        // well together (wiremock runs on a separate runtime and reqwest
        // does real I/O — `sleep` under start_paused doesn't drive
        // those). Use a tiny real interval (50ms) instead.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 10,
                "rejected": []
            })))
            .expect(1..)
            .mount(&server)
            .await;

        let cfg = EventBufferConfig {
            flush_at_size: 1000, // never trips on size
            flush_interval: Duration::from_millis(50),
            ..config_for(&server)
        };
        let buffer = EventBuffer::new(cfg);
        for i in 0..10 {
            buffer.enqueue(ev(&format!("k{i}")));
        }
        assert_eq!(buffer.len_inner(), 10);

        // Poll up to ~1s for the interval flusher to drain the buffer.
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while std::time::Instant::now() < deadline && buffer.len_inner() > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(buffer.len_inner(), 0, "interval flush should have drained");
        buffer.shutdown(Duration::from_secs(1)).await;
    }

    // ── 4. Explicit flush ───────────────────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn test_explicit_flush_returns_report() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 5,
                "rejected": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let buffer = EventBuffer::new(config_for(&server));
        for i in 0..5 {
            buffer.enqueue(ev(&format!("k{i}")));
        }
        let report = buffer.flush().await.expect("flush should succeed");
        assert_eq!(report.accepted, 5);
        assert_eq!(report.rejected, 0);
        assert_eq!(report.retried, 0);
        assert_eq!(buffer.len_inner(), 0);
        buffer.shutdown(Duration::from_millis(100)).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_explicit_flush_on_empty_buffer_is_noop() {
        let server = MockServer::start().await;
        // Expect zero calls — flush on empty buffer must NOT POST.
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202))
            .expect(0)
            .mount(&server)
            .await;

        let buffer = EventBuffer::new(config_for(&server));
        let report = buffer.flush().await.expect("flush should succeed");
        assert_eq!(report, FlushReport::default());
        buffer.shutdown(Duration::from_millis(100)).await;
    }

    // ── 5. Retry on 5xx then succeed ────────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn test_flush_failure_retries_with_backoff() {
        let server = MockServer::start().await;
        let attempt_count = Arc::new(AtomicU32::new(0));

        struct FailThenSucceed {
            count: Arc<AtomicU32>,
        }
        impl Respond for FailThenSucceed {
            fn respond(&self, _req: &Request) -> ResponseTemplate {
                let n = self.count.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    ResponseTemplate::new(500)
                } else {
                    ResponseTemplate::new(202).set_body_json(serde_json::json!({
                        "accepted_count": 3,
                        "rejected": []
                    }))
                }
            }
        }

        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(FailThenSucceed {
                count: Arc::clone(&attempt_count),
            })
            .expect(3)
            .mount(&server)
            .await;

        let cfg = EventBufferConfig {
            backoff_base: Duration::from_millis(1), // keep test fast
            ..config_for(&server)
        };
        let buffer = EventBuffer::new(cfg);
        for i in 0..3 {
            buffer.enqueue(ev(&format!("k{i}")));
        }
        let report = buffer
            .flush()
            .await
            .expect("flush should eventually succeed");
        assert_eq!(report.accepted, 3);
        assert_eq!(report.retried, 2, "first 2 attempts 500, 3rd 202");
        assert_eq!(attempt_count.load(Ordering::SeqCst), 3);
        buffer.shutdown(Duration::from_millis(100)).await;
    }

    // ── 6. Retry exhaustion drops events ────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn test_flush_max_retries_drops_with_warning() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(500))
            .expect(4) // initial + 3 retries
            .mount(&server)
            .await;

        let cfg = EventBufferConfig {
            backoff_base: Duration::from_millis(1),
            max_retries: 3,
            ..config_for(&server)
        };
        let buffer = EventBuffer::new(cfg);
        for i in 0..3 {
            buffer.enqueue(ev(&format!("k{i}")));
        }
        let err = buffer.flush().await.expect_err("should exhaust retries");
        match err {
            FlushError::Network { count, .. } => assert_eq!(count, 3),
            other => panic!("expected Network error, got {other:?}"),
        }
        // Batch is dropped — buffer is empty (no re-enqueue).
        assert_eq!(buffer.len_inner(), 0);
        buffer.shutdown(Duration::from_millis(100)).await;
    }

    // ── 7. Shutdown drains pending events ───────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn test_shutdown_drains_pending() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 5,
                "rejected": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let buffer = EventBuffer::new(config_for(&server));
        for i in 0..5 {
            buffer.enqueue(ev(&format!("k{i}")));
        }
        assert_eq!(buffer.len_inner(), 5);
        buffer.shutdown(Duration::from_secs(5)).await;
        assert_eq!(buffer.len_inner(), 0);
    }

    // ── 8. Shutdown timeout drops overflow ──────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn test_shutdown_timeout_drops_overflow() {
        let server = MockServer::start().await;
        // Mock that hangs longer than the shutdown timeout, forcing the
        // timeout branch.
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(
                ResponseTemplate::new(202)
                    .set_body_json(serde_json::json!({
                        "accepted_count": 0,
                        "rejected": []
                    }))
                    .set_delay(Duration::from_secs(10)),
            )
            .mount(&server)
            .await;

        let buffer = EventBuffer::new(config_for(&server));
        for i in 0..10 {
            buffer.enqueue(ev(&format!("k{i}")));
        }
        // Very short timeout — flush will be in-flight when timeout hits.
        buffer.shutdown(Duration::from_millis(20)).await;
        // After timeout, drain_all_inner() in the timeout branch should
        // have cleared the buffer. (Drained events are reported via
        // tracing::warn; we don't assert log capture here.)
        assert_eq!(buffer.len_inner(), 0);
    }

    // ── 9. 4xx is permanent (no retry) ─────────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn test_4xx_is_permanent_no_retry() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad body"))
            .expect(1) // exactly one — no retries on 4xx
            .mount(&server)
            .await;

        let cfg = EventBufferConfig {
            backoff_base: Duration::from_millis(1),
            ..config_for(&server)
        };
        let buffer = EventBuffer::new(cfg);
        buffer.enqueue(ev("k1"));
        let err = buffer.flush().await.expect_err("should be permanent");
        match err {
            FlushError::Permanent { status, .. } => assert_eq!(status, 400),
            other => panic!("expected Permanent error, got {other:?}"),
        }
        buffer.shutdown(Duration::from_millis(100)).await;
    }

    // ── 10. Per-event rejections counted ───────────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn test_per_event_rejections_counted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 1,
                "rejected": [
                    {"event_key": "k1", "reason": "unknown_event_key"},
                    {"event_key": "k2", "reason": "value_type_mismatch"}
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let buffer = EventBuffer::new(config_for(&server));
        buffer.enqueue(ev("k0"));
        buffer.enqueue(ev("k1"));
        buffer.enqueue(ev("k2"));
        let report = buffer.flush().await.expect("flush should succeed");
        assert_eq!(report.accepted, 1);
        assert_eq!(report.rejected, 2);
        buffer.shutdown(Duration::from_millis(100)).await;
    }

    // ── 11. occurred_at + properties round-trip ─────────────────────────────

    #[tokio::test(flavor = "current_thread")]
    async fn test_event_round_trips_properties_and_occurred_at() {
        let server = MockServer::start().await;
        let captured: Arc<std::sync::Mutex<Option<serde_json::Value>>> =
            Arc::new(std::sync::Mutex::new(None));

        struct Capture {
            slot: Arc<std::sync::Mutex<Option<serde_json::Value>>>,
        }
        impl Respond for Capture {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                *self.slot.lock().unwrap() =
                    Some(serde_json::from_slice(&req.body).unwrap());
                ResponseTemplate::new(202).set_body_json(serde_json::json!({
                    "accepted_count": 1,
                    "rejected": []
                }))
            }
        }

        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(Capture {
                slot: Arc::clone(&captured),
            })
            .expect(1)
            .mount(&server)
            .await;

        let buffer = EventBuffer::new(config_for(&server));
        let mut props = HashMap::new();
        props.insert("currency".to_string(), "USD".to_string());
        let when = DateTime::parse_from_rfc3339("2026-05-19T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        buffer.enqueue(BufferedEvent {
            event_key: "purchase".into(),
            context_type: "user".into(),
            context_key: "u42".into(),
            value: Some(TypedValue::Int(100)),
            properties: Some(props),
            occurred_at: Some(when),
        });
        buffer.flush().await.unwrap();

        let body = captured.lock().unwrap().clone().unwrap();
        let ev0 = &body["events"][0];
        assert_eq!(ev0["event_key"], "purchase");
        assert_eq!(ev0["context_type"], "user");
        assert_eq!(ev0["context_key"], "u42");
        assert_eq!(ev0["value"], serde_json::json!({"int": 100}));
        assert_eq!(ev0["properties"]["currency"], "USD");
        assert_eq!(ev0["occurred_at"], "2026-05-19T12:00:00Z");
        buffer.shutdown(Duration::from_millis(100)).await;
    }

    // ── 12-14. Prometheus counters / gauge (Phase 5 Task 5.4) ───────────────
    //
    // These tests install a `metrics_util::debugging::DebuggingRecorder` as
    // the thread-local recorder for the duration of the test. `current_thread`
    // runtime + `set_default_local_recorder` means all `tokio::spawn`ed work
    // inside the test sees the same recorder. The recorder snapshot exposes
    // the raw counter / gauge values we can assert against.

    use crate::metrics::{
        EVENTS_BUFFERED_TOTAL, EVENTS_FLUSHED_TOTAL, EVENT_BUFFER_SIZE,
    };
    use ::metrics_util::debugging::{DebugValue, DebuggingRecorder};

    /// Sum every counter sample whose key matches `name`, across all
    /// label combinations (so `events_flushed_total{result=accepted}` +
    /// `{result=rejected}` collapse into one number).
    fn snapshot_counter_total(snap: ::metrics_util::debugging::Snapshot, name: &str) -> u64 {
        snap.into_vec()
            .into_iter()
            .filter(|(ck, _, _, _)| ck.key().name() == name)
            .filter_map(|(_, _, _, v)| match v {
                DebugValue::Counter(n) => Some(n),
                _ => None,
            })
            .sum()
    }

    /// Pull the latest gauge sample for a given name.
    fn snapshot_gauge(snap: ::metrics_util::debugging::Snapshot, name: &str) -> Option<f64> {
        snap.into_vec()
            .into_iter()
            .find_map(|(ck, _, _, v)| match v {
                DebugValue::Gauge(f) if ck.key().name() == name => Some(*f),
                _ => None,
            })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_counter_events_buffered_total_increments_on_enqueue() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = ::metrics::set_default_local_recorder(&recorder);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 0,
                "rejected": []
            })))
            .mount(&server)
            .await;
        let buffer = EventBuffer::new(EventBufferConfig {
            flush_at_size: 1000, // never trips on size
            ..config_for(&server)
        });

        let before = snapshot_counter_total(snapshotter.snapshot(), EVENTS_BUFFERED_TOTAL);
        for _ in 0..7 {
            buffer.enqueue(ev("k"));
        }
        let after = snapshot_counter_total(snapshotter.snapshot(), EVENTS_BUFFERED_TOTAL);
        assert_eq!(
            after - before,
            7,
            "expected exactly 7 buffered_total increments"
        );

        // Cleanup — drain to avoid wiremock unmount.
        drop(buffer.drain_all_inner());
        buffer.shutdown(Duration::from_millis(50)).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_counter_events_flushed_total_increments_on_flush() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = ::metrics::set_default_local_recorder(&recorder);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 4,
                "rejected": [
                    {"event_key": "kb", "reason": "value_type_mismatch"}
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let buffer = EventBuffer::new(config_for(&server));
        for i in 0..5 {
            buffer.enqueue(ev(&format!("k{i}")));
        }

        let before = snapshot_counter_total(snapshotter.snapshot(), EVENTS_FLUSHED_TOTAL);
        let report = buffer.flush().await.expect("flush ok");
        assert_eq!(report.accepted, 4);
        assert_eq!(report.rejected, 1);
        let after = snapshot_counter_total(snapshotter.snapshot(), EVENTS_FLUSHED_TOTAL);
        // 4 accepted + 1 rejected → total counter sum increments by 5
        // (split across the `result` label).
        assert_eq!(
            after - before,
            5,
            "expected flushed_total to increment by accepted+rejected"
        );

        buffer.shutdown(Duration::from_millis(100)).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_gauge_event_buffer_size_reflects_buffer_len() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = ::metrics::set_default_local_recorder(&recorder);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/events/track"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
                "accepted_count": 3,
                "rejected": []
            })))
            .mount(&server)
            .await;
        let buffer = EventBuffer::new(EventBufferConfig {
            flush_at_size: 1000,
            ..config_for(&server)
        });
        for _ in 0..3 {
            buffer.enqueue(ev("k"));
        }
        let mid = snapshot_gauge(snapshotter.snapshot(), EVENT_BUFFER_SIZE);
        assert_eq!(
            mid,
            Some(3.0),
            "gauge should reflect 3 buffered events; got {mid:?}"
        );

        buffer.flush().await.expect("flush ok");
        let post = snapshot_gauge(snapshotter.snapshot(), EVENT_BUFFER_SIZE);
        assert_eq!(
            post,
            Some(0.0),
            "gauge should drop to 0 after flush; got {post:?}"
        );
        buffer.shutdown(Duration::from_millis(100)).await;
    }
}
