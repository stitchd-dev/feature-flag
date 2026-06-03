//! Live per-metric statistics compute pass (Phase 3).
//!
//! This is the engine the scheduled stats-service runs for every running
//! experiment on each tick. For each `(unit_context_type, metric)` it:
//!
//! 1. queries the ITT **sample-size** per `(context_type, variant_key)` from
//!    `experiment_assignments` (deduped via `FINAL`) — units with no events
//!    still count;
//! 2. queries the per-`(context_type, variant_key)` **sufficient statistics**
//!    (conversion `successes`, continuous `Σx` / `Σx²`, ratio second moments, or
//!    funnel `successes`) via [`crate::queries::variant_stats`];
//! 3. assembles per-variant [`VariantStats`] (count / numeric / funnel /
//!    percentile) or [`RatioGroupStats`] (ratio), zero-filling non-firing units;
//! 4. picks a control (`"control"` if present, else the lexicographically
//!    smallest variant key);
//! 5. runs the **frequentist** (vs control, Bonferroni-corrected across the
//!    `K−1` comparisons), **bayesian** (vs control), and — when the experiment
//!    enabled it — **sequential** (always-valid mSPRT) analyses;
//! 6. runs an **SRM** chi-square once per `context_type` over the primary
//!    metric's assignment counts;
//! 7. derives a per-variant [`Recommendation`] and an overall winner; and
//! 8. assembles the per-`(metric_key, context_type)` JSON blobs and builds the
//!    [`MetricSummary`] list the [`crate::results_writer`] forwards to
//!    analytics-service.
//!
//! ## Ratio frequentist (delta method)
//!
//! There is no `frequentist::analyze_ratio`, so the ratio contrast is computed
//! inline via [`ratio_frequentist`] using the SAME delta-method formula as
//! `stats::sequential::sequential_ratio` / `interaction::ratio`
//! (`Var(R) ≈ (var_num − 2R·cov + R²·var_den) / (mean_den²·n)`, diff-of-ratios
//! `SE = sqrt(Var(R_t) + Var(R_c))`, two-tailed normal `z`).
//!
//! ## Percentile metrics
//!
//! P50/P90/P99 carry only the point value (the sufficient-stats path cannot
//! reconstruct the raw sample a percentile CI needs); they are emitted with a
//! `NeedsMoreData` recommendation and NO frequentist/bayesian/sequential blob.
//! A bootstrap-based percentile significance test is out of scope here.
//!
//! ## CUPED (deferred)
//!
//! `pre_period_days` is captured on [`crate::scheduler::RunningExperiment`] but
//! intentionally unused in this pass — CUPED variance reduction is tracked as a
//! follow-up (it needs a pre-period per-context fetch + a `cuped_fetch`
//! legacy-column fix).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use clickhouse::Client;
use serde_json::Value;
use uuid::Uuid;

use stitchd_core::experimentation::stats::sequential::RatioGroupStats;
use stitchd_core::experimentation::stats::srm::{SrmObservation, compute_srm};
use stitchd_core::experimentation::stats::{
    AnalysisType, BayesianResult, ConfidenceInterval, FrequentistResult, MetricType, Percentiles,
    Recommendation, VariantStats, bayesian, frequentist,
    recommendation::{RecommendationInput, pick_winner, recommend},
};
use stitchd_core::metric::{
    AggregationConfig, AggregationOperator, MetricDefinition, MetricKind, RatioConfig,
};

use crate::dispatch::rewrite_placeholders_to_clickhouse;
use crate::queries::QueryBind;
use crate::queries::variant_stats::{
    build_aggregation_cells_query, build_assignment_counts_query, build_funnel_cells_query,
    build_ratio_cells_query,
};
use crate::results_writer::{MetricSummary, VariantPoint};
use crate::scheduler::RunningExperiment;
use crate::sequential_compute::{
    SequentialMetricFamily, compute_sequential_blob, compute_sequential_blob_ratio,
    default_tau_squared, fetch_prev_always_valid_p, resolve_sequential_config,
};

// ── CH row shapes ────────────────────────────────────────────────────────────

/// Aggregation sufficient-stats row per `variant_key` (the query is already
/// scoped to one `context_type`, so the column is not re-projected).
#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChAggregationRow {
    variant_key: String,
    n: u64,
    successes: u64,
    value_sum: f64,
    value_sq_sum: f64,
}

/// Ratio sufficient-stats row (delta-method second moments) per `variant_key`.
#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChRatioRow {
    variant_key: String,
    n: u64,
    num_sum: f64,
    den_sum: f64,
    num_sq_sum: f64,
    den_sq_sum: f64,
    num_den_sum: f64,
}

/// Funnel sufficient-stats row (binary final-step path) per `variant_key`.
#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChFunnelRow {
    variant_key: String,
    n: u64,
    successes: u64,
}

/// Assignment-count row — the ITT sample-size denominator + SRM observed count.
#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChAssignmentCountRow {
    variant_key: String,
    n: u64,
}

// ── Cell reader (CH-backed, mockable) ────────────────────────────────────────

/// Reads per-`(context_type, variant_key)` sufficient statistics + assignment
/// counts for one experiment + iteration, one method per metric family.
///
/// Hidden behind a trait so [`run_stats_compute`] is unit-testable with an
/// in-memory fake; the production wiring is [`ClickHouseCellReader`].
#[async_trait]
pub trait CellReader: Send + Sync {
    /// Assignment counts per `(context_type, variant_key)` (ITT denominator).
    async fn assignment_counts(
        &self,
        experiment_id: Uuid,
        iteration_id: Uuid,
        env_id: Uuid,
        context_type: &str,
        iteration_end: DateTime<Utc>,
    ) -> Result<Vec<(String, u64)>, anyhow::Error>;

    /// Aggregation sufficient stats per `(context_type, variant_key)`.
    /// Returns `variant_key → (n, successes, value_sum, value_sq_sum)`.
    async fn aggregation_cells(
        &self,
        cfg: &AggregationConfig,
        experiment_id: Uuid,
        iteration_id: Uuid,
        env_id: Uuid,
        context_type: &str,
        iteration_end: DateTime<Utc>,
    ) -> Result<HashMap<String, AggCell>, anyhow::Error>;

    /// Ratio sufficient stats (delta-method second moments) per variant.
    #[allow(clippy::too_many_arguments)]
    async fn ratio_cells(
        &self,
        num_cfg: &AggregationConfig,
        den_cfg: &AggregationConfig,
        experiment_id: Uuid,
        iteration_id: Uuid,
        env_id: Uuid,
        context_type: &str,
        iteration_end: DateTime<Utc>,
    ) -> Result<HashMap<String, RatioGroupStats>, anyhow::Error>;

    /// Funnel sufficient stats per variant. Returns `variant_key → (n,
    /// successes)`.
    async fn funnel_cells(
        &self,
        cfg: &stitchd_core::metric::FunnelConfig,
        experiment_id: Uuid,
        iteration_id: Uuid,
        env_id: Uuid,
        context_type: &str,
        iteration_end: DateTime<Utc>,
    ) -> Result<HashMap<String, (u64, u64)>, anyhow::Error>;
}

/// Aggregation cell: the event-side sufficient statistics for one variant
/// within a `context_type` (the ITT `n` comes from the assignment count, NOT
/// from this struct's `event_n`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AggCell {
    /// Number of assigned units (matches the assignment count; carried for
    /// cross-checking).
    pub event_n: u64,
    /// Units with ≥1 qualifying event (conversion successes).
    pub successes: u64,
    /// Σ per-unit metric value (continuous).
    pub value_sum: f64,
    /// Σ per-unit metric value² (continuous).
    pub value_sq_sum: f64,
}

/// Production [`CellReader`] over a `clickhouse::Client`.
pub struct ClickHouseCellReader {
    client: Arc<Client>,
}

impl ClickHouseCellReader {
    /// Wrap a shared CH client.
    #[must_use]
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

/// Rewrite a [`crate::queries::BuiltQuery`]'s placeholders to `?` and bind its
/// values onto a fresh CH query (mirrors `interaction_compute::bind_query`).
fn bind_query(client: &Client, sql: String, binds: Vec<QueryBind>) -> clickhouse::query::Query {
    let sql = rewrite_placeholders_to_clickhouse(sql);
    let mut query = client.query(&sql);
    for b in binds {
        query = match b {
            QueryBind::Str(s) => query.bind(s),
            QueryBind::I64(n) => query.bind(n),
            QueryBind::F64(f) => query.bind(f),
        };
    }
    query
}

#[async_trait]
impl CellReader for ClickHouseCellReader {
    async fn assignment_counts(
        &self,
        experiment_id: Uuid,
        iteration_id: Uuid,
        env_id: Uuid,
        context_type: &str,
        iteration_end: DateTime<Utc>,
    ) -> Result<Vec<(String, u64)>, anyhow::Error> {
        let built = build_assignment_counts_query(
            &experiment_id.to_string(),
            &iteration_id.to_string(),
            &env_id.to_string(),
            context_type,
            iteration_end,
        );
        let rows = bind_query(&self.client, built.sql, built.binds)
            .fetch_all::<ChAssignmentCountRow>()
            .await?;
        Ok(rows.into_iter().map(|r| (r.variant_key, r.n)).collect())
    }

    async fn aggregation_cells(
        &self,
        cfg: &AggregationConfig,
        experiment_id: Uuid,
        iteration_id: Uuid,
        env_id: Uuid,
        context_type: &str,
        iteration_end: DateTime<Utc>,
    ) -> Result<HashMap<String, AggCell>, anyhow::Error> {
        let built = build_aggregation_cells_query(
            cfg,
            &experiment_id.to_string(),
            &iteration_id.to_string(),
            &env_id.to_string(),
            context_type,
            iteration_end,
        )?;
        let rows = bind_query(&self.client, built.sql, built.binds)
            .fetch_all::<ChAggregationRow>()
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.variant_key,
                    AggCell {
                        event_n: r.n,
                        successes: r.successes,
                        value_sum: r.value_sum,
                        value_sq_sum: r.value_sq_sum,
                    },
                )
            })
            .collect())
    }

    async fn ratio_cells(
        &self,
        num_cfg: &AggregationConfig,
        den_cfg: &AggregationConfig,
        experiment_id: Uuid,
        iteration_id: Uuid,
        env_id: Uuid,
        context_type: &str,
        iteration_end: DateTime<Utc>,
    ) -> Result<HashMap<String, RatioGroupStats>, anyhow::Error> {
        let built = build_ratio_cells_query(
            num_cfg,
            den_cfg,
            &experiment_id.to_string(),
            &iteration_id.to_string(),
            &env_id.to_string(),
            context_type,
            iteration_end,
        )?;
        let rows = bind_query(&self.client, built.sql, built.binds)
            .fetch_all::<ChRatioRow>()
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.variant_key,
                    RatioGroupStats {
                        n: r.n as i64,
                        num_sum: r.num_sum,
                        den_sum: r.den_sum,
                        num_sq_sum: r.num_sq_sum,
                        den_sq_sum: r.den_sq_sum,
                        num_den_sum: r.num_den_sum,
                    },
                )
            })
            .collect())
    }

    async fn funnel_cells(
        &self,
        cfg: &stitchd_core::metric::FunnelConfig,
        experiment_id: Uuid,
        iteration_id: Uuid,
        env_id: Uuid,
        context_type: &str,
        iteration_end: DateTime<Utc>,
    ) -> Result<HashMap<String, (u64, u64)>, anyhow::Error> {
        let built = build_funnel_cells_query(
            cfg,
            &experiment_id.to_string(),
            &iteration_id.to_string(),
            &env_id.to_string(),
            context_type,
            iteration_end,
        )?;
        let rows = bind_query(&self.client, built.sql, built.binds)
            .fetch_all::<ChFunnelRow>()
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.variant_key, (r.n, r.successes)))
            .collect())
    }
}

// ── Metric-type classification ───────────────────────────────────────────────

/// The analysis [`MetricType`] for a metric definition.
///
/// Aggregation `count`/`uniq` → `Count`; `sum`/`avg` → `Numeric`;
/// `p50`/`p90`/`p99` → `Percentile`. Ratio metrics map to `Numeric` (the
/// delta-method contrast is a mean-difference-style normal test); funnel →
/// `Funnel`.
#[must_use]
pub fn metric_type_for(def: &MetricDefinition) -> MetricType {
    match &def.kind {
        MetricKind::Aggregation(cfg) => match cfg.aggregator {
            AggregationOperator::Count | AggregationOperator::Uniq => MetricType::Count,
            AggregationOperator::Sum | AggregationOperator::Avg => MetricType::Numeric,
            AggregationOperator::P50 | AggregationOperator::P90 | AggregationOperator::P99 => {
                MetricType::Percentile
            }
        },
        MetricKind::Ratio(_) => MetricType::Numeric,
        MetricKind::Funnel(_) => MetricType::Funnel,
    }
}

/// The lowercase discriminant string for the sequential-family lookup
/// ([`SequentialMetricFamily::from_metric_type`]) corresponding to a
/// [`MetricType`].
#[must_use]
fn sequential_family_str(mt: MetricType) -> &'static str {
    match mt {
        MetricType::Count => "count",
        MetricType::Numeric => "numeric",
        MetricType::Funnel => "funnel",
        MetricType::Percentile => "percentile",
    }
}

// ── VariantStats construction (pure) ─────────────────────────────────────────

/// Build a count/conversion [`VariantStats`]: `conversions = successes`,
/// `sample_size`, `conversion_rate = successes / sample_size`.
#[must_use]
pub fn count_variant_stats(sample_size: u64, successes: u64) -> VariantStats {
    let n = sample_size as i64;
    let rate = if sample_size == 0 {
        0.0
    } else {
        successes as f64 / sample_size as f64
    };
    VariantStats {
        sample_size: n,
        conversions: Some(successes as i64),
        mean: None,
        variance: None,
        conversion_rate: Some(rate),
        percentiles: None,
    }
}

/// Build a numeric (sum/avg) [`VariantStats`] from ITT sufficient statistics.
///
/// `mean = value_sum / sample_size`; population variance `value_sq_sum/n − mean²`
/// converted to the **sample** variance `· n/(n−1)` for `n > 1` (matching the
/// Welch t-test's expectation of a sample variance). Non-firing units
/// contribute `0` to both `value_sum` and `value_sq_sum` (ITT).
#[must_use]
pub fn numeric_variant_stats(sample_size: u64, value_sum: f64, value_sq_sum: f64) -> VariantStats {
    let n = sample_size as f64;
    let mean = if sample_size == 0 { 0.0 } else { value_sum / n };
    let variance = if sample_size > 1 {
        let pop_var = (value_sq_sum / n - mean * mean).max(0.0);
        pop_var * (n / (n - 1.0))
    } else {
        0.0
    };
    VariantStats {
        sample_size: sample_size as i64,
        conversions: None,
        mean: Some(mean),
        variance: Some(variance),
        conversion_rate: None,
        percentiles: None,
    }
}

/// Build a funnel [`VariantStats`]: `sample_size` = top-of-funnel assigned
/// units, `conversions = successes` (reached final step), `conversion_rate =
/// successes / sample_size`.
#[must_use]
pub fn funnel_variant_stats(sample_size: u64, successes: u64) -> VariantStats {
    let rate = if sample_size == 0 {
        0.0
    } else {
        successes as f64 / sample_size as f64
    };
    VariantStats {
        sample_size: sample_size as i64,
        conversions: Some(successes as i64),
        mean: None,
        variance: None,
        conversion_rate: Some(rate),
        percentiles: None,
    }
}

/// Build a percentile [`VariantStats`] carrying only the point value (the same
/// quantile is stored in p50/p95/p99 — the sufficient-stats path cannot
/// reconstruct distinct percentiles, and significance is skipped for this
/// family anyway).
#[must_use]
pub fn percentile_variant_stats(sample_size: u64, point: f64) -> VariantStats {
    VariantStats {
        sample_size: sample_size as i64,
        conversions: None,
        mean: Some(point),
        variance: None,
        conversion_rate: None,
        percentiles: Some(Percentiles {
            p50: point,
            p95: point,
            p99: point,
        }),
    }
}

// ── Control selection ────────────────────────────────────────────────────────

/// Pick the control variant key: `"control"` when present in `variant_keys`,
/// else the lexicographically smallest key. Returns `None` for an empty set.
#[must_use]
pub fn select_control(variant_keys: &[String]) -> Option<String> {
    if variant_keys.iter().any(|k| k == "control") {
        return Some("control".to_string());
    }
    variant_keys.iter().min().cloned()
}

// ── Ratio frequentist (delta method) ─────────────────────────────────────────

/// Delta-method point estimate `R` and variance `Var(R)` for a ratio group —
/// identical to `RatioGroupStats::ratio_var` (which is private to stitchd-core),
/// reproduced here so the stats-service can compute the frequentist ratio
/// contrast without an `analyze_ratio` in core. Returns `None` for a degenerate
/// group (`n < 2`, `den_sum ≤ 0`, `mean_den ≤ 0`, or a non-finite/non-positive
/// variance).
#[must_use]
fn ratio_point_and_var(g: &RatioGroupStats) -> Option<(f64, f64)> {
    if g.n < 2 || g.den_sum <= 0.0 {
        return None;
    }
    let n = g.n as f64;
    let mean_num = g.num_sum / n;
    let mean_den = g.den_sum / n;
    if !mean_den.is_finite() || mean_den <= 0.0 {
        return None;
    }
    let r = g.num_sum / g.den_sum;
    let var_num = g.num_sq_sum / n - mean_num * mean_num;
    let var_den = g.den_sq_sum / n - mean_den * mean_den;
    let cov = g.num_den_sum / n - mean_num * mean_den;
    let var_r = (var_num - 2.0 * r * cov + r * r * var_den) / (mean_den * mean_den * n);
    if !var_r.is_finite() || var_r <= 0.0 || !r.is_finite() {
        return None;
    }
    Some((r, var_r))
}

/// Standard normal CDF via the same erf approximation `frequentist.rs` uses
/// (Abramowitz & Stegun 7.1.26). Kept local so the ratio delta-method test does
/// not depend on a private core helper.
fn norm_cdf(z: f64) -> f64 {
    let erf = |x: f64| -> f64 {
        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let x = x.abs();
        let t = 1.0 / (1.0 + 0.327_591_1 * x);
        let poly = t
            * (0.254_829_592
                + t * (-0.284_496_736
                    + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
        sign * (1.0 - poly * (-x * x).exp())
    };
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

/// Frequentist contrast for a **ratio** metric (treatment vs control) via the
/// delta method.
///
/// The effect is the difference of per-group ratios `R_t − R_c`; the groups are
/// independent so `SE = sqrt(Var(R_c) + Var(R_t))` with each `Var(R)` from
/// [`ratio_point_and_var`]. The two-tailed p-value is `2·Φ(−|z|)` and the 95 %
/// CI is `(R_t − R_c) ± 1.96·SE`.
///
/// Returns a non-significant, wide-CI result when either group is degenerate
/// (mirroring the insufficient-data convention of the count/numeric paths).
#[must_use]
pub fn ratio_frequentist(
    control: &RatioGroupStats,
    variant: &RatioGroupStats,
) -> FrequentistResult {
    let (Some((r_c, var_c)), Some((r_t, var_t))) =
        (ratio_point_and_var(control), ratio_point_and_var(variant))
    else {
        return FrequentistResult {
            p_value: 1.0,
            p_value_corrected: None,
            confidence_interval: ConfidenceInterval {
                lower: f64::NEG_INFINITY,
                upper: f64::INFINITY,
            },
            significant: false,
        };
    };
    let diff = r_t - r_c;
    let se = (var_c + var_t).sqrt();
    let z = if se == 0.0 { 0.0 } else { diff / se };
    let p_value = 2.0 * norm_cdf(-z.abs());
    let margin = 1.96 * se;
    FrequentistResult {
        p_value,
        p_value_corrected: None,
        confidence_interval: ConfidenceInterval {
            lower: diff - margin,
            upper: diff + margin,
        },
        significant: p_value < 0.05,
    }
}

/// Bayesian contrast for a **ratio** metric — a normal posterior on the
/// delta-method estimate/SE (no `analyze_ratio` in core). `prob_best =
/// P(R_t > R_c)` under `N(diff, SE²)`; the credible interval is `diff ± 1.96·SE`;
/// expected loss `E[max(R_c − R_t, 0)]` via the closed-form normal tail
/// expectation (matching `bayesian::analyze_numeric`).
#[must_use]
pub fn ratio_bayesian(control: &RatioGroupStats, variant: &RatioGroupStats) -> BayesianResult {
    let (Some((r_c, var_c)), Some((r_t, var_t))) =
        (ratio_point_and_var(control), ratio_point_and_var(variant))
    else {
        return BayesianResult {
            prob_best: 0.5,
            credible_interval: ConfidenceInterval {
                lower: 0.0,
                upper: 0.0,
            },
            expected_loss: 0.0,
        };
    };
    let diff = r_t - r_c;
    let se = (var_c + var_t).sqrt();
    let prob_best = if se == 0.0 {
        if diff > 0.0 {
            1.0
        } else if diff < 0.0 {
            0.0
        } else {
            0.5
        }
    } else {
        1.0 - norm_cdf(-diff / se)
    };
    let z95 = 1.959_964;
    let lower = diff - z95 * se;
    let upper = diff + z95 * se;
    let expected_loss = if se == 0.0 {
        (-diff).max(0.0)
    } else {
        let d = diff / se;
        let phi_d = (-0.5 * d * d).exp() / (2.0 * std::f64::consts::PI).sqrt();
        (se * phi_d + (-diff) * (1.0 - norm_cdf(d))).max(0.0)
    };
    BayesianResult {
        prob_best,
        credible_interval: ConfidenceInterval { lower, upper },
        expected_loss,
    }
}

// ── Per-context-per-metric computation ───────────────────────────────────────

/// The point `metric_value` rendered per variant for a metric+context (the
/// scalar surfaced in `variant_stats` JSON / the timeseries):
/// conversion rate (count/funnel), mean (numeric), ratio `R`, or the point
/// percentile.
fn point_value(mt: MetricType, vs: &VariantStats, ratio: Option<&RatioGroupStats>) -> f64 {
    match mt {
        MetricType::Count | MetricType::Funnel => vs.conversion_rate.unwrap_or(0.0),
        MetricType::Numeric => match ratio {
            Some(g) if g.den_sum > 0.0 => g.num_sum / g.den_sum,
            _ => vs.mean.unwrap_or(0.0),
        },
        MetricType::Percentile => vs.mean.unwrap_or(0.0),
    }
}

/// Serialize a [`FrequentistResult`] to JSON (the stored `frequentist_result`).
#[must_use]
fn frequentist_to_json(r: &FrequentistResult) -> Value {
    serde_json::to_value(r).unwrap_or(Value::Null)
}

/// Serialize a [`BayesianResult`] to JSON (the stored `bayesian_result`).
#[must_use]
fn bayesian_to_json(r: &BayesianResult) -> Value {
    serde_json::to_value(r).unwrap_or(Value::Null)
}

/// Everything computed for one `(metric_key, context_type)` pair.
struct PairResult {
    /// Per-variant point values (for `points_per_metric`).
    points: Vec<VariantPoint>,
    /// `frequentist_result` JSON object keyed by variant_key, or `None` for
    /// percentile metrics.
    frequentist: Option<Value>,
    /// `bayesian_result` JSON object keyed by variant_key, or `None`.
    bayesian: Option<Value>,
    /// `sequential_result` JSON blob, or `None` when disabled / unsupported.
    sequential: Option<Value>,
    /// Overall recommendation string.
    recommendation: String,
}

/// Per-comparison frequentist + bayesian entry serialized into the per-pair
/// blob, keyed by the non-control variant key.
struct VariantComparison {
    frequentist: FrequentistResult,
    bayesian: BayesianResult,
}

/// Compute the full per-pair result for one metric within one `context_type`.
///
/// `sample_sizes` maps `variant_key → ITT sample size` (assignment count).
/// Exactly one of `agg` / `ratio` / `funnel` is populated per the metric kind;
/// `mt` is the resolved [`MetricType`].
#[allow(clippy::too_many_arguments)]
async fn compute_pair(
    ch: &Client,
    exp: &RunningExperiment,
    metric_key: &str,
    context_type: &str,
    mt: MetricType,
    sample_sizes: &HashMap<String, u64>,
    agg: Option<&HashMap<String, AggCell>>,
    ratio: Option<&HashMap<String, RatioGroupStats>>,
    funnel: Option<&HashMap<String, (u64, u64)>>,
    now: DateTime<Utc>,
) -> PairResult {
    // Build the per-variant VariantStats (and RatioGroupStats) over the UNION of
    // assigned variants (sample_sizes) — a variant with assignments but no events
    // is zero-filled (ITT), and a variant present only in the event cells but not
    // in assignments is ignored (it has no ITT denominator).
    let mut variant_stats: HashMap<String, VariantStats> = HashMap::new();
    let mut ratio_groups: HashMap<String, RatioGroupStats> = HashMap::new();
    let mut variant_keys: Vec<String> = Vec::new();

    for (vk, &n) in sample_sizes {
        variant_keys.push(vk.clone());
        match mt {
            MetricType::Count => {
                let successes = agg.and_then(|m| m.get(vk)).map_or(0, |c| c.successes);
                variant_stats.insert(vk.clone(), count_variant_stats(n, successes));
            }
            MetricType::Numeric if ratio.is_some() => {
                let g = ratio
                    .and_then(|m| m.get(vk))
                    .copied()
                    .unwrap_or(RatioGroupStats {
                        n: n as i64,
                        num_sum: 0.0,
                        den_sum: 0.0,
                        num_sq_sum: 0.0,
                        den_sq_sum: 0.0,
                        num_den_sum: 0.0,
                    });
                // For the point value / recommendation sample-size guard we still
                // carry a VariantStats (mean = R when denominator positive).
                let r = if g.den_sum > 0.0 {
                    g.num_sum / g.den_sum
                } else {
                    0.0
                };
                let mut vs = numeric_variant_stats(n, 0.0, 0.0);
                vs.mean = Some(r);
                variant_stats.insert(vk.clone(), vs);
                ratio_groups.insert(vk.clone(), g);
            }
            MetricType::Numeric => {
                let cell = agg.and_then(|m| m.get(vk)).copied().unwrap_or(AggCell {
                    event_n: 0,
                    successes: 0,
                    value_sum: 0.0,
                    value_sq_sum: 0.0,
                });
                variant_stats.insert(
                    vk.clone(),
                    numeric_variant_stats(n, cell.value_sum, cell.value_sq_sum),
                );
            }
            MetricType::Funnel => {
                let successes = funnel.and_then(|m| m.get(vk)).map_or(0, |&(_, s)| s);
                variant_stats.insert(vk.clone(), funnel_variant_stats(n, successes));
            }
            MetricType::Percentile => {
                let point = agg.and_then(|m| m.get(vk)).map_or(0.0, |c| {
                    // For percentile the cells query stored the point in value_sum
                    // path is not meaningful — we instead use mean of the per-unit
                    // value (value_sum / n) as a coarse stand-in point. (Real
                    // percentile point comes from the scalar aggregation builder;
                    // see module docs — significance is skipped regardless.)
                    if n == 0 { 0.0 } else { c.value_sum / n as f64 }
                });
                variant_stats.insert(vk.clone(), percentile_variant_stats(n, point));
            }
        }
    }
    variant_keys.sort();

    // Points (rendered metric_value per variant).
    let points: Vec<VariantPoint> = variant_keys
        .iter()
        .map(|vk| VariantPoint {
            context_type: context_type.to_string(),
            variant_key: vk.clone(),
            metric_value: point_value(
                mt,
                variant_stats.get(vk).expect("vk present"),
                ratio_groups.get(vk),
            ),
        })
        .collect();

    // Control selection.
    let Some(control_key) = select_control(&variant_keys) else {
        return PairResult {
            points,
            frequentist: None,
            bayesian: None,
            sequential: None,
            recommendation: Recommendation::NeedsMoreData.to_string(),
        };
    };

    // Percentile: skip frequentist / bayesian / sequential; NeedsMoreData.
    if mt == MetricType::Percentile {
        return PairResult {
            points,
            frequentist: None,
            bayesian: None,
            sequential: None,
            recommendation: Recommendation::NeedsMoreData.to_string(),
        };
    }

    // Frequentist + Bayesian per non-control variant (vs control).
    let mut comparisons: HashMap<String, VariantComparison> = HashMap::new();
    let mut ordered_non_control: Vec<String> = variant_keys
        .iter()
        .filter(|k| *k != &control_key)
        .cloned()
        .collect();
    ordered_non_control.sort();

    let control_vs = variant_stats.get(&control_key).expect("control present");
    for vk in &ordered_non_control {
        let vs = variant_stats.get(vk).expect("vk present");
        let (freq, bayes) = match mt {
            MetricType::Count => (
                frequentist::analyze_count(control_vs, vs),
                bayesian::analyze_count(control_vs, vs),
            ),
            MetricType::Funnel => (
                frequentist::analyze_funnel(control_vs, vs),
                bayesian::analyze_funnel(control_vs, vs),
            ),
            MetricType::Numeric if !ratio_groups.is_empty() => {
                let cg = ratio_groups.get(&control_key).expect("control ratio");
                let vg = ratio_groups.get(vk).expect("variant ratio");
                (ratio_frequentist(cg, vg), ratio_bayesian(cg, vg))
            }
            MetricType::Numeric => (
                frequentist::analyze_numeric(control_vs, vs),
                bayesian::analyze_numeric(control_vs, vs),
            ),
            MetricType::Percentile => unreachable!("percentile returned early"),
        };
        comparisons.insert(
            vk.clone(),
            VariantComparison {
                frequentist: freq,
                bayesian: bayes,
            },
        );
    }

    // Bonferroni-correct the K−1 raw p-values (in the sorted non-control order).
    let raw_ps: Vec<f64> = ordered_non_control
        .iter()
        .map(|vk| comparisons[vk].frequentist.p_value)
        .collect();
    let corrected = frequentist::bonferroni_correct(&raw_ps);
    for (vk, p_corr) in ordered_non_control.iter().zip(corrected.iter()) {
        if let Some(c) = comparisons.get_mut(vk) {
            c.frequentist.p_value_corrected = Some(*p_corr);
            // Significance decision uses the corrected p when available.
            if !p_corr.is_nan() {
                c.frequentist.significant = *p_corr < 0.05;
            }
        }
    }

    // Assemble the per-variant frequentist / bayesian JSON blobs.
    let mut freq_obj = serde_json::Map::new();
    let mut bayes_obj = serde_json::Map::new();
    for vk in &ordered_non_control {
        let c = &comparisons[vk];
        freq_obj.insert(vk.clone(), frequentist_to_json(&c.frequentist));
        bayes_obj.insert(vk.clone(), bayesian_to_json(&c.bayesian));
    }

    // Recommendation per variant, then overall winner.
    let min_n = if exp.sequential.min_sample_size > 0 {
        Some(exp.sequential.min_sample_size)
    } else {
        None
    };
    let mut recs: Vec<(String, Recommendation)> = Vec::new();
    for vk in &ordered_non_control {
        let c = &comparisons[vk];
        let vs = variant_stats.get(vk).expect("vk present");
        let rec = recommend(&RecommendationInput {
            variant_key: vk.clone(),
            is_control: false,
            frequentist: Some(c.frequentist.clone()),
            bayesian: Some(c.bayesian.clone()),
            analysis_type: AnalysisType::Frequentist,
            sample_size: vs.sample_size,
            min_sample_size: min_n,
        });
        recs.push((vk.clone(), rec));
    }
    let overall = pick_winner(&recs);

    // Sequential (only when enabled for the experiment).
    let sequential = if exp.sequential.enabled {
        compute_sequential(
            ch,
            exp,
            metric_key,
            context_type,
            mt,
            &control_key,
            &variant_stats,
            &ratio_groups,
            now,
        )
        .await
    } else {
        None
    };

    PairResult {
        points,
        frequentist: Some(Value::Object(freq_obj)),
        bayesian: Some(Value::Object(bayes_obj)),
        sequential,
        recommendation: overall.to_string(),
    }
}

/// Compute the sequential (always-valid) blob for one pair, seeding the running
/// minimum from the prior stored row.
#[allow(clippy::too_many_arguments)]
async fn compute_sequential(
    ch: &Client,
    exp: &RunningExperiment,
    metric_key: &str,
    context_type: &str,
    mt: MetricType,
    control_key: &str,
    variant_stats: &HashMap<String, VariantStats>,
    ratio_groups: &HashMap<String, RatioGroupStats>,
    now: DateTime<Utc>,
) -> Option<Value> {
    let family = SequentialMetricFamily::from_metric_type(sequential_family_str(mt))?;

    let prev = fetch_prev_always_valid_p(
        ch,
        exp.env_id,
        exp.experiment_id,
        exp.iteration_id,
        metric_key,
        context_type,
        now,
    )
    .await;

    if family == SequentialMetricFamily::Ratio || !ratio_groups.is_empty() {
        // Ratio path: explicit τ² is preferred; the default falls back to the
        // floor for ratio (its effect scale lives off VariantStats).
        let cfg = resolve_sequential_config(
            exp.sequential.alpha,
            exp.sequential.tau_squared,
            exp.sequential.min_sample_size,
            default_tau_squared(SequentialMetricFamily::Ratio, &[]),
        );
        compute_sequential_blob_ratio(control_key, ratio_groups, &cfg, &prev)
    } else {
        let stats_refs: Vec<&VariantStats> = variant_stats.values().collect();
        let default_tau = default_tau_squared(family, &stats_refs);
        let cfg = resolve_sequential_config(
            exp.sequential.alpha,
            exp.sequential.tau_squared,
            exp.sequential.min_sample_size,
            default_tau,
        );
        compute_sequential_blob(family, control_key, variant_stats, &cfg, &prev)
    }
}

// ── Top-level entry ──────────────────────────────────────────────────────────

/// Compute every per-`(metric_key, context_type)` [`MetricSummary`] for one
/// running experiment.
///
/// `metrics` maps each of the experiment's metric UUIDs to its definition (the
/// caller resolves these in one batch via `metric_repo.find_batch_by_ids`).
/// Metrics absent from the map are skipped. Ratio metrics whose legs are not
/// resolvable to two aggregation metrics in `metrics` are skipped (logged).
///
/// `iteration_end` upper-bounds the event + assignment windows (`Utc::now()` for
/// a still-running iteration). The returned summaries are sorted by
/// `(metric_key, context_type)` for deterministic writes.
///
/// SRM is computed once per `context_type` over the PRIMARY metric's assignment
/// counts (the first metric in the experiment's `metric_ids` order that resolves
/// for that context) and surfaced in the recommendation note of every summary in
/// that context when the health is not Green.
///
/// # Errors
/// Propagates the first ClickHouse reader error encountered.
pub async fn run_stats_compute(
    reader: &dyn CellReader,
    ch: &Client,
    exp: &RunningExperiment,
    metrics: &HashMap<Uuid, MetricDefinition>,
    iteration_end: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Vec<MetricSummary>, anyhow::Error> {
    let context_types: Vec<String> = if exp.unit_context_types.is_empty() {
        vec!["user".to_string()]
    } else {
        exp.unit_context_types.clone()
    };

    // Resolve the experiment's metric definitions in iteration order.
    let defs: Vec<&MetricDefinition> = exp
        .metric_ids
        .iter()
        .filter_map(|id| metrics.get(id))
        .collect();

    let mut points_per_metric: HashMap<String, Vec<VariantPoint>> = HashMap::new();
    let mut frequentist_per_pair: HashMap<(String, String), Value> = HashMap::new();
    let mut bayesian_per_pair: HashMap<(String, String), Value> = HashMap::new();
    let mut sequential_per_pair: HashMap<(String, String), Value> = HashMap::new();
    let mut recommendations_per_pair: HashMap<(String, String), String> = HashMap::new();

    for context_type in &context_types {
        // ITT sample-size denominator per variant (assignment counts). Shared
        // across every metric in this context_type AND used for SRM.
        let counts = reader
            .assignment_counts(
                exp.experiment_id,
                exp.iteration_id,
                exp.env_id,
                context_type,
                iteration_end,
            )
            .await?;
        let sample_sizes: HashMap<String, u64> = counts.iter().cloned().collect();

        // SRM once per context over the assignment counts (equal-split expected).
        let srm_note = srm_note_for(&counts);

        for def in &defs {
            let mt = metric_type_for(def);
            let metric_key = def.key.clone();

            // Fetch the family-specific sufficient stats.
            let (agg, ratio, funnel) = match &def.kind {
                MetricKind::Aggregation(cfg) => {
                    let cells = reader
                        .aggregation_cells(
                            cfg,
                            exp.experiment_id,
                            exp.iteration_id,
                            exp.env_id,
                            context_type,
                            iteration_end,
                        )
                        .await?;
                    (Some(cells), None, None)
                }
                MetricKind::Ratio(ratio_cfg) => {
                    let Some((num_cfg, den_cfg)) = resolve_ratio_legs(ratio_cfg, metrics) else {
                        tracing::debug!(
                            metric_key = %metric_key,
                            "compute: ratio legs not resolvable to two aggregation metrics; skipping"
                        );
                        continue;
                    };
                    let cells = reader
                        .ratio_cells(
                            &num_cfg,
                            &den_cfg,
                            exp.experiment_id,
                            exp.iteration_id,
                            exp.env_id,
                            context_type,
                            iteration_end,
                        )
                        .await?;
                    (None, Some(cells), None)
                }
                MetricKind::Funnel(cfg) => {
                    let cells = reader
                        .funnel_cells(
                            cfg,
                            exp.experiment_id,
                            exp.iteration_id,
                            exp.env_id,
                            context_type,
                            iteration_end,
                        )
                        .await?;
                    (None, None, Some(cells))
                }
            };

            let pair = compute_pair(
                ch,
                exp,
                &metric_key,
                context_type,
                mt,
                &sample_sizes,
                agg.as_ref(),
                ratio.as_ref(),
                funnel.as_ref(),
                now,
            )
            .await;

            points_per_metric
                .entry(metric_key.clone())
                .or_default()
                .extend(pair.points);
            let key = (metric_key.clone(), context_type.clone());
            if let Some(f) = pair.frequentist {
                frequentist_per_pair.insert(key.clone(), f);
            }
            if let Some(b) = pair.bayesian {
                bayesian_per_pair.insert(key.clone(), b);
            }
            if let Some(s) = pair.sequential {
                sequential_per_pair.insert(key.clone(), s);
            }
            // Append the SRM note (if any) to the recommendation string so the
            // SRM verdict surfaces even though the result row has no dedicated
            // SRM column (tracked as a follow-up — see module docs).
            let rec = match &srm_note {
                Some(note) => format!("{}; {}", pair.recommendation, note),
                None => pair.recommendation,
            };
            recommendations_per_pair.insert(key, rec);
        }
    }

    // Build the per-(metric_key, context_type) summaries, then thread the real
    // metric_type onto each from the metric kind.
    let mut summaries = crate::results_writer::build_metric_summaries(
        &points_per_metric,
        &frequentist_per_pair,
        &bayesian_per_pair,
        &sequential_per_pair,
        &recommendations_per_pair,
    );
    let type_by_key: HashMap<String, &'static str> = defs
        .iter()
        .map(|d| (d.key.clone(), metric_type_str(metric_type_for(d))))
        .collect();
    for s in &mut summaries {
        if let Some(t) = type_by_key.get(&s.metric_key) {
            s.metric_type = (*t).to_string();
        }
    }
    Ok(summaries)
}

/// The serialized discriminant for a [`MetricType`] (matches
/// [`MetricType`]'s snake_case serde, used as the result row's `metric_type`).
#[must_use]
fn metric_type_str(mt: MetricType) -> &'static str {
    match mt {
        MetricType::Count => "count",
        MetricType::Numeric => "numeric",
        MetricType::Percentile => "percentile",
        MetricType::Funnel => "funnel",
    }
}

/// Run the SRM chi-square over equal-split assignment counts and, when the
/// health is not Green, return a short human-readable note (`srm:<health>
/// p=<...>`). Returns `None` when SRM is Green or undefined (< 2 variants).
fn srm_note_for(counts: &[(String, u64)]) -> Option<String> {
    if counts.len() < 2 {
        return None;
    }
    let total: u64 = counts.iter().map(|(_, n)| n).sum();
    if total == 0 {
        return None;
    }
    let expected = total as f64 / counts.len() as f64;
    let observations: Vec<SrmObservation> = counts
        .iter()
        .map(|(vk, n)| SrmObservation {
            variant_key: vk.clone(),
            observed: *n,
            expected,
        })
        .collect();
    let result = compute_srm(&observations);
    use stitchd_core::experimentation::stats::srm::SrmHealth;
    match result.health {
        SrmHealth::Green => None,
        SrmHealth::Yellow => Some(format!("srm:yellow p={:.4}", result.overall_chi_sq_p)),
        SrmHealth::Red => Some(format!("srm:red p={:.4}", result.overall_chi_sq_p)),
    }
}

/// Resolve a [`RatioConfig`] to its numerator + denominator
/// [`AggregationConfig`] legs (mirrors `interaction_compute::resolve_ratio_legs`).
fn resolve_ratio_legs(
    ratio_cfg: &RatioConfig,
    metrics: &HashMap<Uuid, MetricDefinition>,
) -> Option<(AggregationConfig, AggregationConfig)> {
    let num = metrics.get(&ratio_cfg.numerator_metric_id.as_uuid())?;
    let den = metrics.get(&ratio_cfg.denominator_metric_id.as_uuid())?;
    let (MetricKind::Aggregation(num_cfg), MetricKind::Aggregation(den_cfg)) =
        (&num.kind, &den.kind)
    else {
        return None;
    };
    Some((num_cfg.clone(), den_cfg.clone()))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── VariantStats construction ────────────────────────────────────────────

    #[test]
    fn count_variant_stats_itt_rate() {
        let vs = count_variant_stats(1000, 150);
        assert_eq!(vs.sample_size, 1000);
        assert_eq!(vs.conversions, Some(150));
        assert!((vs.conversion_rate.unwrap() - 0.15).abs() < 1e-12);
    }

    #[test]
    fn count_variant_stats_zero_sample_is_safe() {
        let vs = count_variant_stats(0, 0);
        assert_eq!(vs.sample_size, 0);
        assert_eq!(vs.conversion_rate, Some(0.0));
    }

    #[test]
    fn numeric_variant_stats_uses_sample_variance() {
        // Values [2, 4, 6]: Σx=12, Σx²=56, n=3 over ITT denominator n=3.
        // mean=4; pop_var=56/3-16=2.6667; sample_var=pop_var*3/2=4.0.
        let vs = numeric_variant_stats(3, 12.0, 56.0);
        assert!((vs.mean.unwrap() - 4.0).abs() < 1e-9);
        assert!(
            (vs.variance.unwrap() - 4.0).abs() < 1e-9,
            "sample variance should be 4.0, got {}",
            vs.variance.unwrap()
        );
    }

    #[test]
    fn numeric_variant_stats_itt_zero_fill_lowers_mean() {
        // 3 firing units summing to 12 but ITT denominator 6 (3 non-firing zeros):
        // mean = 12/6 = 2.0 (ITT spreads the sum over all assigned units).
        let vs = numeric_variant_stats(6, 12.0, 56.0);
        assert!((vs.mean.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(vs.sample_size, 6);
    }

    #[test]
    fn numeric_variant_stats_single_unit_zero_variance() {
        let vs = numeric_variant_stats(1, 5.0, 25.0);
        assert_eq!(vs.variance, Some(0.0));
    }

    #[test]
    fn funnel_variant_stats_rate_over_top_of_funnel() {
        let vs = funnel_variant_stats(500, 50);
        assert_eq!(vs.sample_size, 500);
        assert_eq!(vs.conversions, Some(50));
        assert!((vs.conversion_rate.unwrap() - 0.1).abs() < 1e-12);
    }

    #[test]
    fn percentile_variant_stats_carries_point() {
        let vs = percentile_variant_stats(100, 250.0);
        assert!((vs.mean.unwrap() - 250.0).abs() < 1e-12);
        let p = vs.percentiles.unwrap();
        assert!((p.p50 - 250.0).abs() < 1e-12);
    }

    // ── Control selection ────────────────────────────────────────────────────

    #[test]
    fn select_control_prefers_literal_control() {
        let keys = vec!["b".to_string(), "control".to_string(), "a".to_string()];
        assert_eq!(select_control(&keys), Some("control".to_string()));
    }

    #[test]
    fn select_control_falls_back_to_lexicographically_smallest() {
        let keys = vec!["treatment".to_string(), "baseline".to_string()];
        assert_eq!(select_control(&keys), Some("baseline".to_string()));
    }

    #[test]
    fn select_control_empty_is_none() {
        assert_eq!(select_control(&[]), None);
    }

    // ── metric_type_for ──────────────────────────────────────────────────────

    fn agg_metric(op: AggregationOperator) -> MetricDefinition {
        use stitchd_core::id::{EnvironmentId, MetricId};
        use stitchd_core::metric::GoalDirection;
        let now = Utc::now();
        MetricDefinition {
            id: MetricId::new(),
            environment_id: EnvironmentId::new(),
            key: "k".into(),
            name: "k".into(),
            description: None,
            kind: MetricKind::Aggregation(AggregationConfig {
                event_key: "e".into(),
                aggregator: op,
                on_field: None,
                where_clause: None,
            }),
            goal_direction: GoalDirection::Increase,
            version: 1,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    #[test]
    fn metric_type_classifies_aggregators() {
        assert_eq!(
            metric_type_for(&agg_metric(AggregationOperator::Count)),
            MetricType::Count
        );
        assert_eq!(
            metric_type_for(&agg_metric(AggregationOperator::Uniq)),
            MetricType::Count
        );
        assert_eq!(
            metric_type_for(&agg_metric(AggregationOperator::Sum)),
            MetricType::Numeric
        );
        assert_eq!(
            metric_type_for(&agg_metric(AggregationOperator::Avg)),
            MetricType::Numeric
        );
        assert_eq!(
            metric_type_for(&agg_metric(AggregationOperator::P50)),
            MetricType::Percentile
        );
        assert_eq!(
            metric_type_for(&agg_metric(AggregationOperator::P99)),
            MetricType::Percentile
        );
    }

    // ── Ratio delta-method frequentist ───────────────────────────────────────

    #[test]
    fn ratio_frequentist_detects_clear_difference() {
        // Two clearly-different ratios with spread, mirroring the core
        // sequential_ratio fixture. R_c=0.5, R_t=0.75 on 1000 units each.
        let control = RatioGroupStats {
            n: 1000,
            num_sum: 1000.0,
            den_sum: 2000.0,
            num_sq_sum: 1500.0,
            den_sq_sum: 5000.0,
            num_den_sum: 2400.0,
        };
        let treatment = RatioGroupStats {
            n: 1000,
            num_sum: 1500.0,
            den_sum: 2000.0,
            num_sq_sum: 2800.0,
            den_sq_sum: 5000.0,
            num_den_sum: 3300.0,
        };
        let r = ratio_frequentist(&control, &treatment);
        // Point estimate diff is R_t - R_c = 0.75 - 0.5 = 0.25 (CI midpoint).
        let mid = (r.confidence_interval.lower + r.confidence_interval.upper) / 2.0;
        assert!(
            (mid - 0.25).abs() < 1e-9,
            "CI midpoint {mid} should be 0.25"
        );
        assert!(r.significant, "p={}", r.p_value);
        assert!(r.p_value < 0.05);
    }

    #[test]
    fn ratio_frequentist_known_value_z_check() {
        // Construct groups with hand-computable variances.
        // control: n=4, num=[1,1,1,1] den=[2,2,2,2] → R=0.5, var_num=0, var_den=0
        //   → Var(R)=0 → degenerate (var_r <= 0) → insufficient.
        // Use a spread case instead to exercise a finite z.
        let control = RatioGroupStats {
            n: 100,
            num_sum: 50.0,
            den_sum: 100.0,    // R = 0.5
            num_sq_sum: 30.0,  // mean_num=0.5, var_num=0.30-0.25=0.05
            den_sq_sum: 120.0, // mean_den=1.0, var_den=1.2-1.0=0.2
            num_den_sum: 55.0, // cov=0.55-0.5=0.05
        };
        // Var(R)=(0.05 - 2*0.5*0.05 + 0.25*0.2)/(1.0 * 100) = (0.05-0.05+0.05)/100 = 5e-4
        let (r, var) = ratio_point_and_var(&control).expect("finite");
        assert!((r - 0.5).abs() < 1e-12);
        assert!(
            (var - 5e-4).abs() < 1e-9,
            "Var(R) should be 5e-4, got {var}"
        );
    }

    #[test]
    fn ratio_frequentist_degenerate_is_not_significant() {
        let degenerate = RatioGroupStats {
            n: 1,
            num_sum: 1.0,
            den_sum: 2.0,
            num_sq_sum: 1.0,
            den_sq_sum: 4.0,
            num_den_sum: 2.0,
        };
        let r = ratio_frequentist(&degenerate, &degenerate);
        assert!(!r.significant);
        assert!((r.p_value - 1.0).abs() < 1e-12);
    }

    // ── point_value ──────────────────────────────────────────────────────────

    #[test]
    fn point_value_count_is_conversion_rate() {
        let vs = count_variant_stats(1000, 250);
        let v = point_value(MetricType::Count, &vs, None);
        assert!((v - 0.25).abs() < 1e-12);
    }

    #[test]
    fn point_value_ratio_is_r() {
        let g = RatioGroupStats {
            n: 100,
            num_sum: 30.0,
            den_sum: 120.0,
            num_sq_sum: 0.0,
            den_sq_sum: 0.0,
            num_den_sum: 0.0,
        };
        let mut vs = numeric_variant_stats(100, 0.0, 0.0);
        vs.mean = Some(0.25);
        let v = point_value(MetricType::Numeric, &vs, Some(&g));
        assert!((v - 0.25).abs() < 1e-12, "R = 30/120 = 0.25, got {v}");
    }
}
