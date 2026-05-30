//! ClickHouse query layer — experiment analysis and evaluation telemetry.

pub mod eval_log;
pub mod experiment_queries;

pub use eval_log::{EvalLogRow, insert_eval_log_rows};
pub use experiment_queries::{
    CountMetricRow, FunnelStepRow, NumericMetricRow, QueryError, query_count_metric, query_funnel,
    query_numeric_metric,
};

// ---------------------------------------------------------------------------
// Test-support fixtures
// ---------------------------------------------------------------------------

/// A row suitable for seeding the `experiment_assignments` ClickHouse table
/// in integration tests.
///
/// Matches the 10-field write shape expected by the
/// `ReplacingMergeTree(version)` engine:
/// - `assigned_at` uses `clickhouse::serde::chrono::datetime64::millis` so
///   it round-trips through the `RowBinary` wire as `DateTime64(3)`.
/// - `_version` is the negated assignment epoch (milliseconds) which keeps
///   the **earliest** exposure when the tree is collapsed with `FINAL`.
///
/// Available under `any(test, feature = "test-fixtures")` so integration
/// tests in sibling crates can import it without pulling test-only code
/// into production builds.
#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Debug, Clone, serde::Serialize, clickhouse::Row)]
pub struct SeedAssignmentRow {
    /// Experiment UUID.
    #[serde(with = "clickhouse::serde::uuid")]
    pub experiment_id: uuid::Uuid,
    /// Iteration UUID.
    #[serde(with = "clickhouse::serde::uuid")]
    pub iteration_id: uuid::Uuid,
    /// Environment UUID.
    #[serde(with = "clickhouse::serde::uuid")]
    pub env_id: uuid::Uuid,
    /// Feature flag UUID.
    #[serde(with = "clickhouse::serde::uuid")]
    pub flag_id: uuid::Uuid,
    /// Matched rule UUID, if any.
    #[serde(with = "clickhouse::serde::uuid::option")]
    pub matched_rule_id: Option<uuid::Uuid>,
    /// Context dimension (e.g. `"user"`, `"account"`).
    pub context_type: String,
    /// Context identifier within that dimension.
    pub context_key: String,
    /// Variant the context was bucketed into.
    pub variant_key: String,
    /// Wall-clock time the context was first exposed to this experiment.
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    pub assigned_at: chrono::DateTime<chrono::Utc>,
    /// `ReplacingMergeTree` version — set to `-assigned_at.timestamp_millis()`
    /// so `FINAL` keeps the row with the **earliest** `assigned_at`.
    #[serde(rename = "_version")]
    pub version: i64,
}
