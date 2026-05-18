//! Scylla driver metrics → Prometheus bridge.
//!
//! Reads the built-in [`scylla::observability::metrics::Metrics`] snapshot from the
//! session and emits gauges via the [`metrics`] crate so they are automatically
//! scraped by the `metrics-exporter-prometheus` endpoint that the segmentation
//! service installs at startup.
//!
//! ## Metric names
//!
//! | Name | Kind | Description |
//! |---|---|---|
//! | `scylla_queries_total` | gauge | Cumulative non-paged query count |
//! | `scylla_query_errors_total` | gauge | Cumulative non-paged error count |
//! | `scylla_paged_queries_total` | gauge | Cumulative paged query count |
//! | `scylla_paged_query_errors_total` | gauge | Cumulative paged error count |
//! | `scylla_retries_total` | gauge | Cumulative retry count |
//! | `scylla_connections_active` | gauge | Current open connections |
//! | `scylla_connection_timeouts_total` | gauge | Cumulative connection timeouts |
//! | `scylla_request_timeouts_total` | gauge | Cumulative request timeouts |
//! | `scylla_prepared_cache_size` | gauge | Statements in the prepared-statement cache |
//! | `scylla_query_latency_p50_ms` | gauge | p50 query latency in milliseconds |
//! | `scylla_query_latency_p95_ms` | gauge | p95 query latency in milliseconds |
//! | `scylla_query_latency_p99_ms` | gauge | p99 query latency in milliseconds |

use scylla::observability::metrics::Metrics;

// Re-export the metrics crate macros under an alias to avoid ambiguity with
// this module's own name when called from within `metrics.rs`.
use ::metrics as prom;

// ---------------------------------------------------------------------------
// Snapshot (pure data — no Scylla connection required)
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of Scylla driver metrics.
///
/// Decouples collection (from a live `Metrics` struct) from emission
/// (to the `metrics` crate), making the emission logic unit-testable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScyllaMetricsSnapshot {
    /// Cumulative number of non-paged queries executed.
    pub queries_total: u64,
    /// Cumulative number of non-paged query errors.
    pub query_errors_total: u64,
    /// Cumulative number of paged queries executed.
    pub paged_queries_total: u64,
    /// Cumulative number of paged query errors.
    pub paged_query_errors_total: u64,
    /// Cumulative number of retries triggered by the retry policy.
    pub retries_total: u64,
    /// Current number of open connections to the cluster.
    pub connections_active: u64,
    /// Cumulative number of connection-establishment timeouts.
    pub connection_timeouts_total: u64,
    /// Cumulative number of client-side request timeouts.
    pub request_timeouts_total: u64,
    /// Number of prepared statements in the client's cache.
    pub prepared_cache_size: u64,
    /// p50 query latency in milliseconds (`0` when histogram is empty).
    pub latency_p50_ms: u64,
    /// p95 query latency in milliseconds (`0` when histogram is empty).
    pub latency_p95_ms: u64,
    /// p99 query latency in milliseconds (`0` when histogram is empty).
    pub latency_p99_ms: u64,
}

impl ScyllaMetricsSnapshot {
    /// Collect a snapshot from the Scylla driver `Metrics` struct.
    ///
    /// Latency percentiles default to `0` when the histogram has no observations.
    #[must_use]
    pub fn collect(m: &Metrics, prepared_cache_size: u64) -> Self {
        let latency_p50_ms = m.get_latency_percentile_ms(50.0).unwrap_or(0);
        let latency_p95_ms = m.get_latency_percentile_ms(95.0).unwrap_or(0);
        let latency_p99_ms = m.get_latency_percentile_ms(99.0).unwrap_or(0);

        Self {
            queries_total: m.get_queries_num(),
            query_errors_total: m.get_errors_num(),
            paged_queries_total: m.get_queries_iter_num(),
            paged_query_errors_total: m.get_errors_iter_num(),
            retries_total: m.get_retries_num(),
            connections_active: m.get_total_connections(),
            connection_timeouts_total: m.get_connection_timeouts(),
            request_timeouts_total: m.get_request_timeouts(),
            prepared_cache_size,
            latency_p50_ms,
            latency_p95_ms,
            latency_p99_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------

/// Emit all metrics from `snapshot` to the global `metrics` recorder.
///
/// This is designed for periodic polling (e.g. every 15 s) rather than
/// per-query instrumentation, so all values are emitted as **gauges**
/// (absolute point-in-time readings) rather than counters.
///
/// # Precision
/// Counter values are cast `u64 → f64`. Precision is lost above 2^53
/// (~9 × 10^15), which is unreachable in practice for query/error counts.
#[allow(clippy::cast_precision_loss)]
pub fn emit_metrics(snapshot: &ScyllaMetricsSnapshot) {
    prom::gauge!("scylla_queries_total").set(snapshot.queries_total as f64);
    prom::gauge!("scylla_query_errors_total").set(snapshot.query_errors_total as f64);
    prom::gauge!("scylla_paged_queries_total").set(snapshot.paged_queries_total as f64);
    prom::gauge!("scylla_paged_query_errors_total").set(snapshot.paged_query_errors_total as f64);
    prom::gauge!("scylla_retries_total").set(snapshot.retries_total as f64);
    prom::gauge!("scylla_connections_active").set(snapshot.connections_active as f64);
    prom::gauge!("scylla_connection_timeouts_total").set(snapshot.connection_timeouts_total as f64);
    prom::gauge!("scylla_request_timeouts_total").set(snapshot.request_timeouts_total as f64);
    prom::gauge!("scylla_prepared_cache_size").set(snapshot.prepared_cache_size as f64);
    prom::gauge!("scylla_query_latency_p50_ms").set(snapshot.latency_p50_ms as f64);
    prom::gauge!("scylla_query_latency_p95_ms").set(snapshot.latency_p95_ms as f64);
    prom::gauge!("scylla_query_latency_p99_ms").set(snapshot.latency_p99_ms as f64);
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{ScyllaMetricsSnapshot, emit_metrics};

    fn sample_snapshot() -> ScyllaMetricsSnapshot {
        ScyllaMetricsSnapshot {
            queries_total: 1000,
            query_errors_total: 5,
            paged_queries_total: 200,
            paged_query_errors_total: 1,
            retries_total: 3,
            connections_active: 4,
            connection_timeouts_total: 0,
            request_timeouts_total: 2,
            prepared_cache_size: 12,
            latency_p50_ms: 2,
            latency_p95_ms: 8,
            latency_p99_ms: 15,
        }
    }

    /// `ScyllaMetricsSnapshot::default()` produces all-zero values.
    #[test]
    fn default_snapshot_is_zeroed() {
        let s = ScyllaMetricsSnapshot::default();
        assert_eq!(s.queries_total, 0);
        assert_eq!(s.connections_active, 0);
        assert_eq!(s.latency_p99_ms, 0);
    }

    /// Snapshot fields round-trip through the struct.
    #[test]
    fn snapshot_fields_accessible() {
        let s = sample_snapshot();
        assert_eq!(s.queries_total, 1000);
        assert_eq!(s.query_errors_total, 5);
        assert_eq!(s.latency_p99_ms, 15);
        assert_eq!(s.prepared_cache_size, 12);
    }

    /// `emit_metrics` must not panic even when the metrics recorder is a no-op.
    ///
    /// By default, `metrics` uses a no-op recorder when none has been installed;
    /// this test verifies the function completes without errors in that scenario.
    #[test]
    fn emit_does_not_panic() {
        let snapshot = sample_snapshot();
        emit_metrics(&snapshot); // recorder may be no-op — must not panic
        emit_metrics(&ScyllaMetricsSnapshot::default()); // all-zero edge case
    }

    /// Emitting a snapshot with max u64 values does not overflow or panic.
    #[test]
    fn emit_large_values() {
        let large = ScyllaMetricsSnapshot {
            queries_total: u64::MAX,
            query_errors_total: u64::MAX,
            latency_p99_ms: u64::MAX,
            ..ScyllaMetricsSnapshot::default()
        };
        emit_metrics(&large);
    }
}
