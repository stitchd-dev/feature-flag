//! Response types for the experiment results API.
//!
//! These types mirror [`stitchd_db::experiment_results::ExperimentResultRow`]
//! but expose typed fields with utoipa schema annotations for OpenAPI doc gen.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Full statistical results for one experiment iteration, covering all metrics.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ExperimentResultsResponse {
    /// The experiment these results belong to.
    pub experiment_id: Uuid,
    /// The specific iteration these results were computed for.
    pub iteration_id: Uuid,
    /// When these results were last computed.
    pub computed_at: DateTime<Utc>,
    /// Per-metric statistical results.
    pub metrics: Vec<MetricResultResponse>,
}

/// Statistical results for a single metric within an experiment iteration.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MetricResultResponse {
    /// Metric key (e.g. `"checkout_completed"`).
    pub metric_key: String,
    /// Metric type discriminant: `"count"`, `"numeric"`, `"percentile"`, or `"funnel"`.
    pub metric_type: String,
    /// Per-variant sample statistics (JSONB object).
    pub variant_stats: serde_json::Value,
    /// Frequentist analysis result, if computed; `null` for Bayesian-only runs.
    pub frequentist_result: Option<serde_json::Value>,
    /// Bayesian analysis result, if computed; `null` for frequentist-only runs.
    pub bayesian_result: Option<serde_json::Value>,
    /// Human-readable recommendation string (e.g. `"ship_treatment"`, `"needs_more_data"`).
    pub recommendation: String,
}

/// Response body for a fire-and-forget recompute request.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RecomputeResponse {
    /// Always `"accepted"` — the computation has been queued asynchronously.
    pub status: String,
    /// The experiment that will be recomputed.
    pub experiment_id: Uuid,
    /// The iteration for which results will be recomputed.
    pub iteration_id: Uuid,
}

// ---------------------------------------------------------------------------
// Conversions from DB row types
// ---------------------------------------------------------------------------

use stitchd_db::experiment_results::ExperimentResultRow;

impl MetricResultResponse {
    /// Build a `MetricResultResponse` from a persisted [`ExperimentResultRow`].
    #[must_use]
    pub fn from_row(row: &ExperimentResultRow) -> Self {
        Self {
            metric_key: row.metric_key.clone(),
            metric_type: row.metric_type.clone(),
            variant_stats: row.variant_stats.clone(),
            frequentist_result: row.frequentist_result.clone(),
            bayesian_result: row.bayesian_result.clone(),
            recommendation: row.recommendation.clone(),
        }
    }
}

impl ExperimentResultsResponse {
    /// Build an `ExperimentResultsResponse` from a non-empty slice of
    /// [`ExperimentResultRow`]s that all share the same
    /// `(experiment_id, iteration_id)`.
    ///
    /// `computed_at` is taken as the *minimum* `computed_at` across all rows
    /// (i.e. the time at which the oldest metric was last recomputed).
    ///
    /// # Panics
    /// Panics if `rows` is empty.
    #[must_use]
    pub fn from_rows(rows: &[ExperimentResultRow]) -> Self {
        assert!(!rows.is_empty(), "rows must not be empty");
        let experiment_id = rows[0].experiment_id;
        let iteration_id = rows[0].iteration_id;
        let computed_at = rows
            .iter()
            .map(|r| r.computed_at)
            .min()
            .expect("non-empty slice always has a minimum");
        let metrics = rows.iter().map(MetricResultResponse::from_row).collect();
        Self { experiment_id, iteration_id, computed_at, metrics }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_row(metric_key: &str) -> ExperimentResultRow {
        ExperimentResultRow {
            id: Uuid::new_v4(),
            experiment_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
            metric_key: metric_key.to_string(),
            metric_type: "count".to_string(),
            variant_stats: serde_json::json!({ "control": 100, "treatment": 120 }),
            frequentist_result: Some(serde_json::json!({ "p_value": 0.03 })),
            bayesian_result: None,
            recommendation: "ship_treatment".to_string(),
            computed_at: Utc::now(),
            created_at: Utc::now(),
        }
    }

    // ── Round-trip: ExperimentResultsResponse ──────────────────────────────

    #[test]
    fn experiment_results_response_roundtrip() {
        let row = make_row("checkout");
        let response = ExperimentResultsResponse::from_rows(std::slice::from_ref(&row));

        let json = serde_json::to_string(&response).expect("serialize");
        let deserialized: ExperimentResultsResponse =
            serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.experiment_id, response.experiment_id);
        assert_eq!(deserialized.iteration_id, response.iteration_id);
        assert_eq!(deserialized.metrics.len(), 1);
        assert_eq!(deserialized.metrics[0].metric_key, "checkout");
    }

    // ── Round-trip: MetricResultResponse ───────────────────────────────────

    #[test]
    fn metric_result_response_roundtrip() {
        let row = make_row("revenue");
        let metric = MetricResultResponse::from_row(&row);

        let json = serde_json::to_string(&metric).expect("serialize");
        let deserialized: MetricResultResponse = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.metric_key, "revenue");
        assert_eq!(deserialized.metric_type, "count");
        assert_eq!(deserialized.recommendation, "ship_treatment");
        assert!(deserialized.frequentist_result.is_some());
        assert!(deserialized.bayesian_result.is_none());
    }

    // ── Round-trip: RecomputeResponse ──────────────────────────────────────

    #[test]
    fn recompute_response_roundtrip() {
        let response = RecomputeResponse {
            status: "accepted".to_string(),
            experiment_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
        };

        let json = serde_json::to_string(&response).expect("serialize");
        let deserialized: RecomputeResponse = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.status, "accepted");
        assert_eq!(deserialized.experiment_id, response.experiment_id);
        assert_eq!(deserialized.iteration_id, response.iteration_id);
    }

    // ── ExperimentResultsResponse::from_rows picks minimum computed_at ─────

    #[test]
    fn from_rows_picks_minimum_computed_at() {
        let base = Uuid::new_v4();
        let iter = Uuid::new_v4();
        let earlier = Utc::now() - chrono::Duration::hours(1);
        let later = Utc::now();

        let row1 = ExperimentResultRow {
            id: Uuid::new_v4(),
            experiment_id: base,
            iteration_id: iter,
            metric_key: "m1".to_string(),
            metric_type: "count".to_string(),
            variant_stats: serde_json::json!({}),
            frequentist_result: None,
            bayesian_result: None,
            recommendation: "inconclusive".to_string(),
            computed_at: earlier,
            created_at: Utc::now(),
        };
        let row2 = ExperimentResultRow {
            id: Uuid::new_v4(),
            experiment_id: base,
            iteration_id: iter,
            metric_key: "m2".to_string(),
            metric_type: "numeric".to_string(),
            variant_stats: serde_json::json!({}),
            frequentist_result: None,
            bayesian_result: None,
            recommendation: "needs_more_data".to_string(),
            computed_at: later,
            created_at: Utc::now(),
        };

        let response = ExperimentResultsResponse::from_rows(&[row1, row2]);
        assert_eq!(response.computed_at, earlier);
        assert_eq!(response.metrics.len(), 2);
    }

    // ── MetricResultResponse preserves optional JSON fields ────────────────

    #[test]
    fn metric_result_response_with_bayesian_result() {
        let mut row = make_row("funnel_metric");
        row.bayesian_result = Some(serde_json::json!({ "prob_best": 0.92 }));
        row.frequentist_result = None;
        row.metric_type = "funnel".to_string();
        row.recommendation = "needs_more_data".to_string();

        let metric = MetricResultResponse::from_row(&row);
        assert!(metric.bayesian_result.is_some());
        assert!(metric.frequentist_result.is_none());

        let json = serde_json::to_string(&metric).expect("serialize");
        let deserialized: MetricResultResponse = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.metric_type, "funnel");
        assert!(deserialized.bayesian_result.is_some());
    }
}
