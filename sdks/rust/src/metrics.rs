//! Central registry of SDK metric names + emission helpers.
//!
//! Implements the SDK side of spec § F4.6 — the gateway's request-level
//! metrics live in `crates/stitchd-gateway/src/metrics.rs`; this module
//! owns the **client-side** counters/gauges around the track-event
//! buffer.
//!
//! ## Why a thin wrapper around the `metrics` crate?
//!
//! - **One place** for metric NAMES — typo-resistant, refactor-friendly.
//! - **Label discipline** — every callsite hits the same key set, so a
//!   downstream Prometheus rule never has to special-case "did this
//!   counter have the `result` label or not?".
//! - **No global initialization required**. If the host process hasn't
//!   installed a `metrics` recorder (e.g. unit tests that don't care),
//!   every `counter!` / `gauge!` call is a cheap no-op.
//!
//! ## Labels
//!
//! `env_id` is intentionally OMITTED from SDK-side metric labels. The
//! gateway tags every ingested event with `env_id` (derived from the SDK
//! key) before recording its own counters, so the SDK doesn't need to
//! re-emit the dimension. Keeping the SDK side label-free also keeps
//! `EventBufferConfig` stable (no new field) and avoids any cardinality
//! confusion at the recorder.
//!
//! The one label we DO emit is the small-cardinality `result` /
//! `reason` discriminator on the "flushed" + "dropped" counters, since
//! distinguishing accepted-vs-rejected and retry-exhausted-vs-shutdown
//! is operationally important.

// ── Metric names ────────────────────────────────────────────────────────────
//
// Exposed as `pub(crate) const` so the test module can assert exact
// strings without copy/paste drift.

/// Counter — incremented once per call to `EventBuffer::enqueue`.
/// No labels. Matches spec § F4.6 "events_buffered_total".
pub(crate) const EVENTS_BUFFERED_TOTAL: &str = "stitchd_sdk_events_buffered_total";

/// Counter — incremented after each successful flush. Carries a single
/// label, `result`, with values `accepted` or `rejected` (the latter
/// counts per-event rejections returned in the 202 body).
pub(crate) const EVENTS_FLUSHED_TOTAL: &str = "stitchd_sdk_events_flushed_total";

/// Counter — incremented when a batch is dropped (retries exhausted, or
/// shutdown timeout fired). Label `reason` ∈ `retry_exhausted |
/// shutdown_timeout | permanent_4xx`.
pub(crate) const EVENTS_DROPPED_TOTAL: &str = "stitchd_sdk_events_dropped_total";

/// Gauge — current number of events sitting in the per-Client buffer.
/// Updated after every enqueue + flush. No labels.
pub(crate) const EVENT_BUFFER_SIZE: &str = "stitchd_sdk_event_buffer_size";

// ── Emission helpers ────────────────────────────────────────────────────────
//
// All helpers are `pub(crate)` — only `event_buffer.rs` should be
// emitting these. The functions are intentionally tiny so they get
// inlined and the `metrics::*!` macros expand inside the caller.

/// Record one `enqueue()` event. Caller is responsible for bumping
/// [`record_buffer_size`] afterward.
#[inline]
pub(crate) fn record_buffered() {
    metrics::counter!(EVENTS_BUFFERED_TOTAL).increment(1);
}

/// Record `n` events as accepted by the gateway on a successful flush.
/// `n` may be zero (e.g. flush of an empty batch after server-side dedup).
#[inline]
pub(crate) fn record_flushed_accepted(n: u64) {
    metrics::counter!(EVENTS_FLUSHED_TOTAL, "result" => "accepted").increment(n);
}

/// Record `n` events as per-event-rejected (HTTP 202 with `rejected[]`).
#[inline]
pub(crate) fn record_flushed_rejected(n: u64) {
    metrics::counter!(EVENTS_FLUSHED_TOTAL, "result" => "rejected").increment(n);
}

/// Record `n` events as dropped after retry exhaustion.
#[inline]
pub(crate) fn record_dropped_retry_exhausted(n: u64) {
    metrics::counter!(EVENTS_DROPPED_TOTAL, "reason" => "retry_exhausted").increment(n);
}

/// Record `n` events as dropped because the gateway returned a permanent
/// HTTP 4xx (and we don't retry).
#[inline]
pub(crate) fn record_dropped_permanent(n: u64) {
    metrics::counter!(EVENTS_DROPPED_TOTAL, "reason" => "permanent_4xx").increment(n);
}

/// Record `n` events as dropped because the shutdown timeout fired
/// before the final flush completed.
#[inline]
pub(crate) fn record_dropped_shutdown_timeout(n: u64) {
    metrics::counter!(EVENTS_DROPPED_TOTAL, "reason" => "shutdown_timeout").increment(n);
}

/// Update the buffer-size gauge to the current queue length.
#[inline]
pub(crate) fn record_buffer_size(len: usize) {
    // metrics::gauge!() takes f64.
    #[allow(clippy::cast_precision_loss)]
    metrics::gauge!(EVENT_BUFFER_SIZE).set(len as f64);
}
