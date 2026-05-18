//! Repository trait and ClickHouse implementation for `experiment_results`.
//!
//! Each row represents the computed statistical analysis for one metric
//! within one iteration of an experiment, scoped to an environment.
//!
//! # ClickHouse schema (see migrations/0001_experiment_results.sql)
//! The table uses `MergeTree` ordered by
//! `(env_id, experiment_id, variant_key, metric_key, iteration_id)`.
//! JSON payloads (variant_stats, frequentist_result, bayesian_result) are
//! stored as `String`; absent optional fields use empty string `""`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`ExperimentResultsRepository`] operations.
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    /// ClickHouse client error.
    #[error("clickhouse error: {0}")]
    Clickhouse(#[from] clickhouse::error::Error),
    /// JSON serialization / deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

/// A row as written to / read from the ClickHouse `experiment_results` table.
///
/// JSON columns (`variant_stats`, `frequentist_result`, `bayesian_result`) are
/// stored as `String`. Absent optional results use `""`.
#[derive(Debug, Clone, Serialize, Deserialize, clickhouse::Row)]
pub struct ExperimentResultRow {
    /// Environment UUID (tenant scope).
    #[serde(with = "clickhouse::serde::uuid")]
    pub env_id: Uuid,
    /// FK → experiments.id
    #[serde(with = "clickhouse::serde::uuid")]
    pub experiment_id: Uuid,
    /// FK → experiment_iterations.id
    #[serde(with = "clickhouse::serde::uuid")]
    pub iteration_id: Uuid,
    /// Variant key (e.g. `"control"`, `"treatment_a"`).
    pub variant_key: String,
    /// Metric key (e.g. `"checkout_completed"`).
    pub metric_key: String,
    /// Metric type discriminant: `"count"`, `"numeric"`, `"percentile"`, `"funnel"`.
    pub metric_type: String,
    /// Per-variant sample statistics serialised as JSON string.
    pub variant_stats: String,
    /// Frequentist analysis result as JSON string; `""` when absent.
    pub frequentist_result: String,
    /// Bayesian analysis result as JSON string; `""` when absent.
    pub bayesian_result: String,
    /// Human-readable recommendation string.
    pub recommendation: String,
    /// When this result was computed (Unix milliseconds UTC).
    pub computed_at: i64,
    /// When this row was first inserted (Unix milliseconds UTC).
    pub created_at: i64,
}

/// Input type for [`ExperimentResultsRepository::write`].
///
/// Mirrors `stitchd_db::UpsertResultRow` but adds `env_id` and `variant_key`
/// which are required for the ClickHouse ORDER BY key.
#[derive(Debug, Clone)]
pub struct WriteResultRow {
    /// Environment UUID (tenant scope).
    pub env_id: Uuid,
    /// FK → experiments.id
    pub experiment_id: Uuid,
    /// FK → experiment_iterations.id
    pub iteration_id: Uuid,
    /// Variant key.
    pub variant_key: String,
    /// Metric key.
    pub metric_key: String,
    /// Metric type discriminant.
    pub metric_type: String,
    /// Per-variant sample statistics.
    pub variant_stats: serde_json::Value,
    /// Frequentist analysis result; `None` stored as `""`.
    pub frequentist_result: Option<serde_json::Value>,
    /// Bayesian analysis result; `None` stored as `""`.
    pub bayesian_result: Option<serde_json::Value>,
    /// Human-readable recommendation.
    pub recommendation: String,
    /// Timestamp the analysis was computed.
    pub computed_at: chrono::DateTime<chrono::Utc>,
}

/// Filter for list / get queries.
#[derive(Debug, Clone)]
pub struct ResultsFilter {
    /// Required: environment scope.
    pub env_id: Uuid,
    /// Required: experiment to query.
    pub experiment_id: Uuid,
    /// Optional: constrain to a specific iteration.
    pub iteration_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Data-access interface for the ClickHouse `experiment_results` table.
///
/// # TODO(merge): Worker 1 compatibility note
/// Worker 1 is adding gRPC handlers that depend on this trait. If the method
/// names here differ from what Worker 1 assumed (e.g. `upsert` vs `write`,
/// or `fetch_latest` vs `list`), reconcile at merge time.
/// The canonical interface is defined here; Worker 1 should adapt.
#[async_trait]
pub trait ExperimentResultsRepository: Send + Sync {
    /// Write a batch of result rows to ClickHouse.
    ///
    /// ClickHouse inserts are append-only; callers are responsible for
    /// ensuring idempotency if needed (e.g. deduplicate via ReplacingMergeTree
    /// or by writing distinct rows only).
    ///
    /// # Errors
    /// Returns [`RepoError`] if serialisation or the ClickHouse insert fails.
    async fn write(&self, rows: Vec<WriteResultRow>) -> Result<(), RepoError>;

    /// List all result rows matching the filter.
    ///
    /// When `filter.iteration_id` is `None`, all iterations are returned.
    /// Results are ordered by `(experiment_id, iteration_id, variant_key, metric_key)`.
    ///
    /// # Errors
    /// Returns [`RepoError`] if the ClickHouse query fails.
    async fn list(&self, filter: &ResultsFilter) -> Result<Vec<ExperimentResultRow>, RepoError>;

    /// Fetch the single result row matching exact keys.
    ///
    /// When `iteration_id` is `None` the most-recently-computed row for that
    /// `(env_id, experiment_id, variant_key, metric_key)` combination is
    /// returned.
    ///
    /// Returns `None` when no matching row exists.
    ///
    /// # Errors
    /// Returns [`RepoError`] if the ClickHouse query fails.
    async fn get(
        &self,
        env_id: Uuid,
        experiment_id: Uuid,
        variant_key: &str,
        metric_key: &str,
        iteration_id: Option<Uuid>,
    ) -> Result<Option<ExperimentResultRow>, RepoError>;
}

// ---------------------------------------------------------------------------
// ClickHouse implementation
// ---------------------------------------------------------------------------

const TABLE: &str = "experiment_results";

/// ClickHouse-backed implementation of [`ExperimentResultsRepository`].
pub struct ClickHouseExperimentResultsRepository {
    client: clickhouse::Client,
}

impl ClickHouseExperimentResultsRepository {
    /// Construct a new repository backed by `client`.
    #[must_use]
    pub const fn new(client: clickhouse::Client) -> Self {
        Self { client }
    }

    /// Convert a [`WriteResultRow`] to the serialisable [`ExperimentResultRow`].
    fn to_ch_row(row: WriteResultRow) -> Result<ExperimentResultRow, serde_json::Error> {
        Ok(ExperimentResultRow {
            env_id: row.env_id,
            experiment_id: row.experiment_id,
            iteration_id: row.iteration_id,
            variant_key: row.variant_key,
            metric_key: row.metric_key,
            metric_type: row.metric_type,
            variant_stats: serde_json::to_string(&row.variant_stats)?,
            frequentist_result: row
                .frequentist_result
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?
                .unwrap_or_default(),
            bayesian_result: row
                .bayesian_result
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?
                .unwrap_or_default(),
            recommendation: row.recommendation,
            computed_at: row.computed_at.timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
        })
    }
}

#[async_trait]
impl ExperimentResultsRepository for ClickHouseExperimentResultsRepository {
    async fn write(&self, rows: Vec<WriteResultRow>) -> Result<(), RepoError> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut insert = self.client.insert(TABLE)?;
        for row in rows {
            let ch_row = Self::to_ch_row(row)?;
            insert.write(&ch_row).await?;
        }
        insert.end().await?;
        Ok(())
    }

    async fn list(&self, filter: &ResultsFilter) -> Result<Vec<ExperimentResultRow>, RepoError> {
        let rows = if let Some(iter_id) = filter.iteration_id {
            self.client
                .query(
                    "SELECT env_id, experiment_id, iteration_id, variant_key, metric_key, \
                     metric_type, variant_stats, frequentist_result, bayesian_result, \
                     recommendation, computed_at, created_at \
                     FROM experiment_results \
                     WHERE env_id = ? AND experiment_id = ? AND iteration_id = ? \
                     ORDER BY experiment_id, iteration_id, variant_key, metric_key",
                )
                .bind(filter.env_id)
                .bind(filter.experiment_id)
                .bind(iter_id)
                .fetch_all::<ExperimentResultRow>()
                .await?
        } else {
            self.client
                .query(
                    "SELECT env_id, experiment_id, iteration_id, variant_key, metric_key, \
                     metric_type, variant_stats, frequentist_result, bayesian_result, \
                     recommendation, computed_at, created_at \
                     FROM experiment_results \
                     WHERE env_id = ? AND experiment_id = ? \
                     ORDER BY experiment_id, iteration_id, variant_key, metric_key",
                )
                .bind(filter.env_id)
                .bind(filter.experiment_id)
                .fetch_all::<ExperimentResultRow>()
                .await?
        };
        Ok(rows)
    }

    async fn get(
        &self,
        env_id: Uuid,
        experiment_id: Uuid,
        variant_key: &str,
        metric_key: &str,
        iteration_id: Option<Uuid>,
    ) -> Result<Option<ExperimentResultRow>, RepoError> {
        let row = if let Some(iter_id) = iteration_id {
            self.client
                .query(
                    "SELECT env_id, experiment_id, iteration_id, variant_key, metric_key, \
                     metric_type, variant_stats, frequentist_result, bayesian_result, \
                     recommendation, computed_at, created_at \
                     FROM experiment_results \
                     WHERE env_id = ? AND experiment_id = ? \
                       AND variant_key = ? AND metric_key = ? \
                       AND iteration_id = ? \
                     LIMIT 1",
                )
                .bind(env_id)
                .bind(experiment_id)
                .bind(variant_key)
                .bind(metric_key)
                .bind(iter_id)
                .fetch_optional::<ExperimentResultRow>()
                .await?
        } else {
            // Return the most-recently-computed row when no iteration is specified.
            self.client
                .query(
                    "SELECT env_id, experiment_id, iteration_id, variant_key, metric_key, \
                     metric_type, variant_stats, frequentist_result, bayesian_result, \
                     recommendation, computed_at, created_at \
                     FROM experiment_results \
                     WHERE env_id = ? AND experiment_id = ? \
                       AND variant_key = ? AND metric_key = ? \
                     ORDER BY computed_at DESC \
                     LIMIT 1",
                )
                .bind(env_id)
                .bind(experiment_id)
                .bind(variant_key)
                .bind(metric_key)
                .fetch_optional::<ExperimentResultRow>()
                .await?
        };
        Ok(row)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_write_row(
        env_id: Uuid,
        experiment_id: Uuid,
        iteration_id: Uuid,
        variant_key: &str,
        metric_key: &str,
    ) -> WriteResultRow {
        WriteResultRow {
            env_id,
            experiment_id,
            iteration_id,
            variant_key: variant_key.to_string(),
            metric_key: metric_key.to_string(),
            metric_type: "count".to_string(),
            variant_stats: serde_json::json!({ "control": 100, "treatment": 120 }),
            frequentist_result: Some(serde_json::json!({ "p_value": 0.03 })),
            bayesian_result: None,
            recommendation: "ship_treatment".to_string(),
            computed_at: Utc::now(),
        }
    }

    // -----------------------------------------------------------------------
    // Unit tests (no live ClickHouse required)
    // -----------------------------------------------------------------------

    /// `to_ch_row` serialises optional JSON fields correctly.
    #[test]
    fn to_ch_row_serialises_optional_json() {
        let env_id = Uuid::new_v4();
        let exp_id = Uuid::new_v4();
        let iter_id = Uuid::new_v4();
        let row = make_write_row(env_id, exp_id, iter_id, "control", "checkout");
        let ch_row = ClickHouseExperimentResultsRepository::to_ch_row(row).unwrap();

        // variant_stats must be valid JSON string.
        let parsed: serde_json::Value = serde_json::from_str(&ch_row.variant_stats).unwrap();
        assert_eq!(parsed["control"], serde_json::json!(100));

        // frequentist_result populated.
        let fr: serde_json::Value =
            serde_json::from_str(&ch_row.frequentist_result).unwrap();
        assert_eq!(fr["p_value"], serde_json::json!(0.03));

        // bayesian_result absent → empty string.
        assert_eq!(ch_row.bayesian_result, "");
    }

    /// `to_ch_row` with both optional JSON fields absent produces empty strings.
    #[test]
    fn to_ch_row_both_optional_absent() {
        let row = WriteResultRow {
            env_id: Uuid::new_v4(),
            experiment_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
            variant_key: "control".to_string(),
            metric_key: "revenue".to_string(),
            metric_type: "numeric".to_string(),
            variant_stats: serde_json::json!({}),
            frequentist_result: None,
            bayesian_result: None,
            recommendation: "inconclusive".to_string(),
            computed_at: Utc::now(),
        };
        let ch_row = ClickHouseExperimentResultsRepository::to_ch_row(row).unwrap();
        assert_eq!(ch_row.frequentist_result, "");
        assert_eq!(ch_row.bayesian_result, "");
    }

    /// `to_ch_row` with both optional JSON fields present serialises both.
    #[test]
    fn to_ch_row_both_optional_present() {
        let row = WriteResultRow {
            env_id: Uuid::new_v4(),
            experiment_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
            variant_key: "treatment".to_string(),
            metric_key: "revenue".to_string(),
            metric_type: "numeric".to_string(),
            variant_stats: serde_json::json!({ "mean": 42.5 }),
            frequentist_result: Some(serde_json::json!({ "ci_lower": 0.1 })),
            bayesian_result: Some(serde_json::json!({ "posterior_prob": 0.95 })),
            recommendation: "ship".to_string(),
            computed_at: Utc::now(),
        };
        let ch_row = ClickHouseExperimentResultsRepository::to_ch_row(row).unwrap();
        let fr: serde_json::Value =
            serde_json::from_str(&ch_row.frequentist_result).unwrap();
        assert_eq!(fr["ci_lower"], serde_json::json!(0.1));
        let br: serde_json::Value =
            serde_json::from_str(&ch_row.bayesian_result).unwrap();
        assert_eq!(br["posterior_prob"], serde_json::json!(0.95));
    }

    /// `to_ch_row` preserves all identity fields.
    #[test]
    fn to_ch_row_preserves_identity_fields() {
        let env_id = Uuid::new_v4();
        let exp_id = Uuid::new_v4();
        let iter_id = Uuid::new_v4();
        let row = make_write_row(env_id, exp_id, iter_id, "variant_b", "add_to_cart");
        let ch_row = ClickHouseExperimentResultsRepository::to_ch_row(row).unwrap();
        assert_eq!(ch_row.env_id, env_id);
        assert_eq!(ch_row.experiment_id, exp_id);
        assert_eq!(ch_row.iteration_id, iter_id);
        assert_eq!(ch_row.variant_key, "variant_b");
        assert_eq!(ch_row.metric_key, "add_to_cart");
        assert_eq!(ch_row.metric_type, "count");
        assert_eq!(ch_row.recommendation, "ship_treatment");
    }

    /// `to_ch_row` sets created_at to a non-zero timestamp.
    #[test]
    fn to_ch_row_sets_created_at() {
        let row = make_write_row(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "c",
            "m",
        );
        let ch_row = ClickHouseExperimentResultsRepository::to_ch_row(row).unwrap();
        assert!(ch_row.created_at > 0, "created_at must be a positive epoch ms");
    }

    /// `write` with empty slice short-circuits without touching ClickHouse.
    ///
    /// This test uses a deliberately-unreachable URL; if it were to make a
    /// network call it would fail with a connection error.
    #[tokio::test]
    async fn write_empty_slice_is_no_op() {
        let client =
            clickhouse::Client::default().with_url("http://127.0.0.1:19999");
        let repo = ClickHouseExperimentResultsRepository::new(client);
        // Should succeed immediately without attempting a network connection.
        repo.write(vec![]).await.expect("empty write must be a no-op");
    }

    /// The trait object can be constructed behind `Arc<dyn ExperimentResultsRepository>`.
    #[test]
    fn trait_object_construction() {
        use std::sync::Arc;
        let client =
            clickhouse::Client::default().with_url("http://127.0.0.1:19999");
        let repo: Arc<dyn ExperimentResultsRepository> =
            Arc::new(ClickHouseExperimentResultsRepository::new(client));
        // Simply verifying the type compiles and can be behind Arc<dyn Trait>.
        drop(repo);
    }

    /// `ResultsFilter` without iteration_id compiles and is usable.
    #[test]
    fn results_filter_no_iteration() {
        let filter = ResultsFilter {
            env_id: Uuid::new_v4(),
            experiment_id: Uuid::new_v4(),
            iteration_id: None,
        };
        assert!(filter.iteration_id.is_none());
    }

    /// `ResultsFilter` with iteration_id compiles and is usable.
    #[test]
    fn results_filter_with_iteration() {
        let iter_id = Uuid::new_v4();
        let filter = ResultsFilter {
            env_id: Uuid::new_v4(),
            experiment_id: Uuid::new_v4(),
            iteration_id: Some(iter_id),
        };
        assert_eq!(filter.iteration_id, Some(iter_id));
    }
}
