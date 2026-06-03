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
//! The ratio contrast is computed via [`frequentist::analyze_ratio`] /
//! [`bayesian::analyze_ratio`] in stitchd-core (the analyzers mirror the other
//! metric families), which reuse `RatioGroupStats::ratio_var` — the single
//! source of truth for the delta-method point + variance shared with
//! `stats::sequential::sequential_ratio` / `interaction::ratio`
//! (`Var(R) ≈ (var_num − 2R·cov + R²·var_den) / (mean_den²·n)`, diff-of-ratios
//! `SE = sqrt(Var(R_t) + Var(R_c))`, two-tailed normal `z`).
//!
//! ## Percentile metrics
//!
//! P50/P90/P99 fetch the per-unit raw sample via
//! [`crate::queries::variant_stats::build_percentile_samples_query`] and run a
//! bootstrap significance test (`frequentist::analyze_percentile` +
//! `bayesian::analyze_percentile`) at the aggregator's quantile, producing a
//! real [`Recommendation`]. When the raw sample cannot be fetched (or is empty)
//! the pair falls back to `NeedsMoreData` with no frequentist/bayesian blob.
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
    AnalysisType, BayesianResult, FrequentistResult, MetricType, Percentiles, Recommendation,
    VariantStats, bayesian, frequentist,
    recommendation::{RecommendationInput, pick_winner, recommend},
};
use stitchd_core::metric::{
    AggregationConfig, AggregationOperator, MetricDefinition, MetricKind, RatioConfig,
};

use crate::dispatch::rewrite_placeholders_to_clickhouse;
use crate::queries::QueryBind;
use crate::queries::variant_stats::{
    build_aggregation_cells_query, build_assignment_counts_query, build_funnel_cells_query,
    build_percentile_samples_query, build_ratio_cells_query,
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

/// Raw per-unit sample row (percentile significance) per `variant_key` — the
/// ITT per-unit metric values collected via `groupArray` (capped CH-side).
#[derive(Debug, Clone, serde::Deserialize, clickhouse::Row)]
struct ChPercentileSamplesRow {
    variant_key: String,
    samples: Vec<f64>,
}

/// Upper bound on the number of raw per-unit samples collected per
/// `(context_type, variant_key)` for a percentile significance test. The
/// ClickHouse `groupArray(SAMPLE_CAP)(...)` caps the array server-side so a hot
/// variant cannot stream an unbounded array into the compute pass; a returned
/// length equal to the cap is treated as "possibly truncated" and logged.
const PERCENTILE_SAMPLE_CAP: u64 = 100_000;

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

    /// Raw per-unit metric samples per variant for a percentile significance
    /// test (capped at [`PERCENTILE_SAMPLE_CAP`] CH-side). Returns
    /// `variant_key → Vec<per-unit value>` (ITT — non-firing units contribute
    /// `0`).
    async fn percentile_samples(
        &self,
        cfg: &AggregationConfig,
        experiment_id: Uuid,
        iteration_id: Uuid,
        env_id: Uuid,
        context_type: &str,
        iteration_end: DateTime<Utc>,
    ) -> Result<HashMap<String, Vec<f64>>, anyhow::Error>;
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

    async fn percentile_samples(
        &self,
        cfg: &AggregationConfig,
        experiment_id: Uuid,
        iteration_id: Uuid,
        env_id: Uuid,
        context_type: &str,
        iteration_end: DateTime<Utc>,
    ) -> Result<HashMap<String, Vec<f64>>, anyhow::Error> {
        let built = build_percentile_samples_query(
            cfg,
            &experiment_id.to_string(),
            &iteration_id.to_string(),
            &env_id.to_string(),
            context_type,
            iteration_end,
            PERCENTILE_SAMPLE_CAP,
        )?;
        let rows = bind_query(&self.client, built.sql, built.binds)
            .fetch_all::<ChPercentileSamplesRow>()
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.variant_key, r.samples))
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

/// The target percentile in `[0, 100]` for a percentile aggregator
/// (P50 → 50.0, P90 → 90.0, P99 → 99.0), as consumed by
/// [`frequentist::analyze_percentile`] / [`bayesian::analyze_percentile`]
/// (which divide by 100 internally). Returns `None` for non-percentile
/// aggregators.
#[must_use]
fn percentile_for_aggregator(op: AggregationOperator) -> Option<f64> {
    match op {
        AggregationOperator::P50 => Some(50.0),
        AggregationOperator::P90 => Some(90.0),
        AggregationOperator::P99 => Some(99.0),
        _ => None,
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

/// Raw per-unit samples + target quantile for a percentile-metric significance
/// test. `samples` maps `variant_key → Vec<per-unit value>` (ITT, capped
/// CH-side); `percentile` is the quantile in `[0, 100]` (P50 → 50.0).
struct PercentileInput {
    samples: HashMap<String, Vec<f64>>,
    percentile: f64,
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
    percentile: Option<&PercentileInput>,
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

    // Percentile: run a bootstrap significance test on the raw per-unit samples
    // (frequentist bootstrap CI + bayesian bootstrap prob_best at the
    // aggregator's quantile). No sequential analogue. Falls back to
    // NeedsMoreData when the raw samples are unavailable / empty.
    if mt == MetricType::Percentile {
        return compute_percentile_pair(
            points,
            &variant_keys,
            &control_key,
            exp,
            percentile,
            &variant_stats,
        );
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
                (
                    frequentist::analyze_ratio(cg, vg),
                    bayesian::analyze_ratio(cg, vg),
                )
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

/// Compute the per-pair result for a **percentile** metric via a bootstrap
/// significance test on the raw per-unit samples.
///
/// For each non-control variant we run [`frequentist::analyze_percentile`]
/// (bootstrap difference CI — its `p_value` is NaN by design) and
/// [`bayesian::analyze_percentile`] (bootstrap `prob_best`) at the aggregator's
/// quantile, then derive a [`Recommendation`] from the **bayesian** posterior
/// (`prob_best`) since the bootstrap CI carries no analytic p-value. The
/// per-variant frequentist + bayesian JSON blobs mirror the other families.
///
/// Falls back to a `NeedsMoreData`, blob-less result when the raw samples are
/// missing (the fetch failed) or the control / every variant sample is empty —
/// matching the prior percentile behaviour rather than emitting a degenerate
/// test.
fn compute_percentile_pair(
    points: Vec<VariantPoint>,
    variant_keys: &[String],
    control_key: &str,
    exp: &RunningExperiment,
    percentile: Option<&PercentileInput>,
    variant_stats: &HashMap<String, VariantStats>,
) -> PairResult {
    let needs_more = || PairResult {
        points: points.clone(),
        frequentist: None,
        bayesian: None,
        sequential: None,
        recommendation: Recommendation::NeedsMoreData.to_string(),
    };

    // Require the raw samples + a non-empty control sample to test against.
    let Some(pct) = percentile else {
        return needs_more();
    };
    let q = pct.percentile;
    let control_samples = match pct.samples.get(control_key) {
        Some(s) if !s.is_empty() => s.as_slice(),
        _ => return needs_more(),
    };

    let mut ordered_non_control: Vec<String> = variant_keys
        .iter()
        .filter(|k| *k != control_key)
        .cloned()
        .collect();
    ordered_non_control.sort();

    let min_n = if exp.sequential.min_sample_size > 0 {
        Some(exp.sequential.min_sample_size)
    } else {
        None
    };

    let mut freq_obj = serde_json::Map::new();
    let mut bayes_obj = serde_json::Map::new();
    let mut recs: Vec<(String, Recommendation)> = Vec::new();
    let mut any_variant_tested = false;

    for vk in &ordered_non_control {
        let vs = variant_stats.get(vk).expect("vk present");
        let variant_samples = pct.samples.get(vk).map(Vec::as_slice).unwrap_or(&[]);
        if variant_samples.is_empty() {
            // No raw sample for this variant → cannot test it; it stays
            // NeedsMoreData and contributes no blob entry.
            recs.push((vk.clone(), Recommendation::NeedsMoreData));
            continue;
        }
        any_variant_tested = true;
        let freq = frequentist::analyze_percentile(control_samples, variant_samples, q);
        let bayes = bayesian::analyze_percentile(control_samples, variant_samples, q);
        freq_obj.insert(vk.clone(), frequentist_to_json(&freq));
        bayes_obj.insert(vk.clone(), bayesian_to_json(&bayes));

        // Recommendation from the bayesian posterior: the bootstrap frequentist
        // p-value is NaN, so the Bayesian rule (prob_best thresholds) drives the
        // verdict here.
        let rec = recommend(&RecommendationInput {
            variant_key: vk.clone(),
            is_control: false,
            frequentist: Some(freq),
            bayesian: Some(bayes),
            analysis_type: AnalysisType::Bayesian,
            sample_size: vs.sample_size,
            min_sample_size: min_n,
        });
        recs.push((vk.clone(), rec));
    }

    if !any_variant_tested {
        return needs_more();
    }

    let overall = pick_winner(&recs);
    PairResult {
        points,
        frequentist: Some(Value::Object(freq_obj)),
        bayesian: Some(Value::Object(bayes_obj)),
        sequential: None,
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
/// SRM is computed once per `context_type` over that context's assignment
/// counts (the equal-split chi-square) and attached as a dedicated
/// `variant_stats["srm"]` JSON field on every summary in that context — the
/// exact shape + location the experimentation-service read consumes
/// (`variant_stats.get("srm")` → `srm_json_to_proto` → `ContextTypeResults.srm`).
/// It is NOT folded into the recommendation string.
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
    // SRM JSON per context_type (computed once over the assignment counts);
    // attached under the top-level `"srm"` key of every result row's
    // `variant_stats` in that context, which is exactly where the
    // experimentation-service read consumes it (see [`srm_json_for`]).
    let mut srm_per_ctx: HashMap<String, Value> = HashMap::new();

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

        // SRM once per context over the assignment counts (equal-split
        // expected). Stored to attach under `variant_stats["srm"]` after the
        // summaries are built — NOT folded into the recommendation string.
        if let Some(srm_json) = srm_json_for(&counts) {
            srm_per_ctx.insert(context_type.clone(), srm_json);
        }

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

            // Percentile metrics additionally need the raw per-unit samples for
            // the bootstrap significance test (the scalar cells can't
            // reconstruct a quantile CI). Only fetched for the percentile
            // aggregators; the resolved `q` comes from the aggregator (P50/P90/
            // P99).
            let percentile = match &def.kind {
                MetricKind::Aggregation(cfg)
                    if percentile_for_aggregator(cfg.aggregator).is_some() =>
                {
                    let q = percentile_for_aggregator(cfg.aggregator).expect("percentile agg");
                    let samples = reader
                        .percentile_samples(
                            cfg,
                            exp.experiment_id,
                            exp.iteration_id,
                            exp.env_id,
                            context_type,
                            iteration_end,
                        )
                        .await?;
                    if let Some((vk, s)) = samples
                        .iter()
                        .find(|(_, s)| s.len() as u64 >= PERCENTILE_SAMPLE_CAP)
                    {
                        tracing::warn!(
                            metric_key = %metric_key,
                            context_type = %context_type,
                            variant_key = %vk,
                            cap = PERCENTILE_SAMPLE_CAP,
                            len = s.len(),
                            "compute: percentile sample hit the cap; the bootstrap CI uses a truncated sample"
                        );
                    }
                    Some(PercentileInput {
                        samples,
                        percentile: q,
                    })
                }
                _ => None,
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
                percentile.as_ref(),
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
            // The recommendation is the metric verdict ALONE — the SRM verdict
            // is now surfaced as a dedicated `variant_stats["srm"]` field
            // (attached below), not appended here.
            recommendations_per_pair.insert(key, pair.recommendation);
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
        // Attach the per-context SRM JSON under the top-level `"srm"` key of the
        // row's `variant_stats` object — the exact location the
        // experimentation-service read consumes (`variant_stats.get("srm")` →
        // `srm_json_to_proto` → `ContextTypeResults.srm`). Every row in a
        // context carries the same SRM snapshot (the read de-dupes per context).
        if let Some(srm_json) = srm_per_ctx.get(&s.context_type)
            && let Some(obj) = s.variant_stats.as_object_mut()
        {
            obj.insert("srm".to_string(), srm_json.clone());
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

/// Run the SRM chi-square over equal-split assignment counts and build the
/// per-`context_type` SRM JSON in the EXACT shape the experimentation-service
/// read consumes (`service::srm_json_to_proto`): a top-level object with
///
/// ```json
/// {
///   "per_variant": [
///     {"variant_key": "control", "observed": 1000, "expected": 1000.0,
///      "chi_sq_contribution": 0.0}
///   ],
///   "overall_chi_sq": 0.0,
///   "overall_chi_sq_p": 1.0,
///   "health": "green"
/// }
/// ```
///
/// This object is attached once per context_type under the top-level `"srm"`
/// key of each result row's `variant_stats` JSON (where the read looks for it),
/// REPLACING the prior approach of stuffing an `srm:<health>` note into the
/// recommendation string. `chi_sq_contribution = (observed − expected)² /
/// expected` is computed here because the core [`SrmPerVariant`] exposes
/// `deviation_pct` rather than the contribution the proto field wants.
///
/// Returns `None` when SRM is undefined for the context (< 2 variants or zero
/// total assignments) so no `"srm"` key is emitted in that case.
fn srm_json_for(counts: &[(String, u64)]) -> Option<Value> {
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

    let per_variant: Vec<Value> = result
        .per_variant
        .iter()
        .map(|pv| {
            // chi_sq_contribution = (observed − expected)² / expected (0 when
            // expected == 0 to avoid a NaN), matching the proto SrmPerVariant
            // field the read maps into.
            let contribution = if pv.expected > 0.0 {
                let diff = pv.observed as f64 - pv.expected;
                diff * diff / pv.expected
            } else {
                0.0
            };
            serde_json::json!({
                "variant_key": pv.variant_key,
                "observed": pv.observed,
                "expected": pv.expected,
                "chi_sq_contribution": contribution,
            })
        })
        .collect();

    use stitchd_core::experimentation::stats::srm::SrmHealth;
    let health = match result.health {
        SrmHealth::Green => "green",
        SrmHealth::Yellow => "yellow",
        SrmHealth::Red => "red",
    };

    Some(serde_json::json!({
        "per_variant": per_variant,
        "overall_chi_sq": result.overall_chi_sq,
        "overall_chi_sq_p": result.overall_chi_sq_p,
        "health": health,
    }))
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

    // ── Ratio delta-method analyzers (now in stitchd-core) ───────────────────

    /// The ratio contrast is delegated to `frequentist::analyze_ratio` /
    /// `bayesian::analyze_ratio` in stitchd-core (the inline helpers were
    /// removed). This compute-layer smoke test pins the wired-in behaviour:
    /// R_c=0.5, R_t=0.75 → diff 0.25, significant, prob_best near 1. The
    /// exhaustive known-value / degenerate cases live in the core unit tests.
    #[test]
    fn ratio_analyzers_in_core_detect_clear_difference() {
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
        let f = frequentist::analyze_ratio(&control, &treatment);
        let mid = (f.confidence_interval.lower + f.confidence_interval.upper) / 2.0;
        assert!(
            (mid - 0.25).abs() < 1e-9,
            "CI midpoint {mid} should be 0.25"
        );
        assert!(f.significant, "p={}", f.p_value);
        assert!(f.p_value < 0.05);

        let b = bayesian::analyze_ratio(&control, &treatment);
        assert!(b.prob_best > 0.99, "prob_best={}", b.prob_best);
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

    // ── percentile_for_aggregator ─────────────────────────────────────────────

    #[test]
    fn percentile_for_aggregator_maps_quantiles() {
        assert_eq!(
            percentile_for_aggregator(AggregationOperator::P50),
            Some(50.0)
        );
        assert_eq!(
            percentile_for_aggregator(AggregationOperator::P90),
            Some(90.0)
        );
        assert_eq!(
            percentile_for_aggregator(AggregationOperator::P99),
            Some(99.0)
        );
        assert_eq!(percentile_for_aggregator(AggregationOperator::Count), None);
        assert_eq!(percentile_for_aggregator(AggregationOperator::Sum), None);
    }

    // ── compute_percentile_pair (pure) ────────────────────────────────────────

    fn pct_running_exp() -> RunningExperiment {
        use crate::scheduler::SequentialSettings;
        RunningExperiment {
            experiment_id: Uuid::new_v4(),
            env_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
            metric_ids: vec![],
            variant_keys: vec!["control".into(), "treatment".into()],
            started_at: Utc::now(),
            unit_context_types: vec!["user".into()],
            pre_period_days: 0,
            sequential: SequentialSettings {
                enabled: false,
                alpha: 0.05,
                tau_squared: None,
                min_sample_size: 0,
            },
        }
    }

    /// A clear upward sample shift yields real frequentist + bayesian blobs and
    /// a non-`NeedsMoreData` recommendation (the bead's pure-path proof).
    #[test]
    fn compute_percentile_pair_clear_shift_is_significant() {
        let control_samples: Vec<f64> = (0..200).map(|i| 10.0 + (i % 20) as f64).collect();
        let variant_samples: Vec<f64> = (0..200).map(|i| 40.0 + (i % 20) as f64).collect();
        let mut samples = HashMap::new();
        samples.insert("control".to_string(), control_samples);
        samples.insert("treatment".to_string(), variant_samples);
        let pct = PercentileInput {
            samples,
            percentile: 90.0,
        };
        let mut variant_stats = HashMap::new();
        variant_stats.insert("control".to_string(), percentile_variant_stats(200, 28.0));
        variant_stats.insert("treatment".to_string(), percentile_variant_stats(200, 58.0));
        let variant_keys = vec!["control".to_string(), "treatment".to_string()];

        let res = compute_percentile_pair(
            vec![],
            &variant_keys,
            "control",
            &pct_running_exp(),
            Some(&pct),
            &variant_stats,
        );
        assert!(res.frequentist.is_some(), "percentile freq blob present");
        assert!(res.bayesian.is_some(), "percentile bayes blob present");
        assert_ne!(res.recommendation, "needs_more_data");
        // The bootstrap frequentist p_value is NaN (serialised null); the CI is
        // finite + the bayesian prob_best is high.
        let freq = res.frequentist.unwrap();
        assert!(
            freq["treatment"]["confidence_interval"]["lower"]
                .as_f64()
                .unwrap()
                > 0.0
        );
        let bayes = res.bayesian.unwrap();
        assert!(bayes["treatment"]["prob_best"].as_f64().unwrap() > 0.95);
    }

    /// Missing raw samples (the fetch yielded nothing) → fall back to the prior
    /// `NeedsMoreData`, blob-less behaviour.
    #[test]
    fn compute_percentile_pair_no_samples_is_needs_more_data() {
        let variant_keys = vec!["control".to_string(), "treatment".to_string()];
        let res = compute_percentile_pair(
            vec![],
            &variant_keys,
            "control",
            &pct_running_exp(),
            None,
            &HashMap::new(),
        );
        assert!(res.frequentist.is_none());
        assert!(res.bayesian.is_none());
        assert_eq!(res.recommendation, "needs_more_data");
    }

    // ── srm_json_for (891 — dedicated SRM surfacing) ──────────────────────────

    /// The SRM JSON is emitted in the EXACT shape the experimentation-service
    /// read (`srm_json_to_proto`) consumes: `per_variant[].{variant_key,
    /// observed, expected, chi_sq_contribution}` + `overall_chi_sq` +
    /// `overall_chi_sq_p` + lowercase `health`. A balanced 50/50 split → green.
    #[test]
    fn srm_json_for_balanced_is_green_with_read_shape() {
        let counts = vec![
            ("control".to_string(), 1000),
            ("treatment".to_string(), 1000),
        ];
        let srm = srm_json_for(&counts).expect("srm json for 2 variants");
        assert_eq!(srm["health"], "green");
        assert!(srm["overall_chi_sq"].as_f64().unwrap() < 1e-9);
        assert!(srm["overall_chi_sq_p"].as_f64().unwrap() > 0.99);
        let pv = srm["per_variant"].as_array().expect("per_variant array");
        assert_eq!(pv.len(), 2);
        for row in pv {
            // Every key the read's srm_json_to_proto reads must be present + typed.
            assert!(row["variant_key"].is_string());
            assert!(row["observed"].as_u64().is_some());
            assert!(row["expected"].as_f64().is_some());
            assert!(row["chi_sq_contribution"].as_f64().is_some());
        }
    }

    /// A heavy mismatch (800 vs 1200, expected 1000 each) → red, with the
    /// per-variant `chi_sq_contribution` computed correctly:
    /// (200²/1000) = 40 each, total χ² = 80.
    #[test]
    fn srm_json_for_mismatch_is_red_with_contributions() {
        let counts = vec![("a".to_string(), 800), ("b".to_string(), 1200)];
        let srm = srm_json_for(&counts).expect("srm json");
        assert_eq!(srm["health"], "red");
        assert!((srm["overall_chi_sq"].as_f64().unwrap() - 80.0).abs() < 1e-6);
        let pv = srm["per_variant"].as_array().unwrap();
        for row in pv {
            assert!(
                (row["chi_sq_contribution"].as_f64().unwrap() - 40.0).abs() < 1e-6,
                "each contribution should be 40, got {}",
                row["chi_sq_contribution"]
            );
        }
    }

    /// Fewer than 2 variants (or zero total) → no SRM key emitted.
    #[test]
    fn srm_json_for_single_variant_is_none() {
        assert!(srm_json_for(&[("only".to_string(), 100)]).is_none());
        assert!(
            srm_json_for(&[("a".to_string(), 0), ("b".to_string(), 0)]).is_none(),
            "zero total → no SRM"
        );
    }

    // ── run_stats_compute SRM attachment + round-trip (891) ───────────────────

    /// Fake [`CellReader`] returning fixed per-variant cells + assignment
    /// counts, so `run_stats_compute`'s SRM attachment can be exercised with no
    /// ClickHouse. Sequential is disabled so `ch` is never queried.
    struct FakeReader {
        counts: Vec<(String, u64)>,
        successes: HashMap<String, u64>,
    }

    #[async_trait]
    impl CellReader for FakeReader {
        async fn assignment_counts(
            &self,
            _e: Uuid,
            _i: Uuid,
            _v: Uuid,
            _ct: &str,
            _end: DateTime<Utc>,
        ) -> Result<Vec<(String, u64)>, anyhow::Error> {
            Ok(self.counts.clone())
        }
        async fn aggregation_cells(
            &self,
            _cfg: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _v: Uuid,
            _ct: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, AggCell>, anyhow::Error> {
            Ok(self
                .counts
                .iter()
                .map(|(vk, n)| {
                    (
                        vk.clone(),
                        AggCell {
                            event_n: *n,
                            successes: *self.successes.get(vk).unwrap_or(&0),
                            value_sum: 0.0,
                            value_sq_sum: 0.0,
                        },
                    )
                })
                .collect())
        }
        async fn ratio_cells(
            &self,
            _n: &AggregationConfig,
            _d: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _v: Uuid,
            _ct: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, RatioGroupStats>, anyhow::Error> {
            Ok(HashMap::new())
        }
        async fn funnel_cells(
            &self,
            _cfg: &stitchd_core::metric::FunnelConfig,
            _e: Uuid,
            _i: Uuid,
            _v: Uuid,
            _ct: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, (u64, u64)>, anyhow::Error> {
            Ok(HashMap::new())
        }
        async fn percentile_samples(
            &self,
            _cfg: &AggregationConfig,
            _e: Uuid,
            _i: Uuid,
            _v: Uuid,
            _ct: &str,
            _end: DateTime<Utc>,
        ) -> Result<HashMap<String, Vec<f64>>, anyhow::Error> {
            Ok(HashMap::new())
        }
    }

    /// End-to-end (no CH): the compute pass attaches the SRM JSON under
    /// `variant_stats["srm"]` AND keeps the recommendation free of any SRM text.
    /// The attached JSON is then parsed by the SAME logic the
    /// experimentation-service read uses (mirrored here as `parse_srm`) into a
    /// populated SRM result — the round-trip proof for bead 891.
    #[tokio::test]
    async fn run_stats_compute_attaches_srm_json_and_clean_recommendation() {
        use crate::scheduler::SequentialSettings;

        let env_id = Uuid::new_v4();
        let metric = agg_metric(AggregationOperator::Count);
        let metric_id = metric.id.as_uuid();
        let mut metrics = HashMap::new();
        metrics.insert(metric_id, metric);

        let exp = RunningExperiment {
            experiment_id: Uuid::new_v4(),
            env_id,
            iteration_id: Uuid::new_v4(),
            metric_ids: vec![metric_id],
            variant_keys: vec!["control".into(), "treatment".into()],
            started_at: Utc::now(),
            unit_context_types: vec!["user".into()],
            pre_period_days: 0,
            sequential: SequentialSettings {
                enabled: false,
                alpha: 0.05,
                tau_squared: None,
                min_sample_size: 0,
            },
        };

        // Heavy 800/1200 mismatch → SRM should be RED.
        let reader = FakeReader {
            counts: vec![("control".into(), 800), ("treatment".into(), 1200)],
            successes: HashMap::from([("control".into(), 80), ("treatment".into(), 150)]),
        };

        // `ch` is never queried (sequential disabled); a default client suffices.
        let ch = Client::default();
        let now = Utc::now();
        let summaries = run_stats_compute(&reader, &ch, &exp, &metrics, now, now)
            .await
            .expect("compute pass succeeds without CH I/O");

        assert_eq!(summaries.len(), 1, "one (metric, user) summary");
        let s = &summaries[0];

        // (a) recommendation carries NO srm text any more.
        assert!(
            !s.recommendation.contains("srm"),
            "recommendation must not embed SRM text; got {}",
            s.recommendation
        );

        // (b) variant_stats carries the dedicated `srm` field.
        let srm_val = s
            .variant_stats
            .get("srm")
            .expect("variant_stats carries top-level srm");

        // (c) round-trip: parse with the SAME field reads as the
        // experimentation-service `srm_json_to_proto`.
        let parsed = parse_srm(srm_val).expect("srm json parses");
        assert_eq!(parsed.health, "red");
        assert!((parsed.overall_chi_sq - 80.0).abs() < 1e-6);
        assert_eq!(parsed.per_variant.len(), 2);
        let control = parsed
            .per_variant
            .iter()
            .find(|p| p.variant_key == "control")
            .expect("control row");
        assert_eq!(control.observed, 800);
        assert!((control.expected - 1000.0).abs() < 1e-9);
        assert!((control.chi_sq_contribution - 40.0).abs() < 1e-6);
    }

    /// Minimal mirror of `experimentation-service::service::srm_json_to_proto`'s
    /// field reads — proves the emitted JSON is consumable by that read without
    /// a cross-crate dependency. (The real fn returns a proto `SrmResult`; this
    /// returns the equivalent plain struct using the identical key/type accessors.)
    struct ParsedSrm {
        per_variant: Vec<ParsedSrmVariant>,
        overall_chi_sq: f64,
        health: String,
    }
    struct ParsedSrmVariant {
        variant_key: String,
        observed: u64,
        expected: f64,
        chi_sq_contribution: f64,
    }
    fn parse_srm(val: &Value) -> Option<ParsedSrm> {
        let obj = val.as_object()?;
        let per_variant = obj
            .get("per_variant")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|row| ParsedSrmVariant {
                        variant_key: row
                            .get("variant_key")
                            .and_then(|s| s.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        observed: row.get("observed").and_then(Value::as_u64).unwrap_or(0),
                        expected: row.get("expected").and_then(Value::as_f64).unwrap_or(0.0),
                        chi_sq_contribution: row
                            .get("chi_sq_contribution")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(ParsedSrm {
            per_variant,
            overall_chi_sq: obj
                .get("overall_chi_sq")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            health: obj
                .get("health")
                .and_then(|s| s.as_str())
                .unwrap_or("green")
                .to_lowercase(),
        })
    }
}
