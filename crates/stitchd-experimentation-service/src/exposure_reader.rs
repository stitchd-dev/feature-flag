//! Paginated reads against the ClickHouse `experiment_assignments` table.
//!
//! Powers the gateway's `GET /v1/environments/{env}/experiments/{id}/exposures`
//! endpoint (Phase 7 Task 2). Each row is a first-exposure assignment carrying
//! `(context_type, context_key, variant_key, assigned_at, matched_rule_id)`.
//!
//! Abstracted behind the [`ExposureReader`] trait so unit tests can stub the
//! reader without standing up a real CH client. Production wiring uses
//! [`ClickHouseExposureReader`].

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use clickhouse::Client;
use std::sync::Arc;
use uuid::Uuid;

/// One exposure row returned by [`ExposureReader::list_exposures`].
#[derive(Debug, Clone, PartialEq)]
pub struct ExposureRow {
    pub context_type: String,
    pub context_key: String,
    pub variant_key: String,
    pub assigned_at: DateTime<Utc>,
    /// `None` for default-rule-bound experiments where the eval-log row also
    /// carried a NULL `matched_rule_id`.
    pub matched_rule_id: Option<Uuid>,
}

/// Server-side cap on `limit`. Mirrors the cap on other admin list endpoints.
pub const MAX_LIMIT: u64 = 200;

/// Trait for paginated reads against `experiment_assignments`.
///
/// The single method returns one page of rows plus the total count of rows
/// matching `(experiment_id, context_type)`. The total is computed by the
/// caller's preferred backend (a separate `count(*)` query in the CH impl).
#[async_trait]
pub trait ExposureReader: Send + Sync + 'static {
    /// Fetch one page of exposure rows for `(experiment_id, context_type)`.
    ///
    /// `offset` and `limit` are caller-validated (no clamping inside the
    /// trait); the production CH impl applies [`MAX_LIMIT`] before issuing the
    /// query.
    async fn list_exposures(
        &self,
        experiment_id: Uuid,
        context_type: &str,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<ExposureRow>, u64), clickhouse::error::Error>;
}

/// Production implementation backed by a `clickhouse::Client`.
pub struct ClickHouseExposureReader {
    client: Arc<Client>,
}

impl ClickHouseExposureReader {
    /// Wrap a shared CH client.
    #[must_use]
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct CHExposureRow {
    context_type: String,
    context_key: String,
    variant_key: String,
    /// `DateTime64(3, 'UTC')` materialises as `i64` epoch-millis when
    /// deserialized to the `clickhouse::Row` driver. We reconstruct a
    /// `DateTime<Utc>` from it.
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    assigned_at: DateTime<Utc>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    matched_rule_id: Option<Uuid>,
}

#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct CountRow {
    total: u64,
}

#[async_trait]
impl ExposureReader for ClickHouseExposureReader {
    async fn list_exposures(
        &self,
        experiment_id: Uuid,
        context_type: &str,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<ExposureRow>, u64), clickhouse::error::Error> {
        let limit = limit.min(MAX_LIMIT);
        let experiment_id_str = experiment_id.to_string();

        // We use FINAL to collapse `ReplacingMergeTree` versions so callers see
        // exactly one row per `(experiment_id, iteration_id, context_type,
        // context_key)`. The cardinality of a single experiment+context_type
        // page is small (≤ MAX_LIMIT), so the cost of `FINAL` is bounded.

        let count_sql = format!(
            r"
            SELECT count(*) AS total
            FROM experiment_assignments FINAL
            WHERE experiment_id = toUUID('{experiment_id_str}')
              AND context_type  = ?
            "
        );
        let count_row: CountRow = self
            .client
            .query(&count_sql)
            .bind(context_type)
            .fetch_one()
            .await?;

        let rows_sql = format!(
            r"
            SELECT
                context_type,
                context_key,
                variant_key,
                toUnixTimestamp64Milli(assigned_at) AS assigned_at,
                matched_rule_id
            FROM experiment_assignments FINAL
            WHERE experiment_id = toUUID('{experiment_id_str}')
              AND context_type  = ?
            ORDER BY assigned_at DESC, context_key ASC
            LIMIT {limit}
            OFFSET {offset}
            "
        );
        let rows: Vec<CHExposureRow> = self
            .client
            .query(&rows_sql)
            .bind(context_type)
            .fetch_all()
            .await?;

        let exposures = rows
            .into_iter()
            .map(|r| ExposureRow {
                context_type: r.context_type,
                context_key: r.context_key,
                variant_key: r.variant_key,
                assigned_at: r.assigned_at,
                matched_rule_id: r.matched_rule_id,
            })
            .collect();

        Ok((exposures, count_row.total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `(experiment_id, context_type, offset, limit)` captured per call.
    type StubCall = (Uuid, String, u64, u64);

    /// Test stub recording invocations and returning canned rows.
    pub struct StubReader {
        pub calls: Arc<Mutex<Vec<StubCall>>>,
        pub rows: Vec<ExposureRow>,
        pub total: u64,
    }

    #[async_trait]
    impl ExposureReader for StubReader {
        async fn list_exposures(
            &self,
            experiment_id: Uuid,
            context_type: &str,
            offset: u64,
            limit: u64,
        ) -> Result<(Vec<ExposureRow>, u64), clickhouse::error::Error> {
            self.calls.lock().unwrap().push((
                experiment_id,
                context_type.to_string(),
                offset,
                limit,
            ));
            Ok((self.rows.clone(), self.total))
        }
    }

    #[tokio::test]
    async fn stub_reader_round_trips_params_and_rows() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let exp_id = Uuid::new_v4();
        let row = ExposureRow {
            context_type: "user".to_string(),
            context_key: "alice".to_string(),
            variant_key: "control".to_string(),
            assigned_at: Utc::now(),
            matched_rule_id: Some(Uuid::new_v4()),
        };
        let reader = StubReader {
            calls: calls.clone(),
            rows: vec![row.clone()],
            total: 42,
        };
        let (rows, total) = reader.list_exposures(exp_id, "user", 0, 50).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].context_key, "alice");
        assert_eq!(total, 42);
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].1, "user");
    }

    #[test]
    fn max_limit_is_two_hundred() {
        // Sanity check that the cap matches the wider admin-list convention.
        assert_eq!(MAX_LIMIT, 200);
    }
}
