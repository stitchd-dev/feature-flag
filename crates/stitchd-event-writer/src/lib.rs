//! ClickHouse event ingestion for experiments and metrics.
//!
//! Only pre-registered events with known types are accepted at the ingestion boundary.
#![deny(warnings, missing_docs, clippy::all)]
#![warn(clippy::pedantic, clippy::nursery)]

pub mod migrations;
pub mod writer;

// ---------------------------------------------------------------------------
// Test-support fixtures
// ---------------------------------------------------------------------------

/// A row suitable for seeding the `events` ClickHouse table in integration tests.
///
/// Unlike [`writer::EventRow`] (which uses `i64` Unix-millis for timestamp
/// columns so the writer can be called without a chrono dependency at each
/// call site), `SeedEventRow` accepts `DateTime<Utc>` and applies the
/// `clickhouse::serde::chrono::datetime64::millis` codec. This makes
/// seeding test data ergonomic without having to convert manually.
///
/// Available under `any(test, feature = "test-fixtures")` so integration
/// tests in sibling crates can import it without pulling test-only code
/// into production builds.
#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Debug, Clone, serde::Serialize, clickhouse::Row)]
pub struct SeedEventRow {
    /// Environment UUID.
    #[serde(with = "clickhouse::serde::uuid")]
    pub env_id: uuid::Uuid,
    /// Evaluation-time context pairs: `(context_type, context_key)`.
    pub contexts: Vec<(String, String)>,
    /// Pre-registered metric key.
    pub metric_key: String,
    /// Boolean value, set when the event carries a `Bool` metric.
    pub value_bool: Option<bool>,
    /// Integer value, set when the event carries an `Int` metric.
    pub value_int: Option<i64>,
    /// Double value, set when the event carries a `Double` metric.
    pub value_double: Option<f64>,
    /// Server-side ingestion timestamp.
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// ClickHouse-side ingestion timestamp (filled by `DEFAULT now64()`
    /// but we supply it explicitly in tests to control ordering).
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    pub ingested_at: chrono::DateTime<chrono::Utc>,
    /// Arbitrary per-event metadata.
    pub properties: Vec<(String, String)>,
    /// Client wall-clock time.
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    pub occurred_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {}
}
