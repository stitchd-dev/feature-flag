//! Shared test helpers for gRPC handler integration tests.
//!
//! This module is compiled only under `#[cfg(test)]` and provides the common
//! `InsertRow` used by `event_query.rs` and `metric.rs` tests to seed the
//! ClickHouse `events` table without pulling in the `stitchd-event-writer`
//! crate as a dev-dependency.

/// A minimal row for seeding the `events` ClickHouse table in tests.
///
/// Timestamps are `i64` Unix-milliseconds matching the production schema.
/// `ingested_at` is omitted — the CH table uses `DEFAULT now64()` so it
/// is filled server-side when omitted in a RowBinary insert.
#[derive(serde::Serialize, clickhouse::Row)]
pub struct InsertRow<'a> {
    #[serde(with = "clickhouse::serde::uuid")]
    pub env_id: uuid::Uuid,
    pub contexts: Vec<(String, String)>,
    pub metric_key: &'a str,
    pub value_bool: Option<bool>,
    pub value_int: Option<i64>,
    pub value_double: Option<f64>,
    pub timestamp: i64,
    pub properties: Vec<(String, String)>,
    pub occurred_at: i64,
}
