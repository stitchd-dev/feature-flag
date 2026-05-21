//! Preview-mode query builders.
//!
//! Two families of **day-bucketed time-series** queries live in this
//! module:
//!
//! 1. **Standalone preview** (`build_preview_*_query`) — backs the
//!    metric-preview UI (`POST /v1/metrics/{id}/preview`). Returns one
//!    row per day for ALL events in the environment matching the
//!    metric's `event_key`, irrespective of experiment membership.
//!
//! 2. **Experiment-scoped preview** (`build_experiment_preview_*_query`)
//!    — backs the experiment Detail's "Daily time-series" tab. Same
//!    day-bucketed shape, but each row is also keyed by
//!    `(context_type, variant_key)` derived from `experiment_assignments`.
//!    Returns one row per `(day, context_type, variant_key)` so the UI
//!    can render per-variant sparklines per context type.
//!
//! ## Shape — standalone preview
//!
//! ```sql
//! SELECT toUnixTimestamp(toStartOfDay(e.timestamp, 'UTC')) AS day_ts,
//!        <kind-specific value expression>                  AS value
//! FROM events_v2 AS e
//! WHERE e.env_id = toUUID({env_ph})
//!   AND e.metric_key = {event_ph}              -- aggregation / funnel
//!   AND e.timestamp >= now() - toIntervalDay({days_ph})
//! GROUP BY day_ts
//! ORDER BY day_ts ASC
//! ```
//!
//! ## Shape — experiment-scoped preview
//!
//! ```sql
//! SELECT toUnixTimestamp(toStartOfDay(e.occurred_at, 'UTC')) AS day_ts,
//!        a.context_type AS context_type,
//!        a.variant_key  AS variant_key,
//!        <kind-specific value expression> AS value
//! FROM events_v2 AS e
//! INNER JOIN experiment_assignments AS a
//!     ON e.env_id = a.env_id
//!    AND arrayExists(t -> t.1 = a.context_type AND t.2 = a.context_key, e.contexts)
//! WHERE a.env_id = toUUID(?)
//!   AND a.experiment_id = toUUID(?)
//!   AND a.iteration_id  = toUUID(?)
//!   AND e.metric_key    = ?
//!   AND e.occurred_at  >= a.assigned_at
//!   AND e.occurred_at  <  fromUnixTimestamp64Milli(?)
//! GROUP BY day_ts, a.context_type, a.variant_key
//! ORDER BY day_ts ASC, a.context_type, a.variant_key
//! ```
//!
//! The caller is responsible for zero-filling missing days in Rust (the
//! SQL returns only days that actually saw events).
//!
//! Internal helpers (`render_aggregator`, `on_field_expr`) are
//! `pub(super)`-shared from [`super::aggregation`] so the aggregator-to-CH
//! mapping has a single source of truth.

use chrono::{DateTime, Utc};
use stitchd_core::metric::{AggregationConfig, FunnelConfig, RatioConfig};

use super::{
    BuiltQuery, QueryBind, QueryBuildError, aggregation::render_aggregator, jsonlogic_to_sql,
    push_bind,
};

// ── Aggregation ──────────────────────────────────────────────────────────────

/// Build a day-bucketed preview query for an aggregation metric.
///
/// The aggregator (`count` / `sum` / `avg` / `p50` / `p90` / `p99` /
/// `uniq`) is rendered via the same `render_aggregator` helper used by
/// the experiment-scoped builder, so any change to the underlying CH
/// expression flows to both code paths.
///
/// # Errors
/// - `QueryBuildError::InvalidConfig` when an aggregator requires an
///   `on_field` and none is set.
/// - `QueryBuildError::UnsupportedJsonLogic` when the optional
///   `where_clause` uses an operator we don't translate.
pub fn build_preview_aggregation_query(
    cfg: &AggregationConfig,
    env_id: &str,
    days: u32,
) -> Result<BuiltQuery, QueryBuildError> {
    if cfg.aggregator.requires_field() && cfg.on_field.is_none() {
        return Err(QueryBuildError::InvalidConfig(format!(
            "aggregator `{:?}` requires an on_field",
            cfg.aggregator
        )));
    }

    let mut binds = Vec::new();

    let env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let event_ph = push_bind(&mut binds, QueryBind::Str(cfg.event_key.clone()));
    let days_ph = push_bind(&mut binds, QueryBind::I64(i64::from(days)));

    let extra_where = match cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n  AND ({frag})")
        }
        None => String::new(),
    };

    let agg_expr = render_aggregator(cfg)?;

    // Coerce the aggregator output to `Nullable(Float64)` so the wire shape
    // matches the PreviewRow's `value: Option<f64>` across every kind.
    // `count()` is `UInt64`, `sum/avg/quantile` are `Float64`, `uniq` is
    // `UInt64` — all coerce cleanly to a nullable float. Without this the
    // RowBinary deserialiser hits "tag for enum is not valid" because the
    // null-marker byte expected by `Option<f64>` isn't present for
    // non-nullable columns.
    // Alias events_v2 as `e` so the shared `render_aggregator` (which
    // emits `e.value_double` / `e.properties[...]` to disambiguate from
    // the experiment-scoped JOIN against `experiment_assignments AS a`
    // in `queries::aggregation`) resolves correctly here too. The alias
    // is purely cosmetic for preview — there is no JOIN — but keeping
    // the column references uniform avoids forking the aggregator
    // expression mapping.
    let sql = format!(
        "SELECT\n    \
            toUnixTimestamp(toStartOfDay(e.timestamp, 'UTC')) AS day_ts,\n    \
            CAST({agg_expr} AS Nullable(Float64)) AS value\n\
        FROM events_v2 AS e\n\
        WHERE e.env_id = toUUID({env_ph})\n  \
          AND e.metric_key = {event_ph}\n  \
          AND e.timestamp >= now() - toIntervalDay({days_ph}){extra_where}\n\
        GROUP BY day_ts\n\
        ORDER BY day_ts ASC"
    );

    Ok(BuiltQuery { sql, binds })
}

// ── Ratio ────────────────────────────────────────────────────────────────────

/// Build a day-bucketed preview query for a ratio metric.
///
/// Emits three CTEs (numerator, denominator, denominator_count) keyed by
/// `day_ts` and joins them. Per-day rows where the denominator count is
/// below `min_denominator` are dropped — the UI plots the remaining
/// buckets and the zero-fill pass in the handler turns the gaps into
/// zeros, matching the "insufficient data" semantics.
///
/// `numerator_cfg` and `denominator_cfg` are the already-resolved
/// [`AggregationConfig`]s for the metric ids in `ratio_cfg` (resolution
/// happens in the dispatcher).
///
/// # Errors
/// Propagates [`QueryBuildError`]s from the underlying preview
/// aggregation builder for either leg.
pub fn build_preview_ratio_query(
    ratio_cfg: &RatioConfig,
    numerator_cfg: &AggregationConfig,
    denominator_cfg: &AggregationConfig,
    env_id: &str,
    days: u32,
) -> Result<BuiltQuery, QueryBuildError> {
    let num_q = build_preview_aggregation_query(numerator_cfg, env_id, days)?;
    let den_q = build_preview_aggregation_query(denominator_cfg, env_id, days)?;

    // Denominator *count* — required for the min_denominator guard
    // regardless of the denominator metric's own aggregator. Same trick
    // as the experiment-scoped ratio builder.
    let denom_count_cfg = AggregationConfig {
        event_key: denominator_cfg.event_key.clone(),
        aggregator: stitchd_core::metric::AggregationOperator::Count,
        on_field: None,
        where_clause: denominator_cfg.where_clause.clone(),
    };
    let den_count_q = build_preview_aggregation_query(&denom_count_cfg, env_id, days)?;

    // Shift placeholder indices so the concatenated bind vec lines up.
    let den_offset = num_q.binds.len();
    let den_count_offset = den_offset + den_q.binds.len();
    let den_sql_shifted = shift_placeholders(&den_q.sql, den_offset);
    let den_count_sql_shifted = shift_placeholders(&den_count_q.sql, den_count_offset);

    let mut binds =
        Vec::with_capacity(num_q.binds.len() + den_q.binds.len() + den_count_q.binds.len() + 1);
    binds.extend(num_q.binds);
    binds.extend(den_q.binds);
    binds.extend(den_count_q.binds);
    let min_den_ph = push_bind(&mut binds, QueryBind::I64(ratio_cfg.min_denominator));

    let sql = format!(
        "WITH numerator AS (\n\
        {num_sql}\n\
        ), denominator AS (\n\
        {den_sql}\n\
        ), denominator_count AS (\n\
        {den_count_sql}\n\
        )\n\
        SELECT\n    \
            numerator.day_ts AS day_ts,\n    \
            numerator.value / nullIf(denominator.value, 0) AS value\n\
        FROM numerator\n\
        INNER JOIN denominator ON numerator.day_ts = denominator.day_ts\n\
        INNER JOIN denominator_count ON numerator.day_ts = denominator_count.day_ts\n\
        WHERE denominator_count.value >= {min_den_ph}\n\
        ORDER BY day_ts ASC",
        num_sql = indent_lines(&num_q.sql),
        den_sql = indent_lines(&den_sql_shifted),
        den_count_sql = indent_lines(&den_count_sql_shifted),
    );

    Ok(BuiltQuery { sql, binds })
}

// ── Funnel ───────────────────────────────────────────────────────────────────

/// Build a day-bucketed preview query for a funnel metric.
///
/// Each `(day_ts, context_key)` gets a `windowFunnel` level. The outer
/// SELECT aggregates per `day_ts` and emits the final-step conversion
/// rate — `countIf(level >= N) / nullIf(countIf(level >= 1), 0)` where
/// N is the number of steps. PreviewBucket carries a single scalar per
/// day; the UI shows the overall funnel completion rate over time.
///
/// # Errors
/// - `QueryBuildError::InvalidConfig` for fewer than 2 steps or
///   non-positive `window_seconds`.
pub fn build_preview_funnel_query(
    cfg: &FunnelConfig,
    env_id: &str,
    days: u32,
) -> Result<BuiltQuery, QueryBuildError> {
    if cfg.steps.len() < 2 {
        return Err(QueryBuildError::InvalidConfig(format!(
            "funnel must have at least 2 steps (got {})",
            cfg.steps.len()
        )));
    }
    if cfg.window_seconds <= 0 {
        return Err(QueryBuildError::InvalidConfig(format!(
            "funnel window_seconds must be strictly positive (got {})",
            cfg.window_seconds
        )));
    }

    // clickhouse-rs binds `?` placeholders by SQL position, not by
    // bind-vector index. The rewriter (`rewrite_placeholders_to_clickhouse`)
    // converts `{pN}` → `?` but does NOT reorder the binds vec. So the
    // bind push order must match the order placeholders appear in the
    // final SQL string. The funnel SQL has placeholders in this order:
    //
    //   1. windowFunnel step predicates  (in SELECT) — N step event_keys
    //   2. env_id                        (in WHERE)
    //   3. days                          (in WHERE)
    //   4. metric_key filter             (in WHERE) — same N event_keys
    //
    // We push them in that exact order; each step's event_key is bound
    // twice (once for the predicates, once for the filter) because
    // clickhouse-rs needs one bind per `?` regardless of duplicate
    // logical values.
    let mut binds = Vec::new();

    let mut predicate_phs = Vec::with_capacity(cfg.steps.len());
    for step in &cfg.steps {
        predicate_phs.push(push_bind(
            &mut binds,
            QueryBind::Str(step.event_key.clone()),
        ));
    }
    let env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let days_ph = push_bind(&mut binds, QueryBind::I64(i64::from(days)));
    let mut filter_phs = Vec::with_capacity(cfg.steps.len());
    for step in &cfg.steps {
        filter_phs.push(push_bind(
            &mut binds,
            QueryBind::Str(step.event_key.clone()),
        ));
    }
    let metric_key_filter = filter_phs
        .iter()
        .map(|ph| format!("metric_key = {ph}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let step_predicates = predicate_phs
        .iter()
        .map(|ph| format!("metric_key = {ph}"))
        .collect::<Vec<_>>()
        .join(", ");

    // `windowFunnel(window[, 'strict_order'])`. `count_repeats=true`
    // disables strict_order (lets repeated firings advance the level).
    let mode_arg = if cfg.count_repeats {
        String::new()
    } else {
        ", 'strict_order'".to_owned()
    };
    let window = cfg.window_seconds;
    let final_step = cfg.steps.len() as i64; // level >= final_step == converted

    // Per-context, per-day funnel levels. We pick contexts[1] as the
    // dedup key (matches the firings/stats endpoint convention — first
    // dimension is "the user / actor"). Multi-context attribution is
    // honoured at ingest time; here we keep the same per-day uniqueness
    // semantics as the existing stats query so preview agrees with the
    // sparkline.
    let levels_cte = format!(
        "WITH levels AS (\n    \
            SELECT\n        \
                toStartOfDay(timestamp, 'UTC') AS day_bucket,\n        \
                contexts[1].2 AS dedup_key,\n        \
                windowFunnel({window}{mode_arg})(\n            \
                    toUInt32(toUnixTimestamp(timestamp)),\n            \
                    {step_predicates}\n        \
                ) AS level\n    \
            FROM events_v2\n    \
            WHERE env_id = toUUID({env_ph})\n      \
              AND timestamp >= now() - toIntervalDay({days_ph})\n      \
              AND ({metric_key_filter})\n    \
            GROUP BY day_bucket, dedup_key\n    \
            HAVING dedup_key != ''\n\
        )"
    );

    // Outer SELECT: one row per day with the final-step completion rate.
    let sql = format!(
        "{levels_cte}\n\
        SELECT\n    \
            toUnixTimestamp(day_bucket) AS day_ts,\n    \
            countIf(level >= {final_step}) / nullIf(countIf(level >= 1), 0) AS value\n\
        FROM levels\n\
        GROUP BY day_bucket\n\
        ORDER BY day_bucket ASC"
    );

    Ok(BuiltQuery { sql, binds })
}

// ── Experiment-scoped Aggregation ────────────────────────────────────────────

/// Build a day-bucketed preview query for an aggregation metric scoped
/// to an experiment iteration, broken out by `(context_type, variant_key)`.
///
/// Mirrors [`build_preview_aggregation_query`] but joins against
/// `experiment_assignments` so each row is attributed via the same
/// first-exposure ITT model as the headline experiment stats query
/// (`crate::queries::aggregation::build_aggregation_query`).
///
/// Bind push order (SQL-appearance):
///   1. env_id
///   2. experiment_id
///   3. iteration_id
///   4. event_key
///   5. iteration_end_ms
///   6. where_clause literals (last, emitted by `jsonlogic_to_sql`)
///
/// # Errors
/// - `QueryBuildError::InvalidConfig` when the aggregator requires an
///   `on_field` and none is set.
/// - `QueryBuildError::UnsupportedJsonLogic` when the optional
///   `where_clause` uses an operator outside the supported subset.
pub fn build_experiment_preview_aggregation_query(
    cfg: &AggregationConfig,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    iteration_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    if cfg.aggregator.requires_field() && cfg.on_field.is_none() {
        return Err(QueryBuildError::InvalidConfig(format!(
            "aggregator `{:?}` requires an on_field",
            cfg.aggregator
        )));
    }

    let mut binds = Vec::new();

    let env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let exp_ph = push_bind(&mut binds, QueryBind::Str(experiment_id.to_owned()));
    let iter_ph = push_bind(&mut binds, QueryBind::Str(iteration_id.to_owned()));
    let event_ph = push_bind(&mut binds, QueryBind::Str(cfg.event_key.clone()));
    let iter_end_ph = push_bind(&mut binds, QueryBind::I64(iteration_end.timestamp_millis()));

    let extra_where = match cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n  AND ({frag})")
        }
        None => String::new(),
    };

    let agg_expr = render_aggregator(cfg)?;

    // Coerce aggregator output to Nullable(Float64) so the wire shape
    // matches the PreviewRow's `value: Option<f64>` (uniform with the
    // standalone preview path).
    let sql = format!(
        "SELECT\n    \
            toUnixTimestamp(toStartOfDay(e.occurred_at, 'UTC')) AS day_ts,\n    \
            a.context_type AS context_type,\n    \
            a.variant_key AS variant_key,\n    \
            CAST({agg_expr} AS Nullable(Float64)) AS value\n\
        FROM events_v2 AS e\n\
        INNER JOIN experiment_assignments AS a\n    \
            ON e.env_id = a.env_id\n   \
            AND arrayExists(t -> t.1 = a.context_type AND t.2 = a.context_key, e.contexts)\n\
        WHERE a.env_id = toUUID({env_ph})\n  \
          AND a.experiment_id = toUUID({exp_ph})\n  \
          AND a.iteration_id = toUUID({iter_ph})\n  \
          AND e.metric_key = {event_ph}\n  \
          AND e.occurred_at >= a.assigned_at\n  \
          AND e.occurred_at < fromUnixTimestamp64Milli({iter_end_ph}){extra_where}\n\
        GROUP BY day_ts, a.context_type, a.variant_key\n\
        ORDER BY day_ts ASC, a.context_type, a.variant_key"
    );

    Ok(BuiltQuery { sql, binds })
}

// ── Internal helpers (parallel to ratio.rs's local helpers) ──────────────────

/// Rewrite every `{pN}` placeholder in `sql` by adding `offset` to `N`.
/// Lifted from `queries/ratio.rs:shift_placeholders` — same semantics,
/// keeps this module self-contained.
fn shift_placeholders(sql: &str, offset: usize) -> String {
    if offset == 0 {
        return sql.to_owned();
    }
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(open) = rest.find("{p") {
        out.push_str(&rest[..open]);
        let after_p = open + 2;
        // Scan past digits.
        let digit_end = rest[after_p..]
            .bytes()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digit_end == 0 || rest.as_bytes().get(after_p + digit_end) != Some(&b'}') {
            // Not a real placeholder — emit `{p` literally and keep going.
            out.push_str("{p");
            rest = &rest[after_p..];
            continue;
        }
        let n: usize = rest[after_p..after_p + digit_end]
            .parse()
            .expect("digit_end gated by is_ascii_digit");
        out.push_str(&format!("{{p{}}}", n + offset));
        rest = &rest[after_p + digit_end + 1..]; // +1 for the closing `}`
    }
    out.push_str(rest);
    out
}

/// Indent every line of `sql` by 4 spaces. Used for CTE bodies.
fn indent_lines(sql: &str) -> String {
    sql.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use stitchd_core::id::MetricId;
    use stitchd_core::metric::{AggregationOperator, FunnelStep};

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";

    // ── Aggregation ──────────────────────────────────────────────────────────

    #[test]
    fn aggregation_count_no_where_clause() {
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q = build_preview_aggregation_query(&cfg, ENV_ID, 14).unwrap();
        // count() wrapped in CAST so the wire shape is Nullable(Float64)
        // — uniform with funnel/ratio outputs that nullIf can produce.
        assert!(
            q.sql
                .contains("CAST(count() AS Nullable(Float64)) AS value"),
            "got: {}",
            q.sql
        );
        assert!(q.sql.contains("toStartOfDay(e.timestamp, 'UTC')"));
        assert!(q.sql.contains("toIntervalDay({p2})"));
        // 3 binds: env_id, event_key, days
        assert_eq!(q.binds.len(), 3);
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str("checkout_completed".into()));
        assert_eq!(q.binds[2], QueryBind::I64(14));
    }

    #[test]
    fn aggregation_sum_on_value_field_uses_coalesce_without_to_float() {
        // on_field=="value" must skip toFloat64OrNull — the coalesce of
        // the native numeric columns already returns Nullable(Float64),
        // and `toFloat64OrNull(Float64)` is rejected by ClickHouse at
        // runtime (ILLEGAL_TYPE_OF_ARGUMENT). Property-based on_field
        // takes the toFloat64OrNull path (covered by
        // `aggregation_p90_on_property_field` below).
        let cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: Some("value".into()),
            where_clause: None,
        };
        let q = build_preview_aggregation_query(&cfg, ENV_ID, 30).unwrap();
        assert!(
            q.sql.contains("coalesce(e.value_double"),
            "sum(value) should coalesce, got: {}",
            q.sql
        );
        assert!(
            !q.sql.contains("toFloat64OrNull"),
            "sum(value) must NOT wrap coalesce in toFloat64OrNull, got: {}",
            q.sql
        );
    }

    #[test]
    fn aggregation_p90_on_property_field() {
        let cfg = AggregationConfig {
            event_key: "page_load".into(),
            aggregator: AggregationOperator::P90,
            on_field: Some("latency_ms".into()),
            where_clause: None,
        };
        let q = build_preview_aggregation_query(&cfg, ENV_ID, 7).unwrap();
        assert!(q.sql.contains("quantile(0.9)"));
        assert!(q.sql.contains("properties['latency_ms']"));
    }

    #[test]
    fn aggregation_requires_field_for_sum() {
        // Defensive: sum without on_field rejected at build time.
        let cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: None,
            where_clause: None,
        };
        let err = build_preview_aggregation_query(&cfg, ENV_ID, 14).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn aggregation_where_clause_applies_as_extra_predicate() {
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(json!({"==": [{"var": "currency"}, "USD"]})),
        };
        let q = build_preview_aggregation_query(&cfg, ENV_ID, 14).unwrap();
        assert!(q.sql.contains("properties['currency']"), "got: {}", q.sql);
        // 4 binds: env_id, event_key, days, "USD"
        assert_eq!(q.binds.len(), 4);
        assert_eq!(q.binds[3], QueryBind::Str("USD".into()));
    }

    // ── Ratio ────────────────────────────────────────────────────────────────

    #[test]
    fn ratio_joins_three_ctes_on_day_ts() {
        let num_cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let den_cfg = AggregationConfig {
            event_key: "checkout_started".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let ratio_cfg = RatioConfig {
            numerator_metric_id: MetricId::new(),
            denominator_metric_id: MetricId::new(),
            min_denominator: 30,
        };
        let q = build_preview_ratio_query(&ratio_cfg, &num_cfg, &den_cfg, ENV_ID, 14).unwrap();
        assert!(q.sql.contains("WITH numerator AS"));
        assert!(q.sql.contains("denominator AS"));
        assert!(q.sql.contains("denominator_count AS"));
        assert!(q.sql.contains("ON numerator.day_ts = denominator.day_ts"));
        assert!(
            q.sql.contains("denominator_count.value >= {p"),
            "min_denominator guard missing in: {}",
            q.sql
        );
        // 10 binds: 3 per leg (env, key, days) × 3 legs + min_denominator
        assert_eq!(q.binds.len(), 10);
        assert_eq!(q.binds.last(), Some(&QueryBind::I64(30)));
    }

    #[test]
    fn ratio_guards_division_by_zero_via_nullif() {
        let num_cfg = AggregationConfig {
            event_key: "a".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let den_cfg = AggregationConfig {
            event_key: "b".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let ratio_cfg = RatioConfig {
            numerator_metric_id: MetricId::new(),
            denominator_metric_id: MetricId::new(),
            min_denominator: 0,
        };
        let q = build_preview_ratio_query(&ratio_cfg, &num_cfg, &den_cfg, ENV_ID, 14).unwrap();
        assert!(q.sql.contains("nullIf(denominator.value, 0)"));
    }

    // ── Funnel ───────────────────────────────────────────────────────────────

    #[test]
    fn funnel_strict_order_emits_window_funnel_with_mode() {
        let cfg = FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "signup_started".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "signup_completed".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 3600,
            count_repeats: false,
        };
        let q = build_preview_funnel_query(&cfg, ENV_ID, 14).unwrap();
        assert!(
            q.sql.contains("windowFunnel(3600, 'strict_order')"),
            "expected strict_order mode in: {}",
            q.sql
        );
        // Final-step level == 2 for a 2-step funnel.
        assert!(q.sql.contains("countIf(level >= 2)"));
        // Divide-by-zero guard.
        assert!(q.sql.contains("nullIf(countIf(level >= 1), 0)"));
    }

    #[test]
    fn funnel_count_repeats_omits_strict_order() {
        let cfg = FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "a".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "b".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "c".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 86400,
            count_repeats: true,
        };
        let q = build_preview_funnel_query(&cfg, ENV_ID, 30).unwrap();
        assert!(q.sql.contains("windowFunnel(86400)("));
        assert!(!q.sql.contains("strict_order"));
        assert!(q.sql.contains("countIf(level >= 3)"));
    }

    #[test]
    fn funnel_rejects_fewer_than_two_steps() {
        let cfg = FunnelConfig {
            steps: vec![FunnelStep {
                event_key: "only".into(),
                where_clause: None,
            }],
            window_seconds: 3600,
            count_repeats: false,
        };
        let err = build_preview_funnel_query(&cfg, ENV_ID, 14).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn funnel_rejects_non_positive_window() {
        let cfg = FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "a".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "b".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 0,
            count_repeats: false,
        };
        let err = build_preview_funnel_query(&cfg, ENV_ID, 14).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    // ── Experiment-scoped aggregation preview ────────────────────────────────

    fn iter_end() -> chrono::DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 5, 21, 0, 0, 0).unwrap()
    }

    const EXP_ID: &str = "00000000-0000-0000-0000-000000000002";
    const ITER_ID: &str = "00000000-0000-0000-0000-000000000003";

    #[test]
    fn experiment_preview_aggregation_joins_assignments_and_groups_by_day_ctx_variant() {
        let cfg = AggregationConfig {
            event_key: "click".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q =
            build_experiment_preview_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, iter_end())
                .unwrap();
        assert!(
            q.sql.contains("INNER JOIN experiment_assignments AS a"),
            "JOIN must be present, got:\n{}",
            q.sql
        );
        assert!(
            q.sql.contains(
                "AND arrayExists(t -> t.1 = a.context_type AND t.2 = a.context_key, e.contexts)"
            ),
            "join predicate via arrayExists missing, got:\n{}",
            q.sql
        );
        assert!(
            q.sql.contains("a.context_type AS context_type"),
            "must surface context_type"
        );
        assert!(
            q.sql.contains("a.variant_key AS variant_key"),
            "must surface variant_key"
        );
        assert!(
            q.sql
                .contains("GROUP BY day_ts, a.context_type, a.variant_key"),
            "must group per-day per-context-type per-variant, got:\n{}",
            q.sql
        );
        assert!(
            q.sql.contains("toStartOfDay(e.occurred_at, 'UTC')"),
            "must bucket by occurred_at day, got:\n{}",
            q.sql
        );
        // Binds in SQL-appearance order: env, exp, iter, event, iter_end.
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str(EXP_ID.into()));
        assert_eq!(q.binds[2], QueryBind::Str(ITER_ID.into()));
        assert_eq!(q.binds[3], QueryBind::Str("click".into()));
        assert_eq!(q.binds[4], QueryBind::I64(iter_end().timestamp_millis()));
        assert_eq!(q.binds.len(), 5);
    }

    #[test]
    fn experiment_preview_aggregation_excludes_pre_exposure_and_post_iteration() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q =
            build_experiment_preview_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, iter_end())
                .unwrap();
        assert!(q.sql.contains("e.occurred_at >= a.assigned_at"));
        assert!(
            q.sql
                .contains("e.occurred_at < fromUnixTimestamp64Milli({p4})")
        );
    }

    #[test]
    fn experiment_preview_aggregation_no_event_side_experiment_tags() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q =
            build_experiment_preview_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, iter_end())
                .unwrap();
        assert!(!q.sql.contains("arrayExists(t -> t.1 = 'experiment'"));
        assert!(!q.sql.contains("t.1 = 'iteration'"));
        assert!(!q.sql.contains("t.1 = 'variant'"));
    }

    #[test]
    fn experiment_preview_aggregation_with_where_clause_appends_literal_bind() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(serde_json::json!({"==": [{"var": "country"}, "US"]})),
        };
        let q =
            build_experiment_preview_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, iter_end())
                .unwrap();
        assert_eq!(q.binds.last(), Some(&QueryBind::Str("US".into())));
        assert!(q.sql.contains("AND (properties['country'] = {p5})"));
    }

    #[test]
    fn experiment_preview_aggregation_sum_requires_on_field() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Sum,
            on_field: None,
            where_clause: None,
        };
        let err =
            build_experiment_preview_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, iter_end())
                .unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    // ── Placeholder rewriting ────────────────────────────────────────────────

    #[test]
    fn shift_placeholders_zero_offset_is_noop() {
        let s = "SELECT {p0}, {p1}, {p2}";
        assert_eq!(shift_placeholders(s, 0), s);
    }

    #[test]
    fn shift_placeholders_offsets_multi_digit() {
        let s = "{p0} {p9} {p10}";
        assert_eq!(shift_placeholders(s, 3), "{p3} {p12} {p13}");
    }
}
