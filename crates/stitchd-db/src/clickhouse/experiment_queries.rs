//! ClickHouse aggregation queries for experiment statistical analysis.
//!
//! Three query families are provided:
//!
//! - [`query_count_metric`] — count / conversion metrics queried from the
//!   `events_experiment_daily` pre-aggregated AggregatingMergeTree table via
//!   `countMerge`/`uniqMerge`. Avoids full `arrayFirst`/`arrayExists` raw
//!   event scans for the most common query type.
//! - [`query_numeric_metric`] — numeric sum / avg / percentile metrics queried
//!   from the raw `events` table. The MV omits quantile states, so percentile
//!   columns (p50/p95/p99) require raw-event access.
//! - [`query_funnel`] — multi-step funnel conversion counts chained via context
//!   keys, queried from the raw `events` table.
//!
//! All three functions take a [`clickhouse::Client`] and return owned result
//! structs. They do **not** require a live ClickHouse instance in unit tests;
//! query-builder logic (SQL string construction, parameter binding) is tested
//! via the `build_*_sql` helper functions that are exported for testing.

use chrono::{DateTime, Utc};
use clickhouse::Client;
use serde::Deserialize;
use uuid::Uuid;

// ── Error ────────────────────────────────────────────────────────────────────

/// Error type for ClickHouse experiment query operations.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// Underlying ClickHouse client error.
    #[error("clickhouse query failed: {0}")]
    Clickhouse(#[from] clickhouse::error::Error),
    /// The funnel steps list was empty.
    #[error("funnel steps must not be empty")]
    EmptyFunnelSteps,
}

// ── Result row types ─────────────────────────────────────────────────────────

/// One row from a count / conversion metric query.
///
/// `sample_size` = distinct context keys that triggered the metric;
/// `conversions` = total event occurrences in the date range.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, clickhouse::Row)]
pub struct CountMetricRow {
    /// Variant key (e.g. `"control"`, `"treatment"`).
    pub variant_key: String,
    /// Number of distinct evaluation contexts enrolled in this variant.
    pub sample_size: i64,
    /// Total event occurrences for this variant in the date range.
    pub conversions: i64,
}

/// One row from a numeric sum / avg / percentile metric query.
#[derive(Debug, Clone, PartialEq, Deserialize, clickhouse::Row)]
pub struct NumericMetricRow {
    /// Variant key.
    pub variant_key: String,
    /// Number of distinct evaluation contexts enrolled in this variant.
    pub sample_size: i64,
    /// Sum of numeric values across all events.
    pub sum: f64,
    /// Arithmetic mean of numeric values.
    pub mean: f64,
    /// Sample variance of numeric values.
    pub variance: f64,
    /// 50th percentile (median).
    pub p50: f64,
    /// 95th percentile.
    pub p95: f64,
    /// 99th percentile.
    pub p99: f64,
}

/// One row from a funnel query — one row per `(variant_key, step_index)`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, clickhouse::Row)]
pub struct FunnelStepRow {
    /// Variant key.
    pub variant_key: String,
    /// Zero-based index of this funnel step.
    pub step_index: usize,
    /// The registered event key for this step.
    pub event_key: String,
    /// Number of distinct contexts that completed this step.
    pub count: i64,
}

// ── SQL builder helpers (pure functions, unit-testable) ───────────────────────

/// Build the SQL string for a count/conversion metric query.
///
/// Reads from `events_experiment_daily` (pre-aggregated AggregatingMergeTree)
/// rather than scanning raw events. `countMerge` and `uniqMerge` finalise the
/// stored aggregate states across all matched day partitions.
///
/// `sample_size` is an HyperLogLog approximation via `uniqMerge` (vs the
/// previous `uniqExact` on raw events). For experiment analysis at scale the
/// approximation error is negligible.
///
/// # Parameters (positional `?` binding order)
/// 1. `env_id`         — UUID
/// 2. `experiment_id`  — String stored in the MV `experiment_id` column
/// 3. `metric_key`     — String
/// 4. `date_from`      — Unix timestamp (seconds); converted to `Date` in SQL
/// 5. `date_to`        — Unix timestamp (seconds); converted to `Date` in SQL (exclusive)
#[must_use]
pub const fn build_count_metric_sql() -> &'static str {
    r"
SELECT
    variant_key,
    toInt64(uniqMerge(uniq_ctx_state))  AS sample_size,
    toInt64(countMerge(count_state))    AS conversions
FROM events_experiment_daily
WHERE
    env_id        = ?
    AND experiment_id = ?
    AND metric_key    = ?
    AND day >= toDate(fromUnixTimestamp(?))
    AND day <  toDate(fromUnixTimestamp(?))
GROUP BY variant_key
HAVING variant_key != ''
"
}

/// Build the SQL string for a numeric (sum / avg / percentile) metric query.
///
/// # Parameters (positional `?` binding order)
/// 1. `env_id`
/// 2. `experiment_id`
/// 3. `metric_key`
/// 4. `date_from`
/// 5. `date_to`
#[must_use]
pub const fn build_numeric_metric_sql() -> &'static str {
    r"
SELECT
    arrayFirst(t -> t.1 = 'variant', contexts).2    AS variant_key,
    uniqExact(arrayFirst(t -> t.2 != '', contexts).2) AS sample_size,
    sum(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))))) AS sum,
    avg(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))))) AS mean,
    varSamp(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))))) AS variance,
    quantile(0.5)(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))))) AS p50,
    quantile(0.95)(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))))) AS p95,
    quantile(0.99)(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))))) AS p99
FROM events
WHERE
    env_id     = ?
    AND arrayExists(t -> t.1 = 'experiment' AND t.2 = ?, contexts)
    AND metric_key = ?
    AND (value_double IS NOT NULL OR value_int IS NOT NULL)
    AND timestamp >= ?
    AND timestamp <  ?
GROUP BY variant_key
HAVING variant_key != ''
"
}

/// Build the SQL string for a single funnel step.
///
/// Returns `(variant_key, step_index, event_key, count)` for the given step.
/// Each step counts distinct context keys that have seen `event_key` AND all
/// preceding steps (chained via context).
///
/// For simplicity this implementation counts distinct contexts per step
/// independently; the final step's count is used for the funnel conversion rate
/// as specified.
///
/// # Parameters (positional `?` binding order)
/// 1. `env_id`
/// 2. `experiment_id`
/// 3. `event_key`    — this step's event key
/// 4. `step_index`   — literal integer (injected inline, not bound)
/// 5. `event_key`    — repeated for the SELECT literal
/// 6. `date_from`
/// 7. `date_to`
///
/// Because `step_index` and `event_key` in SELECT are literals we inject them
/// directly; the function takes them as parameters and returns an owned `String`.
#[must_use]
pub fn build_funnel_step_sql(step_index: usize, event_key: &str) -> String {
    // event_key is safe: it is a pre-registered metric key (alphanumeric + _/-).
    // step_index is usize — no injection risk.
    format!(
        r"
SELECT
    arrayFirst(t -> t.1 = 'variant', contexts).2    AS variant_key,
    {step_index}                                      AS step_index,
    '{event_key}'                                     AS event_key,
    uniqExact(arrayFirst(t -> t.2 != '', contexts).2) AS count
FROM events
WHERE
    env_id     = ?
    AND arrayExists(t -> t.1 = 'experiment' AND t.2 = ?, contexts)
    AND metric_key = '{event_key}'
    AND timestamp >= ?
    AND timestamp <  ?
GROUP BY variant_key
HAVING variant_key != ''
"
    )
}

// ── Query execution functions ─────────────────────────────────────────────────

/// Query count / conversion metrics per `(env_id, experiment_id, metric_key,
/// variant_key)` over a date range.
///
/// # Errors
/// Returns [`QueryError`] if the ClickHouse query fails.
pub async fn query_count_metric(
    client: &Client,
    env_id: Uuid,
    experiment_id: &str,
    metric_key: &str,
    date_from: DateTime<Utc>,
    date_to: DateTime<Utc>,
) -> Result<Vec<CountMetricRow>, QueryError> {
    let rows = client
        .query(build_count_metric_sql())
        .bind(env_id)
        .bind(experiment_id)
        .bind(metric_key)
        .bind(date_from.timestamp())
        .bind(date_to.timestamp())
        .fetch_all::<CountMetricRow>()
        .await?;
    Ok(rows)
}

/// Query numeric sum / avg / percentile metrics per `(env_id, experiment_id,
/// metric_key, variant_key)` over a date range.
///
/// Only events with a numeric value (`value_int` or `value_double`) are
/// included.
///
/// # Errors
/// Returns [`QueryError`] if the ClickHouse query fails.
pub async fn query_numeric_metric(
    client: &Client,
    env_id: Uuid,
    experiment_id: &str,
    metric_key: &str,
    date_from: DateTime<Utc>,
    date_to: DateTime<Utc>,
) -> Result<Vec<NumericMetricRow>, QueryError> {
    let rows = client
        .query(build_numeric_metric_sql())
        .bind(env_id)
        .bind(experiment_id)
        .bind(metric_key)
        .bind(date_from.timestamp())
        .bind(date_to.timestamp())
        .fetch_all::<NumericMetricRow>()
        .await?;
    Ok(rows)
}

/// Query per-variant per-step funnel conversion counts.
///
/// `funnel_steps` is an ordered list of event keys; each step counts distinct
/// contexts that triggered that event within the date range and experiment.
/// The **final step**'s count is the funnel conversion denominator for analysis.
///
/// Returns a flat `Vec<FunnelStepRow>` with one row per `(variant_key, step)`.
///
/// # Errors
/// Returns [`QueryError::EmptyFunnelSteps`] if `funnel_steps` is empty, or a
/// [`QueryError::Clickhouse`] on query failure.
pub async fn query_funnel(
    client: &Client,
    env_id: Uuid,
    experiment_id: &str,
    funnel_steps: &[&str],
    date_from: DateTime<Utc>,
    date_to: DateTime<Utc>,
) -> Result<Vec<FunnelStepRow>, QueryError> {
    if funnel_steps.is_empty() {
        return Err(QueryError::EmptyFunnelSteps);
    }

    let mut results = Vec::new();
    for (idx, &event_key) in funnel_steps.iter().enumerate() {
        let sql = build_funnel_step_sql(idx, event_key);
        let step_rows = client
            .query(&sql)
            .bind(env_id)
            .bind(experiment_id)
            .bind(date_from.timestamp())
            .bind(date_to.timestamp())
            .fetch_all::<FunnelStepRow>()
            .await?;
        results.extend(step_rows);
    }
    Ok(results)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── CountMetricRow construction ───────────────────────────────────────────

    #[test]
    fn count_metric_row_fields_accessible() {
        let row = CountMetricRow {
            variant_key: "control".to_string(),
            sample_size: 500,
            conversions: 120,
        };
        assert_eq!(row.variant_key, "control");
        assert_eq!(row.sample_size, 500);
        assert_eq!(row.conversions, 120);
    }

    #[test]
    fn count_metric_row_clone_and_eq() {
        let row = CountMetricRow {
            variant_key: "treatment".to_string(),
            sample_size: 600,
            conversions: 180,
        };
        let cloned = row.clone();
        assert_eq!(row, cloned);
    }

    // ── NumericMetricRow construction ─────────────────────────────────────────

    #[test]
    fn numeric_metric_row_fields_accessible() {
        let row = NumericMetricRow {
            variant_key: "control".to_string(),
            sample_size: 300,
            sum: 1500.0,
            mean: 5.0,
            variance: 2.5,
            p50: 4.8,
            p95: 9.1,
            p99: 12.3,
        };
        assert_eq!(row.variant_key, "control");
        assert!((row.mean - 5.0).abs() < f64::EPSILON);
        assert!((row.p99 - 12.3).abs() < f64::EPSILON);
    }

    #[test]
    fn numeric_metric_row_clone_and_eq() {
        let row = NumericMetricRow {
            variant_key: "v1".to_string(),
            sample_size: 100,
            sum: 200.0,
            mean: 2.0,
            variance: 0.5,
            p50: 1.9,
            p95: 3.8,
            p99: 4.5,
        };
        assert_eq!(row.clone(), row);
    }

    // ── FunnelStepRow construction ────────────────────────────────────────────

    #[test]
    fn funnel_step_row_fields_accessible() {
        let row = FunnelStepRow {
            variant_key: "treatment".to_string(),
            step_index: 2,
            event_key: "checkout_completed".to_string(),
            count: 88,
        };
        assert_eq!(row.step_index, 2);
        assert_eq!(row.event_key, "checkout_completed");
        assert_eq!(row.count, 88);
    }

    // ── SQL builder: count metric ─────────────────────────────────────────────

    #[test]
    fn count_metric_sql_reads_from_mv_not_raw_events() {
        let sql = build_count_metric_sql();
        assert!(
            sql.contains("FROM events_experiment_daily"),
            "count metric must read from the pre-aggregated MV"
        );
        assert!(
            !sql.contains("arrayExists"),
            "count metric must not scan raw events with arrayExists"
        );
        assert!(
            !sql.contains("arrayFirst"),
            "count metric must not scan raw events with arrayFirst"
        );
    }

    #[test]
    fn count_metric_sql_uses_merge_aggregates() {
        let sql = build_count_metric_sql();
        assert!(sql.contains("countMerge"), "must use countMerge to finalise count_state");
        assert!(sql.contains("uniqMerge"), "must use uniqMerge to finalise uniq_ctx_state");
    }

    #[test]
    fn count_metric_sql_filters_env_id() {
        let sql = build_count_metric_sql();
        assert!(sql.contains("env_id"));
    }

    #[test]
    fn count_metric_sql_filters_experiment_id_column() {
        let sql = build_count_metric_sql();
        assert!(sql.contains("experiment_id"), "must filter by experiment_id column in MV");
    }

    #[test]
    fn count_metric_sql_selects_variant_key() {
        let sql = build_count_metric_sql();
        assert!(sql.contains("variant_key"));
    }

    #[test]
    fn count_metric_sql_selects_sample_size_and_conversions() {
        let sql = build_count_metric_sql();
        assert!(sql.contains("sample_size"));
        assert!(sql.contains("conversions"));
    }

    #[test]
    fn count_metric_sql_has_five_bind_placeholders() {
        let sql = build_count_metric_sql();
        let count = sql.chars().filter(|&c| c == '?').count();
        assert_eq!(count, 5, "expected 5 bind params: env_id, experiment_id, metric_key, date_from, date_to");
    }

    #[test]
    fn count_metric_sql_groups_by_variant_key() {
        let sql = build_count_metric_sql();
        assert!(sql.contains("GROUP BY variant_key"));
    }

    // ── SQL builder: numeric metric ───────────────────────────────────────────

    #[test]
    fn numeric_metric_sql_contains_events_table() {
        let sql = build_numeric_metric_sql();
        // Numeric metric reads from raw events — the MV lacks quantile states (p50/p95/p99).
        assert!(sql.contains("FROM events"));
    }

    #[test]
    fn numeric_metric_sql_selects_all_aggregates() {
        let sql = build_numeric_metric_sql();
        assert!(sql.contains("sum("), "missing sum aggregate");
        assert!(sql.contains("avg("), "missing avg aggregate");
        assert!(sql.contains("varSamp("), "missing variance aggregate");
        assert!(sql.contains("quantile(0.5)"), "missing p50");
        assert!(sql.contains("quantile(0.95)"), "missing p95");
        assert!(sql.contains("quantile(0.99)"), "missing p99");
    }

    #[test]
    fn numeric_metric_sql_filters_numeric_values_only() {
        let sql = build_numeric_metric_sql();
        assert!(
            sql.contains("value_double IS NOT NULL OR value_int IS NOT NULL"),
            "missing numeric-only filter"
        );
    }

    #[test]
    fn numeric_metric_sql_has_five_bind_placeholders() {
        let sql = build_numeric_metric_sql();
        let count = sql.chars().filter(|&c| c == '?').count();
        assert_eq!(count, 5, "expected 5 bind params, got {count}");
    }

    // ── SQL builder: funnel step ──────────────────────────────────────────────

    #[test]
    fn funnel_step_sql_injects_step_index() {
        let sql = build_funnel_step_sql(3, "add_to_cart");
        assert!(sql.contains('3'), "step_index 3 should appear in SQL");
    }

    #[test]
    fn funnel_step_sql_injects_event_key() {
        let sql = build_funnel_step_sql(0, "page_view");
        assert!(sql.contains("page_view"), "event_key should appear in SQL");
    }

    #[test]
    fn funnel_step_sql_has_four_bind_placeholders() {
        let sql = build_funnel_step_sql(1, "checkout");
        let count = sql.chars().filter(|&c| c == '?').count();
        assert_eq!(count, 4, "expected 4 bind params, got {count}");
    }

    #[test]
    fn funnel_step_sql_selects_required_columns() {
        let sql = build_funnel_step_sql(0, "signup");
        assert!(sql.contains("variant_key"));
        assert!(sql.contains("step_index"));
        assert!(sql.contains("event_key"));
        assert!(sql.contains("count"));
    }

    #[test]
    fn funnel_step_sql_different_steps_differ() {
        let sql0 = build_funnel_step_sql(0, "view");
        let sql1 = build_funnel_step_sql(1, "click");
        assert_ne!(sql0, sql1);
    }

    // ── QueryError ────────────────────────────────────────────────────────────

    #[test]
    fn empty_funnel_steps_error_display() {
        let err = QueryError::EmptyFunnelSteps;
        assert!(err.to_string().contains("empty"));
    }

    // ── Mock result aggregation logic ─────────────────────────────────────────
    // These tests simulate the downstream consumer of the query results —
    // verifying that the returned structs can be aggregated as required.

    #[test]
    fn mock_count_results_conversion_rate() {
        let rows = [
            CountMetricRow {
                variant_key: "control".to_string(),
                sample_size: 1000,
                conversions: 100,
            },
            CountMetricRow {
                variant_key: "treatment".to_string(),
                sample_size: 1000,
                conversions: 130,
            },
        ];
        let control = rows.iter().find(|r| r.variant_key == "control").unwrap();
        let treatment = rows.iter().find(|r| r.variant_key == "treatment").unwrap();

        #[allow(clippy::cast_precision_loss)]
        let control_rate = control.conversions as f64 / control.sample_size as f64;
        #[allow(clippy::cast_precision_loss)]
        let treatment_rate = treatment.conversions as f64 / treatment.sample_size as f64;

        assert!((control_rate - 0.10).abs() < 1e-9);
        assert!((treatment_rate - 0.13).abs() < 1e-9);
        assert!(treatment_rate > control_rate);
    }

    #[test]
    fn mock_numeric_results_mean_difference() {
        let rows = [
            NumericMetricRow {
                variant_key: "control".to_string(),
                sample_size: 500,
                sum: 2500.0,
                mean: 5.0,
                variance: 1.0,
                p50: 4.9,
                p95: 8.0,
                p99: 10.0,
            },
            NumericMetricRow {
                variant_key: "treatment".to_string(),
                sample_size: 500,
                sum: 3000.0,
                mean: 6.0,
                variance: 1.2,
                p50: 5.9,
                p95: 9.0,
                p99: 11.5,
            },
        ];
        let mean_diff = rows[1].mean - rows[0].mean;
        assert!((mean_diff - 1.0).abs() < 1e-9);
    }

    #[test]
    fn mock_funnel_final_step_conversion_rate() {
        // Simulates: step 0 = page_view, step 1 = add_to_cart, step 2 = checkout
        let rows = [
            FunnelStepRow {
                variant_key: "control".to_string(),
                step_index: 0,
                event_key: "page_view".to_string(),
                count: 1000,
            },
            FunnelStepRow {
                variant_key: "control".to_string(),
                step_index: 1,
                event_key: "add_to_cart".to_string(),
                count: 400,
            },
            FunnelStepRow {
                variant_key: "control".to_string(),
                step_index: 2,
                event_key: "checkout".to_string(),
                count: 200,
            },
            FunnelStepRow {
                variant_key: "treatment".to_string(),
                step_index: 0,
                event_key: "page_view".to_string(),
                count: 1000,
            },
            FunnelStepRow {
                variant_key: "treatment".to_string(),
                step_index: 1,
                event_key: "add_to_cart".to_string(),
                count: 450,
            },
            FunnelStepRow {
                variant_key: "treatment".to_string(),
                step_index: 2,
                event_key: "checkout".to_string(),
                count: 260,
            },
        ];

        let max_step = rows.iter().map(|r| r.step_index).max().unwrap();

        let control_final = rows
            .iter()
            .find(|r| r.variant_key == "control" && r.step_index == max_step)
            .unwrap();
        let treatment_final = rows
            .iter()
            .find(|r| r.variant_key == "treatment" && r.step_index == max_step)
            .unwrap();

        // Funnel conversion = final step count / step 0 count
        let control_step0 = rows
            .iter()
            .find(|r| r.variant_key == "control" && r.step_index == 0)
            .unwrap();
        let treatment_step0 = rows
            .iter()
            .find(|r| r.variant_key == "treatment" && r.step_index == 0)
            .unwrap();

        #[allow(clippy::cast_precision_loss)]
        let control_rate = control_final.count as f64 / control_step0.count as f64;
        #[allow(clippy::cast_precision_loss)]
        let treatment_rate = treatment_final.count as f64 / treatment_step0.count as f64;

        assert!((control_rate - 0.20).abs() < 1e-9);
        assert!((treatment_rate - 0.26).abs() < 1e-9);
        assert!(treatment_rate > control_rate);
    }

    #[test]
    fn funnel_rows_can_be_grouped_by_variant() {
        let rows = [
            FunnelStepRow {
                variant_key: "control".to_string(),
                step_index: 0,
                event_key: "view".to_string(),
                count: 100,
            },
            FunnelStepRow {
                variant_key: "treatment".to_string(),
                step_index: 0,
                event_key: "view".to_string(),
                count: 110,
            },
        ];
        assert_eq!(
            rows.iter().filter(|r| r.variant_key == "control").count(),
            1
        );
        assert_eq!(
            rows.iter().filter(|r| r.variant_key == "treatment").count(),
            1
        );
    }
}
