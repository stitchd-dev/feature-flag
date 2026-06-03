//! Per-variant-tuple **metric** cell aggregation for N-way cross-experiment
//! interaction analysis.
//!
//! [`super::interaction`] produces the bare contingency table of *shared
//! contexts* (how many contexts landed in each variant-tuple cell). That tells
//! us the population overlap but says nothing about the *metric*. This module
//! goes one step further: for a single shared metric it aggregates the metric
//! outcome per cell so the interaction decomposition math
//! ([`stitchd_core::experimentation::stats::interaction`]) can run.
//!
//! ## k-way generalization
//!
//! Every builder takes `exp_ids: &[&str]` (length 2 for a pairwise interaction,
//! 3 for a three-way). The assignment self-join chains `k` aliases
//! `a0, a1, …, a{k-1}` of `experiment_assignments FINAL`, each INNER-joined on
//! the shared context identity `(env_id, context_type, context_key)`. The cell
//! is keyed by a CH `array(a0.variant_key, a1.variant_key, …)` value in tuple
//! order — the reader maps it straight to a `Vec<String>`. The strict
//! first-exposure (ITT) lower bound is `greatest(a0.assigned_at, …, a{k-1})` —
//! the instant the context was simultaneously exposed to *all* `k` experiments.
//!
//! ## Metric families
//!
//! Three builders cover the supported metric kinds; all share the same k-way
//! `shared` CTE shape and emit a `variant_keys` array plus the family's
//! sufficient statistics:
//!
//! - **Aggregation** ([`build_interaction_metric_cells_query`]) — conversion
//!   (`count`/`uniq` → binary) and continuous (`sum`/`avg`/percentile →
//!   numeric) both emit `variant_keys, n, successes, value_sum, value_sq_sum`
//!   (num/den columns zero). The caller reads the columns relevant to
//!   [`MetricCellKind`].
//! - **Funnel** ([`build_interaction_funnel_cells_query`]) — per shared context
//!   `windowFunnel(window_seconds)(occurred_at, step1, …, stepN) == N` ⇒
//!   success; emits `variant_keys, n, successes` (binary path).
//! - **Ratio** ([`build_interaction_ratio_cells_query`]) — per shared context
//!   the numerator value and denominator value (two `Aggregation` legs of the
//!   `RatioConfig`), reduced to `Σnum, Σden, Σnum², Σden², Σ(num·den), n` per
//!   cell for the delta-method contrast variance.
//!
//! All SQL is parameterized via [`push_bind`] / [`QueryBind`] / [`BuiltQuery`].
//! `clickhouse-rs` binds `?` by SQL position, so binds are pushed left-to-right
//! in appearance order; the per-context ITT lower bound is a column reference
//! carried out of `shared`, not a bind.

use chrono::{DateTime, Utc};
use stitchd_core::metric::{AggregationConfig, AggregationOperator, FunnelConfig};

use super::{BuiltQuery, QueryBind, QueryBuildError, push_bind};

/// Whether a metric is treated as a **conversion** (binary) or **continuous**
/// (numeric) outcome for interaction analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricCellKind {
    /// Binary success/failure per context — built from a count/uniq
    /// aggregator. Yields `(n, successes)` cells.
    Conversion,
    /// Numeric per-context value — built from sum/avg/percentile aggregators.
    /// Yields `(n, sum, sum_sq)` cells.
    Continuous,
}

impl MetricCellKind {
    /// Classify an [`AggregationConfig`] by its aggregator.
    ///
    /// `Count` / `Uniq` are occurrence counts → [`Self::Conversion`]; the
    /// numeric aggregators (`Sum`, `Avg`, `P50`, `P90`, `P99`) operate on a
    /// per-event value → [`Self::Continuous`].
    #[must_use]
    pub const fn classify(cfg: &AggregationConfig) -> Self {
        match cfg.aggregator {
            AggregationOperator::Count | AggregationOperator::Uniq => Self::Conversion,
            AggregationOperator::Sum
            | AggregationOperator::Avg
            | AggregationOperator::P50
            | AggregationOperator::P90
            | AggregationOperator::P99 => Self::Continuous,
        }
    }
}

/// Validate the k-way experiment list: at least 2, at most 3 (the supported
/// interaction orders), and all distinct.
fn validate_exp_ids(exp_ids: &[&str]) -> Result<(), QueryBuildError> {
    if !(2..=3).contains(&exp_ids.len()) {
        return Err(QueryBuildError::InvalidConfig(format!(
            "interaction order must be 2 or 3 (got {} experiments)",
            exp_ids.len()
        )));
    }
    for i in 0..exp_ids.len() {
        for j in (i + 1)..exp_ids.len() {
            if exp_ids[i] == exp_ids[j] {
                return Err(QueryBuildError::InvalidConfig(
                    "interaction experiments must be distinct".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Emit the k-way `shared` CTE body (between `WITH shared AS (` and the closing
/// `)`), pushing binds in SQL-appearance order. Aliases are `a0, a1, …`. The
/// returned SQL carries one `variant_keys` array column (in tuple order) and a
/// `joint_assigned_at = greatest(…)` ITT lower bound.
///
/// Bind order per experiment `i`: `env_id` (a0 only), `experiment_id`,
/// `context_type` (a0 only), `assigned_at` upper bound. env/context are pinned
/// to a0 by the JOIN `ON`, so only a0 binds them.
fn emit_shared_cte(
    binds: &mut Vec<QueryBind>,
    exp_ids: &[&str],
    env_id: &str,
    context_type: &str,
    interaction_end: DateTime<Utc>,
) -> String {
    let end_ms = interaction_end.timestamp_millis();
    let k = exp_ids.len();

    // SELECT list: the variant-key array (tuple order) + the joint ITT bound.
    let variant_array = (0..k)
        .map(|i| format!("a{i}.variant_key"))
        .collect::<Vec<_>>()
        .join(", ");
    let assigned_list = (0..k)
        .map(|i| format!("a{i}.assigned_at"))
        .collect::<Vec<_>>()
        .join(", ");

    // FROM + chained INNER JOINs on the shared context identity.
    let mut from_clause = String::from("FROM experiment_assignments AS a0 FINAL\n");
    for i in 1..k {
        from_clause.push_str(&format!(
            "    INNER JOIN experiment_assignments AS a{i} FINAL\n        \
                ON a0.env_id = a{i}.env_id\n       \
                AND a0.context_type = a{i}.context_type\n       \
                AND a0.context_key = a{i}.context_key\n"
        ));
    }

    // WHERE: env (a0), then per-alias experiment + window bounds; context_type
    // bound once on a0.
    let env_ph = push_bind(binds, QueryBind::Str(env_id.to_owned()));
    let mut where_parts: Vec<String> = vec![format!("a0.env_id = toUUID({env_ph})")];
    for (i, exp) in exp_ids.iter().enumerate() {
        let exp_ph = push_bind(binds, QueryBind::Str((*exp).to_owned()));
        where_parts.push(format!("a{i}.experiment_id = toUUID({exp_ph})"));
        if i == 0 {
            let ctx_ph = push_bind(binds, QueryBind::Str(context_type.to_owned()));
            where_parts.push(format!("a0.context_type = {ctx_ph}"));
        }
        let end_ph = push_bind(binds, QueryBind::I64(end_ms));
        where_parts.push(format!(
            "a{i}.assigned_at < fromUnixTimestamp64Milli({end_ph})"
        ));
    }
    for i in 0..k {
        where_parts.push(format!("a{i}.variant_key != ''"));
    }
    let where_clause = where_parts.join("\n      AND ");

    format!(
        "    SELECT\n        \
            a0.context_key AS context_key,\n        \
            array({variant_array}) AS variant_keys,\n        \
            greatest({assigned_list}) AS joint_assigned_at\n    \
        {from_clause}    \
        WHERE {where_clause}\n"
    )
}

/// Build the per-variant-tuple metric-cell aggregation query for the
/// k-way interaction among `exp_ids` within `env_id`, restricted to one
/// `context_type` and one shared `Aggregation` metric.
///
/// The result rows carry `(variant_keys: Array(String), n, successes,
/// value_sum, value_sq_sum)`. The caller reads only the columns relevant to the
/// metric kind:
///
/// - [`MetricCellKind::Conversion`] → `n`, `successes` (→ binary cells),
/// - [`MetricCellKind::Continuous`] → `n`, `value_sum`, `value_sq_sum`
///   (→ continuous cells).
///
/// `interaction_end` upper-bounds both the assignment overlap window and the
/// event window. `variant_keys` is the per-experiment variant tuple in the same
/// order as `exp_ids`.
///
/// # Errors
/// Returns [`QueryBuildError::InvalidConfig`] when `exp_ids` is not 2 or 3
/// distinct experiments.
pub fn build_interaction_metric_cells_query(
    cfg: &AggregationConfig,
    env_id: &str,
    exp_ids: &[&str],
    context_type: &str,
    interaction_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    validate_exp_ids(exp_ids)?;

    let end_ms = interaction_end.timestamp_millis();
    let mut binds = Vec::new();

    // ── shared CTE: k-way assignment self-join ──────────────────────────────
    let shared_cte = emit_shared_cte(&mut binds, exp_ids, env_id, context_type, interaction_end);

    // ── matched_events CTE: per-context metric outcome over events ──────────
    let ev_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let event_ph = push_bind(&mut binds, QueryBind::Str(cfg.event_key.clone()));
    let ev_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let ev_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));

    // The numeric value expression mirrors super::aggregation's canonical
    // column routing.
    let value_expr = super::numeric_value_expr(cfg);

    let sql = format!(
        "WITH\n\
        shared AS (\n\
        {shared_cte}\
        ),\n\
        matched_events AS (\n    \
            SELECT\n        \
                ctx_pair.2 AS context_key,\n        \
                {value_expr} AS value,\n        \
                e.occurred_at AS occurred_at\n    \
            FROM events AS e\n    \
            ARRAY JOIN e.contexts AS ctx_pair\n    \
            WHERE e.env_id = toUUID({ev_env_ph})\n      \
              AND e.metric_key = {event_ph}\n      \
              AND ctx_pair.1 = {ev_ctx_ph}\n      \
              AND e.occurred_at < fromUnixTimestamp64Milli({ev_end_ph})\n\
        ),\n\
        ctx_events AS (\n    \
            SELECT\n        \
                s.variant_keys AS variant_keys,\n        \
                countIf(me.occurred_at >= s.joint_assigned_at) AS event_count,\n        \
                sumIf(me.value, me.occurred_at >= s.joint_assigned_at) AS value_sum,\n        \
                sumIf(me.value * me.value, me.occurred_at >= s.joint_assigned_at) AS value_sq_sum\n    \
            FROM shared AS s\n    \
            LEFT JOIN matched_events AS me\n        \
                ON me.context_key = s.context_key\n    \
            GROUP BY s.context_key, s.variant_keys\n\
        )\n\
        SELECT\n    \
            ce.variant_keys AS variant_keys,\n    \
            count() AS n,\n    \
            countIf(ce.event_count > 0) AS successes,\n    \
            sum(ce.value_sum) AS value_sum,\n    \
            sum(ce.value_sq_sum) AS value_sq_sum\n\
        FROM ctx_events AS ce\n\
        GROUP BY ce.variant_keys"
    );

    Ok(BuiltQuery { sql, binds })
}

/// Build the per-variant-tuple **funnel** cell query for the k-way interaction
/// among `exp_ids`.
///
/// Per shared context, `windowFunnel(window_seconds[, 'strict_order'])` over the
/// step events evaluates to the deepest step reached; a context is a *success*
/// iff it reached the final step (`level == steps.len()`). Emits
/// `(variant_keys, n, successes)` — the binary path.
///
/// The funnel's step events are bounded below by the per-context joint exposure
/// (`occurred_at >= joint_assigned_at`) and above by `interaction_end`, matching
/// the ITT discipline of the aggregation path.
///
/// # Errors
/// Returns [`QueryBuildError::InvalidConfig`] when `exp_ids` is not 2 or 3
/// distinct experiments, `steps.len() < 2`, or `window_seconds <= 0`.
pub fn build_interaction_funnel_cells_query(
    cfg: &FunnelConfig,
    env_id: &str,
    exp_ids: &[&str],
    context_type: &str,
    interaction_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    validate_exp_ids(exp_ids)?;
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

    let end_ms = interaction_end.timestamp_millis();
    let mut binds = Vec::new();

    // ── shared CTE: k-way assignment self-join (binds pushed first) ─────────
    let shared_cte = emit_shared_cte(&mut binds, exp_ids, env_id, context_type, interaction_end);

    // ── step predicates for windowFunnel (in the SELECT list of ctx_funnel) ─
    let step_predicate_phs: Vec<String> = cfg
        .steps
        .iter()
        .map(|step| push_bind(&mut binds, QueryBind::Str(step.event_key.clone())))
        .collect();
    let step_predicates = step_predicate_phs
        .iter()
        .map(|ph| format!("me.metric_key = {ph}"))
        .collect::<Vec<_>>()
        .join(",\n                ");

    // ── matched_events bounds (env + context + metric_key OR-filter + end) ──
    let ev_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let ev_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let metric_key_filter_phs: Vec<String> = cfg
        .steps
        .iter()
        .map(|step| push_bind(&mut binds, QueryBind::Str(step.event_key.clone())))
        .collect();
    let metric_key_filter = metric_key_filter_phs
        .iter()
        .map(|ph| format!("me.metric_key = {ph}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let ev_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));

    let mode_arg = if cfg.count_repeats {
        String::new()
    } else {
        ", 'strict_order'".to_owned()
    };
    let window = cfg.window_seconds;
    let final_level = cfg.steps.len() as i64;

    // ctx_funnel computes per shared context the windowFunnel level, with the
    // step-event stream flattened from events. The per-context ITT lower bound
    // (joint_assigned_at) gates the events via a predicate on occurred_at;
    // matched_events is keyed by context_key.
    let sql = format!(
        "WITH\n\
        shared AS (\n\
        {shared_cte}\
        ),\n\
        matched_events AS (\n    \
            SELECT\n        \
                ctx_pair.2 AS context_key,\n        \
                e.metric_key AS metric_key,\n        \
                e.occurred_at AS occurred_at\n    \
            FROM events AS e\n    \
            ARRAY JOIN e.contexts AS ctx_pair\n    \
            WHERE e.env_id = toUUID({ev_env_ph})\n      \
              AND ctx_pair.1 = {ev_ctx_ph}\n      \
              AND ({metric_key_filter})\n      \
              AND e.occurred_at < fromUnixTimestamp64Milli({ev_end_ph})\n\
        ),\n\
        ctx_funnel AS (\n    \
            SELECT\n        \
                s.variant_keys AS variant_keys,\n        \
                windowFunnel({window}{mode_arg})(\n            \
                    toUInt32(toUnixTimestamp(me.occurred_at)),\n            \
                    {step_predicates}\n        \
                ) AS level\n    \
            FROM shared AS s\n    \
            LEFT JOIN matched_events AS me\n        \
                ON me.context_key = s.context_key\n       \
                AND me.occurred_at >= s.joint_assigned_at\n    \
            GROUP BY s.context_key, s.variant_keys\n\
        )\n\
        SELECT\n    \
            cf.variant_keys AS variant_keys,\n    \
            count() AS n,\n    \
            countIf(cf.level >= {final_level}) AS successes\n\
        FROM ctx_funnel AS cf\n\
        GROUP BY cf.variant_keys"
    );

    Ok(BuiltQuery { sql, binds })
}

/// Build the per-variant-tuple **ratio** cell query for the k-way interaction
/// among `exp_ids`.
///
/// Per shared context the numerator value and denominator value are summed from
/// the two `Aggregation` legs (`num_cfg` / `den_cfg` resolved from the
/// `RatioConfig`), then reduced per cell to the delta-method sufficient
/// statistics `Σnum, Σden, Σnum², Σden², Σ(num·den), n`. Both legs are bounded
/// below by the per-context joint exposure (ITT) and above by `interaction_end`.
///
/// # Errors
/// Returns [`QueryBuildError::InvalidConfig`] when `exp_ids` is not 2 or 3
/// distinct experiments.
pub fn build_interaction_ratio_cells_query(
    num_cfg: &AggregationConfig,
    den_cfg: &AggregationConfig,
    env_id: &str,
    exp_ids: &[&str],
    context_type: &str,
    interaction_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    validate_exp_ids(exp_ids)?;

    let end_ms = interaction_end.timestamp_millis();
    let mut binds = Vec::new();

    // ── shared CTE: k-way assignment self-join ──────────────────────────────
    let shared_cte = emit_shared_cte(&mut binds, exp_ids, env_id, context_type, interaction_end);

    // ── numerator events ────────────────────────────────────────────────────
    let num_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let num_event_ph = push_bind(&mut binds, QueryBind::Str(num_cfg.event_key.clone()));
    let num_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let num_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));
    let num_value_expr = super::numeric_value_expr(num_cfg);

    // ── denominator events ──────────────────────────────────────────────────
    let den_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let den_event_ph = push_bind(&mut binds, QueryBind::Str(den_cfg.event_key.clone()));
    let den_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let den_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));
    let den_value_expr = super::numeric_value_expr(den_cfg);

    // Per shared context: sum the numerator and denominator metric over
    // post-exposure events, then per cell accumulate the five second-moment
    // terms the delta method needs. `num`/`den` are per-context point values;
    // the outer SELECT squares/cross-multiplies them and sums across contexts.
    let sql = format!(
        "WITH\n\
        shared AS (\n\
        {shared_cte}\
        ),\n\
        num_events AS (\n    \
            SELECT\n        \
                ctx_pair.2 AS context_key,\n        \
                {num_value_expr} AS value,\n        \
                e.occurred_at AS occurred_at\n    \
            FROM events AS e\n    \
            ARRAY JOIN e.contexts AS ctx_pair\n    \
            WHERE e.env_id = toUUID({num_env_ph})\n      \
              AND e.metric_key = {num_event_ph}\n      \
              AND ctx_pair.1 = {num_ctx_ph}\n      \
              AND e.occurred_at < fromUnixTimestamp64Milli({num_end_ph})\n\
        ),\n\
        den_events AS (\n    \
            SELECT\n        \
                ctx_pair.2 AS context_key,\n        \
                {den_value_expr} AS value,\n        \
                e.occurred_at AS occurred_at\n    \
            FROM events AS e\n    \
            ARRAY JOIN e.contexts AS ctx_pair\n    \
            WHERE e.env_id = toUUID({den_env_ph})\n      \
              AND e.metric_key = {den_event_ph}\n      \
              AND ctx_pair.1 = {den_ctx_ph}\n      \
              AND e.occurred_at < fromUnixTimestamp64Milli({den_end_ph})\n\
        ),\n\
        ctx_ratio AS (\n    \
            SELECT\n        \
                s.variant_keys AS variant_keys,\n        \
                sumIf(ne.value, ne.occurred_at >= s.joint_assigned_at) AS num,\n        \
                sumIf(de.value, de.occurred_at >= s.joint_assigned_at) AS den\n    \
            FROM shared AS s\n    \
            LEFT JOIN num_events AS ne\n        \
                ON ne.context_key = s.context_key\n    \
            LEFT JOIN den_events AS de\n        \
                ON de.context_key = s.context_key\n    \
            GROUP BY s.context_key, s.variant_keys\n\
        )\n\
        SELECT\n    \
            cr.variant_keys AS variant_keys,\n    \
            count() AS n,\n    \
            sum(cr.num) AS num_sum,\n    \
            sum(cr.den) AS den_sum,\n    \
            sum(cr.num * cr.num) AS num_sq_sum,\n    \
            sum(cr.den * cr.den) AS den_sq_sum,\n    \
            sum(cr.num * cr.den) AS num_den_sum\n\
        FROM ctx_ratio AS cr\n\
        GROUP BY cr.variant_keys"
    );

    Ok(BuiltQuery { sql, binds })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use stitchd_core::metric::FunnelStep;

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";
    const EXP_A: &str = "00000000-0000-0000-0000-0000000000aa";
    const EXP_B: &str = "00000000-0000-0000-0000-0000000000bb";
    const EXP_C: &str = "00000000-0000-0000-0000-0000000000cc";

    fn end() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap()
    }

    fn count_cfg() -> AggregationConfig {
        AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        }
    }

    fn sum_cfg() -> AggregationConfig {
        AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: Some("revenue".into()),
            where_clause: None,
        }
    }

    fn funnel_cfg() -> FunnelConfig {
        FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "view".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "checkout".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 3600,
            count_repeats: false,
        }
    }

    // ── MetricCellKind::classify ─────────────────────────────────────────────

    #[test]
    fn classify_count_and_uniq_are_conversion() {
        let mut c = count_cfg();
        assert_eq!(MetricCellKind::classify(&c), MetricCellKind::Conversion);
        c.aggregator = AggregationOperator::Uniq;
        assert_eq!(MetricCellKind::classify(&c), MetricCellKind::Conversion);
    }

    #[test]
    fn classify_numeric_aggregators_are_continuous() {
        for op in [
            AggregationOperator::Sum,
            AggregationOperator::Avg,
            AggregationOperator::P50,
            AggregationOperator::P90,
            AggregationOperator::P99,
        ] {
            let cfg = AggregationConfig {
                event_key: "x".into(),
                aggregator: op,
                on_field: None,
                where_clause: None,
            };
            assert_eq!(
                MetricCellKind::classify(&cfg),
                MetricCellKind::Continuous,
                "{op:?} should be continuous"
            );
        }
    }

    // ── Aggregation k=2 ──────────────────────────────────────────────────────

    #[test]
    fn agg_k2_self_joins_two_assignments_final() {
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(
            q.sql.contains("FROM experiment_assignments AS a0 FINAL"),
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("INNER JOIN experiment_assignments AS a1 FINAL"),
            "{}",
            q.sql
        );
        // Exactly one INNER JOIN for a 2-way (a1 only).
        assert_eq!(
            q.sql.matches("INNER JOIN experiment_assignments").count(),
            1,
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("AND a0.context_key = a1.context_key"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn agg_k2_emits_variant_keys_array_in_tuple_order() {
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(
            q.sql.contains("array(a0.variant_key, a1.variant_key)"),
            "{}",
            q.sql
        );
        assert!(q.sql.contains("AS variant_keys"), "{}", q.sql);
        assert!(q.sql.contains("GROUP BY ce.variant_keys"), "{}", q.sql);
    }

    #[test]
    fn agg_k2_itt_lower_bound_is_greatest_over_both() {
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(
            q.sql
                .contains("greatest(a0.assigned_at, a1.assigned_at) AS joint_assigned_at"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("me.occurred_at >= s.joint_assigned_at"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn agg_k2_emits_conversion_and_continuous_columns() {
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(q.sql.contains("count() AS n"), "{}", q.sql);
        assert!(
            q.sql.contains("countIf(ce.event_count > 0) AS successes"),
            "{}",
            q.sql
        );
        assert!(q.sql.contains("AS value_sum"), "{}", q.sql);
        assert!(q.sql.contains("AS value_sq_sum"), "{}", q.sql);
    }

    #[test]
    fn agg_k2_array_join_not_arrayexists() {
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(
            q.sql.contains("ARRAY JOIN e.contexts AS ctx_pair"),
            "{}",
            q.sql
        );
        assert!(!q.sql.contains("arrayExists"), "{}", q.sql);
    }

    #[test]
    fn agg_k2_bind_order_is_left_to_right() {
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        // shared CTE: env, expA, ctx, endA, expB, endB
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str(EXP_A.into()));
        assert_eq!(q.binds[2], QueryBind::Str("user".into()));
        assert_eq!(q.binds[3], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds[4], QueryBind::Str(EXP_B.into()));
        assert_eq!(q.binds[5], QueryBind::I64(end().timestamp_millis()));
        // events: env, event_key, ctx, end
        assert_eq!(q.binds[6], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[7], QueryBind::Str("checkout_completed".into()));
        assert_eq!(q.binds[8], QueryBind::Str("user".into()));
        assert_eq!(q.binds[9], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds.len(), 10);
    }

    #[test]
    fn agg_continuous_uses_property_value_expr() {
        let q = build_interaction_metric_cells_query(
            &sum_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(
            q.sql.contains("toFloat64OrNull(e.properties['revenue'])"),
            "{}",
            q.sql
        );
    }

    // ── Aggregation k=3 ──────────────────────────────────────────────────────

    #[test]
    fn agg_k3_chains_three_aliases_with_two_joins() {
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B, EXP_C],
            "user",
            end(),
        )
        .unwrap();
        assert!(
            q.sql.contains("FROM experiment_assignments AS a0 FINAL"),
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("INNER JOIN experiment_assignments AS a1 FINAL"),
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("INNER JOIN experiment_assignments AS a2 FINAL"),
            "{}",
            q.sql
        );
        // 3-way ⇒ two INNER JOINs (a1, a2).
        assert_eq!(
            q.sql.matches("INNER JOIN experiment_assignments").count(),
            2,
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("AND a0.context_key = a2.context_key"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn agg_k3_variant_array_and_greatest_span_three() {
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B, EXP_C],
            "user",
            end(),
        )
        .unwrap();
        assert!(
            q.sql
                .contains("array(a0.variant_key, a1.variant_key, a2.variant_key)"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains(
                "greatest(a0.assigned_at, a1.assigned_at, a2.assigned_at) AS joint_assigned_at"
            ),
            "{}",
            q.sql
        );
    }

    #[test]
    fn agg_k3_bind_order_covers_all_three_experiments() {
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B, EXP_C],
            "user",
            end(),
        )
        .unwrap();
        // shared CTE: env, expA, ctx, endA, expB, endB, expC, endC
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str(EXP_A.into()));
        assert_eq!(q.binds[2], QueryBind::Str("user".into()));
        assert_eq!(q.binds[3], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds[4], QueryBind::Str(EXP_B.into()));
        assert_eq!(q.binds[5], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds[6], QueryBind::Str(EXP_C.into()));
        assert_eq!(q.binds[7], QueryBind::I64(end().timestamp_millis()));
        // events: env, event_key, ctx, end
        assert_eq!(q.binds[8], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[9], QueryBind::Str("checkout_completed".into()));
        assert_eq!(q.binds[10], QueryBind::Str("user".into()));
        assert_eq!(q.binds[11], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds.len(), 12);
    }

    // ── Funnel ───────────────────────────────────────────────────────────────

    #[test]
    fn funnel_k2_emits_window_funnel_and_binary_columns() {
        let q = build_interaction_funnel_cells_query(
            &funnel_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(
            q.sql.contains("windowFunnel(3600, 'strict_order')"),
            "{}",
            q.sql
        );
        // success ⇒ reached final step (level >= 2 for a 2-step funnel).
        assert!(
            q.sql.contains("countIf(cf.level >= 2) AS successes"),
            "{}",
            q.sql
        );
        assert!(q.sql.contains("count() AS n"), "{}", q.sql);
        assert!(q.sql.contains("AS variant_keys"), "{}", q.sql);
    }

    #[test]
    fn funnel_count_repeats_omits_strict_order() {
        let mut cfg = funnel_cfg();
        cfg.count_repeats = true;
        let q = build_interaction_funnel_cells_query(&cfg, ENV_ID, &[EXP_A, EXP_B], "user", end())
            .unwrap();
        assert!(q.sql.contains("windowFunnel(3600)("), "{}", q.sql);
        assert!(!q.sql.contains("strict_order"), "{}", q.sql);
    }

    #[test]
    fn funnel_step_predicates_and_metric_filter_bound() {
        let cfg = FunnelConfig {
            steps: vec![
                FunnelStep {
                    event_key: "view".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "cart".into(),
                    where_clause: None,
                },
                FunnelStep {
                    event_key: "buy".into(),
                    where_clause: None,
                },
            ],
            window_seconds: 7200,
            count_repeats: false,
        };
        let q = build_interaction_funnel_cells_query(&cfg, ENV_ID, &[EXP_A, EXP_B], "user", end())
            .unwrap();
        // 3-step funnel ⇒ success at level >= 3.
        assert!(
            q.sql.contains("countIf(cf.level >= 3) AS successes"),
            "{}",
            q.sql
        );
        // The three step event keys are bound twice (predicate + filter).
        let view_binds = q
            .binds
            .iter()
            .filter(|b| **b == QueryBind::Str("view".into()))
            .count();
        assert_eq!(view_binds, 2, "step predicate + metric_key filter");
        // metric_key OR-filter present.
        assert!(q.sql.contains(" OR "), "{}", q.sql);
    }

    #[test]
    fn funnel_k3_chains_three_aliases() {
        let q = build_interaction_funnel_cells_query(
            &funnel_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B, EXP_C],
            "user",
            end(),
        )
        .unwrap();
        assert_eq!(
            q.sql.matches("INNER JOIN experiment_assignments").count(),
            2,
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("array(a0.variant_key, a1.variant_key, a2.variant_key)"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn funnel_rejects_one_step() {
        let cfg = FunnelConfig {
            steps: vec![FunnelStep {
                event_key: "only".into(),
                where_clause: None,
            }],
            window_seconds: 3600,
            count_repeats: false,
        };
        let err =
            build_interaction_funnel_cells_query(&cfg, ENV_ID, &[EXP_A, EXP_B], "user", end())
                .unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    // ── Ratio ────────────────────────────────────────────────────────────────

    #[test]
    fn ratio_k2_emits_second_moment_columns() {
        let q = build_interaction_ratio_cells_query(
            &count_cfg(),
            &sum_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(q.sql.contains("sum(cr.num) AS num_sum"), "{}", q.sql);
        assert!(q.sql.contains("sum(cr.den) AS den_sum"), "{}", q.sql);
        assert!(
            q.sql.contains("sum(cr.num * cr.num) AS num_sq_sum"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("sum(cr.den * cr.den) AS den_sq_sum"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("sum(cr.num * cr.den) AS num_den_sum"),
            "{}",
            q.sql
        );
        assert!(q.sql.contains("count() AS n"), "{}", q.sql);
    }

    #[test]
    fn ratio_k2_joins_numerator_and_denominator_event_streams() {
        let q = build_interaction_ratio_cells_query(
            &count_cfg(),
            &sum_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(q.sql.contains("num_events AS ("), "{}", q.sql);
        assert!(q.sql.contains("den_events AS ("), "{}", q.sql);
        assert!(q.sql.contains("LEFT JOIN num_events AS ne"), "{}", q.sql);
        assert!(q.sql.contains("LEFT JOIN den_events AS de"), "{}", q.sql);
        // Numerator is a count → count(); denominator is sum over revenue prop.
        assert!(
            q.sql.contains("toFloat64OrNull(e.properties['revenue'])"),
            "{}",
            q.sql
        );
        // ITT applied per-context to both legs.
        assert!(
            q.sql
                .contains("sumIf(ne.value, ne.occurred_at >= s.joint_assigned_at) AS num"),
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("sumIf(de.value, de.occurred_at >= s.joint_assigned_at) AS den"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn ratio_k3_chains_three_aliases() {
        let q = build_interaction_ratio_cells_query(
            &count_cfg(),
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B, EXP_C],
            "user",
            end(),
        )
        .unwrap();
        assert_eq!(
            q.sql.matches("INNER JOIN experiment_assignments").count(),
            2,
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("array(a0.variant_key, a1.variant_key, a2.variant_key)"),
            "{}",
            q.sql
        );
    }

    // ── Validation ───────────────────────────────────────────────────────────

    #[test]
    fn rejects_self_interaction() {
        let err = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_A],
            "user",
            end(),
        )
        .unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn rejects_order_below_two() {
        let err =
            build_interaction_metric_cells_query(&count_cfg(), ENV_ID, &[EXP_A], "user", end())
                .unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn rejects_order_above_three() {
        let err = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B, EXP_C, "00000000-0000-0000-0000-0000000000dd"],
            "user",
            end(),
        )
        .unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn context_type_is_bound_not_inlined() {
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(!q.sql.contains("'user'"), "{}", q.sql);
    }
}
