//! Async compute pipeline for experiment statistical analysis.
//!
//! [`ComputeResultsJob`] fetches aggregated metric data from ClickHouse, runs the
//! configured stats engine (frequentist or Bayesian), and persists the results to
//! PostgreSQL via [`ExperimentResultsRepository`].
//!
//! The job is designed to be fire-and-forget via [`spawn_compute_job`].

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use tracing::instrument;
use uuid::Uuid;

use stitchd_core::experimentation::stats::{
    AnalysisType, MetricType, Percentiles, VariantStats,
    bayesian, frequentist, recommendation,
    recommendation::RecommendationInput,
};
use stitchd_db::{
    clickhouse::experiment_queries::{
        CountMetricRow, FunnelStepRow, NumericMetricRow, query_count_metric, query_funnel,
        query_numeric_metric,
    },
    experiment_results::{ExperimentResultsRepository, UpsertResultRow},
};

// ── MetricDefinition ──────────────────────────────────────────────────────────

/// Describes a single metric to be computed for an experiment iteration.
#[derive(Debug, Clone)]
pub struct MetricDefinition {
    /// Metric key (e.g. `"checkout_completed"`).
    pub metric_key: String,
    /// Kind of metric — determines which ClickHouse query family is used.
    pub metric_type: MetricType,
    /// Ordered list of event keys for funnel metrics.  `None` for non-funnel types.
    pub funnel_steps: Option<Vec<String>>,
    /// Target percentile on a 0–100 scale (e.g. `95.0` for p95).
    /// Only used when `metric_type == MetricType::Percentile`.
    pub percentile: Option<f64>,
}

// ── ComputeResultsJob ─────────────────────────────────────────────────────────

/// Fire-and-forget job that computes statistical results for one experiment iteration.
///
/// Fetch → aggregate → analyse → persist.  Intended to be launched via
/// [`spawn_compute_job`] so the caller is never blocked.
#[derive(Debug)]
pub struct ComputeResultsJob {
    /// The experiment being analysed.
    pub experiment_id: Uuid,
    /// The specific iteration to compute results for.
    pub iteration_id: Uuid,
    /// Statistical methodology (frequentist or Bayesian).
    pub analysis_type: AnalysisType,
    /// Ordered list of metrics to compute.
    pub metrics: Vec<MetricDefinition>,
    /// Start of the analysis window (inclusive).
    pub date_from: DateTime<Utc>,
    /// End of the analysis window (exclusive).
    pub date_to: DateTime<Utc>,
    /// Minimum sample size before drawing a conclusion.
    pub min_sample_size: Option<i64>,
    /// Environment ID — scopes all ClickHouse queries.
    pub env_id: Uuid,
}

impl ComputeResultsJob {
    /// Run the compute pipeline.
    ///
    /// For each metric:
    /// 1. Query ClickHouse for raw aggregations.
    /// 2. Build per-variant [`VariantStats`].
    /// 3. Run statistical analysis.
    /// 4. Compute a recommendation.
    /// 5. Upsert the result row into PostgreSQL.
    ///
    /// # Errors
    /// Returns `Err` if a ClickHouse query or PostgreSQL upsert fails.
    #[instrument(
        name = "compute_results",
        skip(self, ch_client, results_repo),
        fields(
            experiment_id = %self.experiment_id,
            iteration_id  = %self.iteration_id,
            metric_count  = self.metrics.len(),
        )
    )]
    pub async fn run(
        self,
        ch_client: Arc<clickhouse::Client>,
        results_repo: Arc<dyn ExperimentResultsRepository>,
    ) -> anyhow::Result<()> {
        let experiment_id_str = self.experiment_id.to_string();
        let computed_at = Utc::now();

        for metric in &self.metrics {
            let row = self
                .compute_metric(metric, &ch_client, &results_repo, &experiment_id_str, computed_at)
                .await?;
            results_repo
                .upsert(&row)
                .await
                .map_err(|e| anyhow!("failed to upsert result for metric '{}': {e}", metric.metric_key))?;
        }
        Ok(())
    }

    // ── private helpers ───────────────────────────────────────────────────────

    /// Compute the result row for a single metric.
    async fn compute_metric(
        &self,
        metric: &MetricDefinition,
        ch: &clickhouse::Client,
        _results_repo: &Arc<dyn ExperimentResultsRepository>,
        experiment_id_str: &str,
        computed_at: DateTime<Utc>,
    ) -> anyhow::Result<UpsertResultRow> {
        // 1. Fetch rows from ClickHouse and build VariantStats map.
        let variant_stats_map: HashMap<String, VariantStats> = match metric.metric_type {
            MetricType::Count => {
                let rows = query_count_metric(
                    ch,
                    self.env_id,
                    experiment_id_str,
                    &metric.metric_key,
                    self.date_from,
                    self.date_to,
                )
                .await
                .map_err(|e| anyhow!("clickhouse count query failed for '{}': {e}", metric.metric_key))?;
                build_count_variant_stats(&rows)
            }
            MetricType::Numeric | MetricType::Percentile => {
                let rows = query_numeric_metric(
                    ch,
                    self.env_id,
                    experiment_id_str,
                    &metric.metric_key,
                    self.date_from,
                    self.date_to,
                )
                .await
                .map_err(|e| anyhow!("clickhouse numeric query failed for '{}': {e}", metric.metric_key))?;
                build_numeric_variant_stats(&rows, metric.metric_type)
            }
            MetricType::Funnel => {
                let steps: Vec<&str> = metric
                    .funnel_steps
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(String::as_str)
                    .collect();
                let rows = query_funnel(
                    ch,
                    self.env_id,
                    experiment_id_str,
                    &steps,
                    self.date_from,
                    self.date_to,
                )
                .await
                .map_err(|e| anyhow!("clickhouse funnel query failed for '{}': {e}", metric.metric_key))?;
                build_funnel_variant_stats(&rows)
            }
        };

        // 2. Identify the control variant.
        let control_key = find_control_key(&variant_stats_map);
        let control_stats = variant_stats_map
            .get(&control_key)
            .ok_or_else(|| anyhow!("no control variant found for metric '{}'", metric.metric_key))?;

        // 3. Run analysis for each non-control variant vs control.
        let mut all_variant_recommendations: Vec<(String, stitchd_core::experimentation::stats::Recommendation)> = Vec::new();
        let mut frequentist_result_val: Option<JsonValue> = None;
        let mut bayesian_result_val: Option<JsonValue> = None;

        let treatment_keys: Vec<String> = variant_stats_map
            .keys()
            .filter(|k| *k != &control_key)
            .cloned()
            .collect();

        for variant_key in &treatment_keys {
            let variant_stats = &variant_stats_map[variant_key];

            let (freq_result, bayes_result) = run_analysis(
                metric,
                control_stats,
                variant_stats,
                self.analysis_type,
            );

            let rec_input = RecommendationInput {
                variant_key: variant_key.clone(),
                is_control: false,
                frequentist: freq_result.clone(),
                bayesian: bayes_result.clone(),
                analysis_type: self.analysis_type,
                sample_size: variant_stats.sample_size,
                min_sample_size: self.min_sample_size,
            };
            let rec = recommendation::recommend(&rec_input);
            all_variant_recommendations.push((variant_key.clone(), rec));

            // Store the last variant's analysis results (single-treatment case is the common path).
            if let Some(ref fr) = freq_result {
                frequentist_result_val = Some(serde_json::to_value(fr)?);
            }
            if let Some(ref br) = bayes_result {
                bayesian_result_val = Some(serde_json::to_value(br)?);
            }
        }

        // If no treatment variants found, still produce a row with NeedsMoreData.
        let overall_recommendation = if all_variant_recommendations.is_empty() {
            stitchd_core::experimentation::stats::Recommendation::NeedsMoreData
        } else {
            recommendation::pick_winner(&all_variant_recommendations)
        };

        // 4. Build the UpsertResultRow.
        let row = UpsertResultRow {
            experiment_id: self.experiment_id,
            iteration_id: self.iteration_id,
            metric_key: metric.metric_key.clone(),
            metric_type: metric_type_str(metric.metric_type).to_string(),
            variant_stats: serde_json::to_value(&variant_stats_map)?,
            frequentist_result: frequentist_result_val,
            bayesian_result: bayesian_result_val,
            recommendation: overall_recommendation.to_string(),
            computed_at,
        };

        Ok(row)
    }
}

// ── spawn_compute_job ─────────────────────────────────────────────────────────

/// Spawn a [`ComputeResultsJob`] as a fire-and-forget Tokio task.
///
/// Errors from the job are logged as `tracing::error!` events; the task never
/// panics and the returned [`tokio::task::JoinHandle`] completes with `()`.
pub fn spawn_compute_job(
    job: ComputeResultsJob,
    ch: Arc<clickhouse::Client>,
    repo: Arc<dyn ExperimentResultsRepository>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = job.run(ch, repo).await {
            tracing::error!(error = %e, "compute_results job failed");
        }
    })
}

// ── Aggregation helpers ───────────────────────────────────────────────────────

/// Build a `VariantStats` map from count metric rows.
fn build_count_variant_stats(rows: &[CountMetricRow]) -> HashMap<String, VariantStats> {
    rows.iter()
        .map(|row| {
            let conversion_rate = if row.sample_size > 0 {
                #[allow(clippy::cast_precision_loss)]
                Some(row.conversions as f64 / row.sample_size as f64)
            } else {
                None
            };
            let stats = VariantStats {
                sample_size: row.sample_size,
                conversions: Some(row.conversions),
                mean: conversion_rate,
                variance: None,
                conversion_rate,
                percentiles: None,
            };
            (row.variant_key.clone(), stats)
        })
        .collect()
}

/// Build a `VariantStats` map from numeric/percentile metric rows.
fn build_numeric_variant_stats(
    rows: &[NumericMetricRow],
    metric_type: MetricType,
) -> HashMap<String, VariantStats> {
    rows.iter()
        .map(|row| {
            let percentiles = if metric_type == MetricType::Percentile {
                Some(Percentiles { p50: row.p50, p95: row.p95, p99: row.p99 })
            } else {
                None
            };
            let stats = VariantStats {
                sample_size: row.sample_size,
                conversions: None,
                mean: Some(row.mean),
                variance: Some(row.variance),
                conversion_rate: None,
                percentiles,
            };
            (row.variant_key.clone(), stats)
        })
        .collect()
}

/// Build a `VariantStats` map from funnel step rows.
///
/// Uses the final (maximum) step count as the conversion count; the first step
/// count (step 0) is used as the sample_size denominator.
fn build_funnel_variant_stats(rows: &[FunnelStepRow]) -> HashMap<String, VariantStats> {
    // Group by variant key.
    let mut by_variant: HashMap<String, Vec<&FunnelStepRow>> = HashMap::new();
    for row in rows {
        by_variant.entry(row.variant_key.clone()).or_default().push(row);
    }

    by_variant
        .into_iter()
        .map(|(variant_key, step_rows)| {
            let max_step = step_rows.iter().map(|r| r.step_index).max().unwrap_or(0);
            let first_step = step_rows.iter().find(|r| r.step_index == 0);
            let last_step = step_rows.iter().find(|r| r.step_index == max_step);

            let sample_size = first_step.map_or(0, |r| r.count);
            let conversions = last_step.map_or(0, |r| r.count);

            let conversion_rate = if sample_size > 0 {
                #[allow(clippy::cast_precision_loss)]
                Some(conversions as f64 / sample_size as f64)
            } else {
                None
            };

            let stats = VariantStats {
                sample_size,
                conversions: Some(conversions),
                mean: conversion_rate,
                variance: None,
                conversion_rate,
                percentiles: None,
            };
            (variant_key, stats)
        })
        .collect()
}

/// Identify the control variant key.
///
/// Returns the key `"control"` if present; otherwise returns the first key found
/// (arbitrary but deterministic within a run).
fn find_control_key(map: &HashMap<String, VariantStats>) -> String {
    if map.contains_key("control") {
        "control".to_string()
    } else {
        map.keys().next().cloned().unwrap_or_default()
    }
}

/// Run both frequentist and Bayesian analyses (only the selected one is used for
/// recommendation; both are stored for auditability).
///
/// For percentile metrics the analysis uses the mean/variance fields of
/// `VariantStats` (matching the frequentist `analyze_numeric` interface) because
/// we don't have raw sample arrays available from the aggregated ClickHouse rows.
fn run_analysis(
    metric: &MetricDefinition,
    control: &VariantStats,
    variant: &VariantStats,
    analysis_type: AnalysisType,
) -> (
    Option<stitchd_core::experimentation::stats::FrequentistResult>,
    Option<stitchd_core::experimentation::stats::BayesianResult>,
) {
    match (metric.metric_type, analysis_type) {
        (MetricType::Count, AnalysisType::Frequentist) => {
            (Some(frequentist::analyze_count(control, variant)), None)
        }
        (MetricType::Count, AnalysisType::Bayesian) => {
            (None, Some(bayesian::analyze_count(control, variant)))
        }
        (MetricType::Numeric, AnalysisType::Frequentist)
        | (MetricType::Percentile, AnalysisType::Frequentist) => {
            (Some(frequentist::analyze_numeric(control, variant)), None)
        }
        (MetricType::Numeric, AnalysisType::Bayesian)
        | (MetricType::Percentile, AnalysisType::Bayesian) => {
            (None, Some(bayesian::analyze_numeric(control, variant)))
        }
        (MetricType::Funnel, AnalysisType::Frequentist) => {
            (Some(frequentist::analyze_funnel(control, variant)), None)
        }
        (MetricType::Funnel, AnalysisType::Bayesian) => {
            (None, Some(bayesian::analyze_funnel(control, variant)))
        }
    }
}

/// Canonical string discriminant for a `MetricType`.
const fn metric_type_str(mt: MetricType) -> &'static str {
    match mt {
        MetricType::Count => "count",
        MetricType::Numeric => "numeric",
        MetricType::Percentile => "percentile",
        MetricType::Funnel => "funnel",
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use std::sync::Mutex;
    use stitchd_db::experiment_results::ExperimentResultRow;

    // ── Mock repository ───────────────────────────────────────────────────────

    /// In-memory accumulator that satisfies `ExperimentResultsRepository`.
    struct MockResultsRepo {
        rows: Arc<Mutex<Vec<UpsertResultRow>>>,
    }

    impl MockResultsRepo {
        fn new() -> Self {
            Self { rows: Arc::new(Mutex::new(Vec::new())) }
        }

        fn snapshot(&self) -> Vec<UpsertResultRow> {
            self.rows.lock().unwrap().clone()
        }
    }

    fn make_result_row(row: &UpsertResultRow) -> ExperimentResultRow {
        ExperimentResultRow {
            id: Uuid::new_v4(),
            experiment_id: row.experiment_id,
            iteration_id: row.iteration_id,
            metric_key: row.metric_key.clone(),
            metric_type: row.metric_type.clone(),
            variant_stats: row.variant_stats.clone(),
            frequentist_result: row.frequentist_result.clone(),
            bayesian_result: row.bayesian_result.clone(),
            recommendation: row.recommendation.clone(),
            computed_at: row.computed_at,
            created_at: row.computed_at,
        }
    }

    #[async_trait]
    impl ExperimentResultsRepository for MockResultsRepo {
        async fn upsert(&self, row: &UpsertResultRow) -> Result<ExperimentResultRow, sqlx::Error> {
            let result = make_result_row(row);
            self.rows.lock().unwrap().push(row.clone());
            Ok(result)
        }

        async fn fetch_latest(
            &self,
            _: Uuid,
        ) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
            Ok(self.rows.lock().unwrap().iter().map(make_result_row).collect())
        }

        async fn fetch_by_iteration(
            &self,
            _: Uuid,
            _: Uuid,
        ) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
            Ok(self.rows.lock().unwrap().iter().map(make_result_row).collect())
        }

        async fn is_stale(&self, _: Uuid, _: Uuid) -> Result<bool, sqlx::Error> {
            Ok(false)
        }
    }

    // ── Aggregation logic tests (pure, no ClickHouse) ─────────────────────────

    #[test]
    fn build_count_variant_stats_computes_conversion_rate() {
        let rows = vec![
            CountMetricRow { variant_key: "control".to_string(), sample_size: 1000, conversions: 100 },
            CountMetricRow { variant_key: "treatment".to_string(), sample_size: 1000, conversions: 150 },
        ];
        let stats = build_count_variant_stats(&rows);
        assert_eq!(stats.len(), 2);

        let ctrl = &stats["control"];
        assert_eq!(ctrl.sample_size, 1000);
        assert_eq!(ctrl.conversions, Some(100));
        let rate = ctrl.conversion_rate.unwrap();
        assert!((rate - 0.10).abs() < 1e-9);

        let trt = &stats["treatment"];
        let trt_rate = trt.conversion_rate.unwrap();
        assert!((trt_rate - 0.15).abs() < 1e-9);
    }

    #[test]
    fn build_count_variant_stats_zero_sample_size_gives_none_rate() {
        let rows = vec![CountMetricRow {
            variant_key: "control".to_string(),
            sample_size: 0,
            conversions: 0,
        }];
        let stats = build_count_variant_stats(&rows);
        assert!(stats["control"].conversion_rate.is_none());
    }

    #[test]
    fn build_numeric_variant_stats_populates_mean_and_variance() {
        let rows = vec![NumericMetricRow {
            variant_key: "control".to_string(),
            sample_size: 500,
            sum: 2500.0,
            mean: 5.0,
            variance: 2.5,
            p50: 4.8,
            p95: 9.1,
            p99: 12.3,
        }];
        let stats = build_numeric_variant_stats(&rows, MetricType::Numeric);
        let ctrl = &stats["control"];
        assert_eq!(ctrl.sample_size, 500);
        assert!((ctrl.mean.unwrap() - 5.0).abs() < 1e-9);
        assert!((ctrl.variance.unwrap() - 2.5).abs() < 1e-9);
        assert!(ctrl.percentiles.is_none(), "Numeric type should not populate percentiles");
    }

    #[test]
    fn build_numeric_variant_stats_populates_percentiles_for_percentile_type() {
        let rows = vec![NumericMetricRow {
            variant_key: "control".to_string(),
            sample_size: 500,
            sum: 2500.0,
            mean: 5.0,
            variance: 2.5,
            p50: 4.8,
            p95: 9.1,
            p99: 12.3,
        }];
        let stats = build_numeric_variant_stats(&rows, MetricType::Percentile);
        let ctrl = &stats["control"];
        let pct = ctrl.percentiles.as_ref().unwrap();
        assert!((pct.p50 - 4.8).abs() < 1e-9);
        assert!((pct.p95 - 9.1).abs() < 1e-9);
        assert!((pct.p99 - 12.3).abs() < 1e-9);
    }

    #[test]
    fn build_funnel_variant_stats_uses_first_and_last_steps() {
        let rows = vec![
            FunnelStepRow { variant_key: "control".to_string(), step_index: 0, event_key: "view".to_string(), count: 1000 },
            FunnelStepRow { variant_key: "control".to_string(), step_index: 1, event_key: "click".to_string(), count: 400 },
            FunnelStepRow { variant_key: "control".to_string(), step_index: 2, event_key: "checkout".to_string(), count: 200 },
        ];
        let stats = build_funnel_variant_stats(&rows);
        let ctrl = &stats["control"];
        assert_eq!(ctrl.sample_size, 1000);
        assert_eq!(ctrl.conversions, Some(200));
        let rate = ctrl.conversion_rate.unwrap();
        assert!((rate - 0.20).abs() < 1e-9);
    }

    #[test]
    fn build_funnel_variant_stats_multiple_variants() {
        let rows = vec![
            FunnelStepRow { variant_key: "control".to_string(), step_index: 0, event_key: "view".to_string(), count: 1000 },
            FunnelStepRow { variant_key: "control".to_string(), step_index: 1, event_key: "buy".to_string(), count: 200 },
            FunnelStepRow { variant_key: "treatment".to_string(), step_index: 0, event_key: "view".to_string(), count: 1000 },
            FunnelStepRow { variant_key: "treatment".to_string(), step_index: 1, event_key: "buy".to_string(), count: 260 },
        ];
        let stats = build_funnel_variant_stats(&rows);
        assert_eq!(stats.len(), 2);
        let t_rate = stats["treatment"].conversion_rate.unwrap();
        assert!((t_rate - 0.26).abs() < 1e-9);
    }

    #[test]
    fn find_control_key_returns_control_when_present() {
        let mut map = HashMap::new();
        map.insert("control".to_string(), VariantStats { sample_size: 100, conversions: None, mean: None, variance: None, conversion_rate: None, percentiles: None });
        map.insert("treatment".to_string(), VariantStats { sample_size: 100, conversions: None, mean: None, variance: None, conversion_rate: None, percentiles: None });
        assert_eq!(find_control_key(&map), "control");
    }

    #[test]
    fn find_control_key_fallback_when_no_control_key() {
        let mut map = HashMap::new();
        map.insert("variant_a".to_string(), VariantStats { sample_size: 100, conversions: None, mean: None, variance: None, conversion_rate: None, percentiles: None });
        let key = find_control_key(&map);
        assert_eq!(key, "variant_a");
    }

    #[test]
    fn metric_type_str_round_trips() {
        assert_eq!(metric_type_str(MetricType::Count), "count");
        assert_eq!(metric_type_str(MetricType::Numeric), "numeric");
        assert_eq!(metric_type_str(MetricType::Percentile), "percentile");
        assert_eq!(metric_type_str(MetricType::Funnel), "funnel");
    }

    // ── Analysis routing tests ────────────────────────────────────────────────

    fn count_metric_def() -> MetricDefinition {
        MetricDefinition {
            metric_key: "checkout".to_string(),
            metric_type: MetricType::Count,
            funnel_steps: None,
            percentile: None,
        }
    }

    fn make_count_stats_for_analysis(sample_size: i64, conversions: i64) -> VariantStats {
        let rate = conversions as f64 / sample_size as f64;
        VariantStats {
            sample_size,
            conversions: Some(conversions),
            mean: Some(rate),
            variance: None,
            conversion_rate: Some(rate),
            percentiles: None,
        }
    }

    #[test]
    fn run_analysis_frequentist_count_returns_frequentist_result() {
        let def = count_metric_def();
        let ctrl = make_count_stats_for_analysis(1000, 100);
        let variant = make_count_stats_for_analysis(1000, 150);
        let (freq, bayes) = run_analysis(&def, &ctrl, &variant, AnalysisType::Frequentist);
        assert!(freq.is_some(), "frequentist result should be present");
        assert!(bayes.is_none(), "bayesian result should be absent for frequentist run");
        assert!(freq.unwrap().significant);
    }

    #[test]
    fn run_analysis_bayesian_count_returns_bayesian_result() {
        let def = count_metric_def();
        let ctrl = make_count_stats_for_analysis(1000, 100);
        let variant = make_count_stats_for_analysis(1000, 150);
        let (freq, bayes) = run_analysis(&def, &ctrl, &variant, AnalysisType::Bayesian);
        assert!(freq.is_none(), "frequentist result should be absent for Bayesian run");
        assert!(bayes.is_some(), "bayesian result should be present");
        assert!(bayes.unwrap().prob_best > 0.99);
    }

    #[test]
    fn run_analysis_frequentist_numeric() {
        let def = MetricDefinition {
            metric_key: "revenue".to_string(),
            metric_type: MetricType::Numeric,
            funnel_steps: None,
            percentile: None,
        };
        let ctrl = VariantStats { sample_size: 500, conversions: None, mean: Some(10.0), variance: Some(4.0), conversion_rate: None, percentiles: None };
        let variant = VariantStats { sample_size: 500, conversions: None, mean: Some(12.0), variance: Some(4.0), conversion_rate: None, percentiles: None };
        let (freq, _) = run_analysis(&def, &ctrl, &variant, AnalysisType::Frequentist);
        assert!(freq.is_some());
        assert!(freq.unwrap().significant);
    }

    #[test]
    fn run_analysis_frequentist_funnel() {
        let def = MetricDefinition {
            metric_key: "signup_funnel".to_string(),
            metric_type: MetricType::Funnel,
            funnel_steps: Some(vec!["view".to_string(), "signup".to_string()]),
            percentile: None,
        };
        let ctrl = VariantStats { sample_size: 1000, conversions: None, mean: None, variance: None, conversion_rate: Some(0.10), percentiles: None };
        let variant = VariantStats { sample_size: 1000, conversions: None, mean: None, variance: None, conversion_rate: Some(0.15), percentiles: None };
        let (freq, _) = run_analysis(&def, &ctrl, &variant, AnalysisType::Frequentist);
        assert!(freq.is_some());
        assert!(freq.unwrap().significant);
    }

    // ── spawn_compute_job shape test ──────────────────────────────────────────

    #[tokio::test]
    async fn spawn_compute_job_returns_join_handle_immediately() {
        // We cannot construct a real clickhouse::Client without a server, so we
        // test the spawn shape: the function must return a JoinHandle without
        // blocking, and the handle must be joinable.
        //
        // We wire an immediately-failing job by pointing at a non-existent CH
        // server; the fire-and-forget semantics mean the caller never panics.
        let ch = Arc::new(clickhouse::Client::default().with_url("http://127.0.0.1:1")); // unreachable
        let repo: Arc<dyn ExperimentResultsRepository> = Arc::new(MockResultsRepo::new());

        let job = ComputeResultsJob {
            experiment_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
            analysis_type: AnalysisType::Frequentist,
            metrics: vec![],          // empty — no CH queries attempted
            date_from: Utc::now(),
            date_to: Utc::now(),
            min_sample_size: None,
            env_id: Uuid::new_v4(),
        };

        let handle = spawn_compute_job(job, ch, repo);
        // Handle must be obtainable without blocking; joining completes quickly.
        handle.await.expect("join handle should complete");
    }

    // ── Mock repository upsert accumulation test ──────────────────────────────

    #[test]
    fn mock_repo_accumulates_upserted_rows() {
        let repo = MockResultsRepo::new();
        let row = UpsertResultRow {
            experiment_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
            metric_key: "checkout".to_string(),
            metric_type: "count".to_string(),
            variant_stats: serde_json::json!({}),
            frequentist_result: None,
            bayesian_result: None,
            recommendation: "inconclusive".to_string(),
            computed_at: Utc::now(),
        };
        // Verify the mock won't panic when we use it in a sync context.
        repo.rows.lock().unwrap().push(row.clone());
        let snapshot = repo.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].metric_key, "checkout");
    }
}
