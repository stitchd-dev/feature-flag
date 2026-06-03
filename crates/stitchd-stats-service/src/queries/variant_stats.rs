//! Per-`(context_type, variant_key)` **sufficient-statistics** query builders
//! for the live experiment-stats compute pass (Phase 3).
//!
//! The point-scalar [`super::aggregation`] / [`super::ratio`] / [`super::funnel`]
//! builders return only one aggregated value per variant — enough to render a
//! number, but NOT enough to run a frequentist / bayesian / sequential test
//! (those need per-variant `n`, `successes`, `Σx`, `Σx²`, and the ratio second
//! moments). This module specialises the **N-way interaction** sufficient-stats
//! SQL ([`super::interaction_metric`]) down to the single-experiment case: one
//! experiment, one iteration, GROUP BY `(context_type, variant_key)`.
//!
//! ## Intent-to-treat (ITT) discipline — identical to [`super::aggregation`]
//!
//! Every builder dedups assignments via `experiment_assignments FINAL`
//! (first-exposure `assigned_at`), INNER-JOINs the flattened `events.contexts`
//! against the assignment's `(env_id, context_type, context_key)`, and gates
//! event contribution on `e.occurred_at >= a.assigned_at` (post-exposure) and
//! `e.occurred_at < iteration_end`. A unit with NO events still counts toward
//! `n` (the ITT denominator) because the LEFT JOIN keeps it with zero metric
//! contribution.
//!
//! ## Families
//!
//! - **Aggregation** ([`build_aggregation_cells_query`]) — `n, successes,
//!   value_sum, value_sq_sum` per `(context_type, variant_key)`. Conversion
//!   (`count`/`uniq`) reads `successes`; continuous (`sum`/`avg`/percentile)
//!   reads `value_sum` / `value_sq_sum`.
//! - **Ratio** ([`build_ratio_cells_query`]) — `n, num_sum, den_sum,
//!   num_sq_sum, den_sq_sum, num_den_sum` per `(context_type, variant_key)` for
//!   the delta-method contrast variance.
//! - **Funnel** ([`build_funnel_cells_query`]) — `n, successes` (reached final
//!   step) per `(context_type, variant_key)`.
//! - **Assignment counts** ([`build_assignment_counts_query`]) — `n` per
//!   `(context_type, variant_key)`, the ITT sample-size denominator (used both
//!   as the per-metric `sample_size` and as the SRM observed counts).
//!
//! All SQL is parameterised via [`push_bind`] / [`QueryBind`] / [`BuiltQuery`];
//! binds are pushed in left-to-right SQL-appearance order (clickhouse-rs binds
//! `?` positionally).

use chrono::{DateTime, Utc};
use stitchd_core::metric::{AggregationConfig, FunnelConfig};

use super::{BuiltQuery, QueryBind, QueryBuildError, jsonlogic_to_sql, push_bind};

/// Emit the shared `assignments` CTE body: a deduped (`FINAL`) per-context
/// first-exposure projection scoped to one experiment + iteration + env,
/// restricted to `context_type` and `assigned_at < iteration_end`, dropping the
/// empty-variant sentinel. Carries `(context_type, context_key, variant_key,
/// assigned_at)` for the downstream event join + GROUP BY.
///
/// Binds pushed in order: `env_id`, `experiment_id`, `iteration_id`,
/// `context_type`, `iteration_end`.
fn emit_assignments_cte(
    binds: &mut Vec<QueryBind>,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    context_type: &str,
    iteration_end: DateTime<Utc>,
) -> String {
    let end_ms = iteration_end.timestamp_millis();
    let env_ph = push_bind(binds, QueryBind::Str(env_id.to_owned()));
    let exp_ph = push_bind(binds, QueryBind::Str(experiment_id.to_owned()));
    let iter_ph = push_bind(binds, QueryBind::Str(iteration_id.to_owned()));
    let ctx_ph = push_bind(binds, QueryBind::Str(context_type.to_owned()));
    let end_ph = push_bind(binds, QueryBind::I64(end_ms));
    format!(
        "    SELECT\n        \
            a.context_type AS context_type,\n        \
            a.context_key AS context_key,\n        \
            a.variant_key AS variant_key,\n        \
            a.assigned_at AS assigned_at\n    \
        FROM experiment_assignments AS a FINAL\n    \
        WHERE a.env_id = toUUID({env_ph})\n      \
          AND a.experiment_id = toUUID({exp_ph})\n      \
          AND a.iteration_id = toUUID({iter_ph})\n      \
          AND a.context_type = {ctx_ph}\n      \
          AND a.assigned_at < fromUnixTimestamp64Milli({end_ph})\n      \
          AND a.variant_key != ''\n"
    )
}

/// Build the per-`(context_type, variant_key)` **aggregation** sufficient-stats
/// query for one experiment + iteration restricted to a single `context_type`
/// and a single `Aggregation` metric.
///
/// Result rows carry `(context_type, variant_key, n, successes, value_sum,
/// value_sq_sum)`. Conversion metrics read `n` + `successes`; continuous metrics
/// read `n` + `value_sum` + `value_sq_sum`. `n` is the ITT denominator (units
/// with no qualifying event contribute `0`).
///
/// # Errors
/// Returns [`QueryBuildError`] when the metric `where_clause` JsonLogic cannot
/// be translated.
pub fn build_aggregation_cells_query(
    cfg: &AggregationConfig,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    context_type: &str,
    iteration_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    let end_ms = iteration_end.timestamp_millis();
    let mut binds = Vec::new();

    let assignments_cte = emit_assignments_cte(
        &mut binds,
        experiment_id,
        iteration_id,
        env_id,
        context_type,
        iteration_end,
    );

    let ev_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let event_ph = push_bind(&mut binds, QueryBind::Str(cfg.event_key.clone()));
    let ev_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let ev_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));
    let value_expr = super::numeric_value_expr(cfg);

    // Optional metric where_clause — applied to the events stream so a
    // property-filtered metric is computed over matching events only. Its
    // literals are the LAST binds (inside matched_events WHERE).
    let extra_where = match cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n      AND ({frag})")
        }
        None => String::new(),
    };

    let sql = format!(
        "WITH\n\
        assignments AS (\n\
        {assignments_cte}\
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
              AND e.occurred_at < fromUnixTimestamp64Milli({ev_end_ph}){extra_where}\n\
        ),\n\
        ctx_events AS (\n    \
            SELECT\n        \
                asg.context_type AS context_type,\n        \
                asg.variant_key AS variant_key,\n        \
                countIf(me.occurred_at >= asg.assigned_at) AS event_count,\n        \
                sumIf(me.value, me.occurred_at >= asg.assigned_at) AS value_sum,\n        \
                sumIf(me.value * me.value, me.occurred_at >= asg.assigned_at) AS value_sq_sum\n    \
            FROM assignments AS asg\n    \
            LEFT JOIN matched_events AS me\n        \
                ON me.context_key = asg.context_key\n    \
            GROUP BY asg.context_type, asg.context_key, asg.variant_key\n\
        )\n\
        SELECT\n    \
            ce.variant_key AS variant_key,\n    \
            count() AS n,\n    \
            countIf(ce.event_count > 0) AS successes,\n    \
            sum(ce.value_sum) AS value_sum,\n    \
            sum(ce.value_sq_sum) AS value_sq_sum\n\
        FROM ctx_events AS ce\n\
        GROUP BY ce.variant_key"
    );

    Ok(BuiltQuery { sql, binds })
}

/// Build the per-`(context_type, variant_key)` **ratio** sufficient-stats query
/// for one experiment + iteration restricted to a single `context_type`.
///
/// Per assigned context the numerator and denominator are summed from the two
/// `Aggregation` legs (`num_cfg` / `den_cfg`), then reduced per
/// `(context_type, variant_key)` to the delta-method sufficient statistics
/// `n, Σnum, Σden, Σnum², Σden², Σ(num·den)`. Both legs are ITT-bounded; a unit
/// with no events contributes `(num, den) = (0, 0)`.
///
/// # Errors
/// Returns [`QueryBuildError`] when either leg's `where_clause` cannot be
/// translated.
pub fn build_ratio_cells_query(
    num_cfg: &AggregationConfig,
    den_cfg: &AggregationConfig,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    context_type: &str,
    iteration_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    let end_ms = iteration_end.timestamp_millis();
    let mut binds = Vec::new();

    let assignments_cte = emit_assignments_cte(
        &mut binds,
        experiment_id,
        iteration_id,
        env_id,
        context_type,
        iteration_end,
    );

    // numerator events
    let num_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let num_event_ph = push_bind(&mut binds, QueryBind::Str(num_cfg.event_key.clone()));
    let num_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let num_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));
    let num_value_expr = super::numeric_value_expr(num_cfg);
    let num_extra_where = match num_cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n      AND ({frag})")
        }
        None => String::new(),
    };

    // denominator events
    let den_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let den_event_ph = push_bind(&mut binds, QueryBind::Str(den_cfg.event_key.clone()));
    let den_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let den_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));
    let den_value_expr = super::numeric_value_expr(den_cfg);
    let den_extra_where = match den_cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n      AND ({frag})")
        }
        None => String::new(),
    };

    // Pre-aggregate each leg to ONE per-context point value FIRST (num_ctx /
    // den_ctx), then INNER-JOIN 1:1 on context_key before accumulating the
    // second moments — mirrors interaction_metric's ratio path so a context
    // with M numerator and K denominator events never fans out to M×K rows.
    let sql = format!(
        "WITH\n\
        assignments AS (\n\
        {assignments_cte}\
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
              AND e.occurred_at < fromUnixTimestamp64Milli({num_end_ph}){num_extra_where}\n\
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
              AND e.occurred_at < fromUnixTimestamp64Milli({den_end_ph}){den_extra_where}\n\
        ),\n\
        num_ctx AS (\n    \
            SELECT\n        \
                asg.context_type AS context_type,\n        \
                asg.context_key AS context_key,\n        \
                any(asg.variant_key) AS variant_key,\n        \
                sumIf(ne.value, ne.occurred_at >= asg.assigned_at) AS num\n    \
            FROM assignments AS asg\n    \
            LEFT JOIN num_events AS ne\n        \
                ON ne.context_key = asg.context_key\n    \
            GROUP BY asg.context_type, asg.context_key\n\
        ),\n\
        den_ctx AS (\n    \
            SELECT\n        \
                asg.context_key AS context_key,\n        \
                sumIf(de.value, de.occurred_at >= asg.assigned_at) AS den\n    \
            FROM assignments AS asg\n    \
            LEFT JOIN den_events AS de\n        \
                ON de.context_key = asg.context_key\n    \
            GROUP BY asg.context_key\n\
        ),\n\
        ctx_ratio AS (\n    \
            SELECT\n        \
                num_ctx.context_type AS context_type,\n        \
                num_ctx.variant_key AS variant_key,\n        \
                num_ctx.num AS num,\n        \
                den_ctx.den AS den\n    \
            FROM num_ctx\n    \
            INNER JOIN den_ctx\n        \
                ON num_ctx.context_key = den_ctx.context_key\n\
        )\n\
        SELECT\n    \
            cr.variant_key AS variant_key,\n    \
            count() AS n,\n    \
            sum(cr.num) AS num_sum,\n    \
            sum(cr.den) AS den_sum,\n    \
            sum(cr.num * cr.num) AS num_sq_sum,\n    \
            sum(cr.den * cr.den) AS den_sq_sum,\n    \
            sum(cr.num * cr.den) AS num_den_sum\n\
        FROM ctx_ratio AS cr\n\
        GROUP BY cr.variant_key"
    );

    Ok(BuiltQuery { sql, binds })
}

/// Build the per-`(context_type, variant_key)` **funnel** sufficient-stats query
/// for one experiment + iteration restricted to a single `context_type`.
///
/// Per assigned context, `windowFunnel(window_seconds[, 'strict_order'])` over
/// the step events evaluates to the deepest step reached; a context is a
/// *success* iff it reached the final step. Emits `(context_type, variant_key,
/// n, successes)`. The per-context ITT lower bound is folded into each step
/// condition (ClickHouse rejects a mixed equality+inequality `JOIN ... ON`),
/// matching the interaction funnel path.
///
/// # Errors
/// Returns [`QueryBuildError::InvalidConfig`] when `steps.len() < 2` or
/// `window_seconds <= 0`.
pub fn build_funnel_cells_query(
    cfg: &FunnelConfig,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    context_type: &str,
    iteration_end: DateTime<Utc>,
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

    let end_ms = iteration_end.timestamp_millis();
    let mut binds = Vec::new();

    let assignments_cte = emit_assignments_cte(
        &mut binds,
        experiment_id,
        iteration_id,
        env_id,
        context_type,
        iteration_end,
    );

    // step predicates for windowFunnel (ITT bound folded into each step)
    let step_predicate_phs: Vec<String> = cfg
        .steps
        .iter()
        .map(|step| push_bind(&mut binds, QueryBind::Str(step.event_key.clone())))
        .collect();
    let step_predicates = step_predicate_phs
        .iter()
        .map(|ph| format!("me.metric_key = {ph} AND me.occurred_at >= asg.assigned_at"))
        .collect::<Vec<_>>()
        .join(",\n                ");

    let ev_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let ev_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let metric_key_filter_phs: Vec<String> = cfg
        .steps
        .iter()
        .map(|step| push_bind(&mut binds, QueryBind::Str(step.event_key.clone())))
        .collect();
    let metric_key_filter = metric_key_filter_phs
        .iter()
        .map(|ph| format!("e.metric_key = {ph}"))
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

    let sql = format!(
        "WITH\n\
        assignments AS (\n\
        {assignments_cte}\
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
                asg.context_type AS context_type,\n        \
                asg.variant_key AS variant_key,\n        \
                windowFunnel({window}{mode_arg})(\n            \
                    toUInt32(toUnixTimestamp(me.occurred_at)),\n            \
                    {step_predicates}\n        \
                ) AS level\n    \
            FROM assignments AS asg\n    \
            LEFT JOIN matched_events AS me\n        \
                ON me.context_key = asg.context_key\n    \
            GROUP BY asg.context_type, asg.context_key, asg.variant_key\n\
        )\n\
        SELECT\n    \
            cf.variant_key AS variant_key,\n    \
            count() AS n,\n    \
            countIf(cf.level >= {final_level}) AS successes\n\
        FROM ctx_funnel AS cf\n\
        GROUP BY cf.variant_key"
    );

    Ok(BuiltQuery { sql, binds })
}

/// Build the per-`(context_type, variant_key)` **assignment-count** query for
/// one experiment + iteration restricted to a single `context_type`.
///
/// Result rows carry `(context_type, variant_key, n)` where `n` is the deduped
/// (`FINAL`) count of assigned contexts — the ITT sample-size denominator for
/// every metric in this `context_type`, and the observed counts for the SRM
/// test.
#[must_use]
pub fn build_assignment_counts_query(
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    context_type: &str,
    iteration_end: DateTime<Utc>,
) -> BuiltQuery {
    let mut binds = Vec::new();
    let assignments_cte = emit_assignments_cte(
        &mut binds,
        experiment_id,
        iteration_id,
        env_id,
        context_type,
        iteration_end,
    );
    let sql = format!(
        "WITH\n\
        assignments AS (\n\
        {assignments_cte}\
        )\n\
        SELECT\n    \
            asg.variant_key AS variant_key,\n    \
            count() AS n\n\
        FROM assignments AS asg\n\
        GROUP BY asg.variant_key"
    );
    BuiltQuery { sql, binds }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use stitchd_core::metric::{AggregationOperator, FunnelStep};

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";
    const EXP_ID: &str = "00000000-0000-0000-0000-000000000002";
    const ITER_ID: &str = "00000000-0000-0000-0000-000000000003";

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

    fn den_cfg() -> AggregationConfig {
        AggregationConfig {
            event_key: "checkout_started".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
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

    // ── Aggregation ──────────────────────────────────────────────────────────

    #[test]
    fn aggregation_dedups_assignments_final_and_groups_per_variant() {
        let q = build_aggregation_cells_query(&count_cfg(), EXP_ID, ITER_ID, ENV_ID, "user", end())
            .unwrap();
        assert!(
            q.sql.contains("FROM experiment_assignments AS a FINAL"),
            "{}",
            q.sql
        );
        assert!(q.sql.contains("GROUP BY ce.variant_key"), "{}", q.sql);
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
    fn aggregation_itt_lower_bound_on_assigned_at() {
        let q = build_aggregation_cells_query(&count_cfg(), EXP_ID, ITER_ID, ENV_ID, "user", end())
            .unwrap();
        assert!(
            q.sql.contains("me.occurred_at >= asg.assigned_at"),
            "{}",
            q.sql
        );
        // The assignment window upper bound + event window upper bound both bind.
        assert!(
            q.sql.contains("a.assigned_at < fromUnixTimestamp64Milli"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn aggregation_bind_order_is_left_to_right() {
        let q = build_aggregation_cells_query(&count_cfg(), EXP_ID, ITER_ID, ENV_ID, "user", end())
            .unwrap();
        // assignments CTE: env, exp, iter, ctx, end
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str(EXP_ID.into()));
        assert_eq!(q.binds[2], QueryBind::Str(ITER_ID.into()));
        assert_eq!(q.binds[3], QueryBind::Str("user".into()));
        assert_eq!(q.binds[4], QueryBind::I64(end().timestamp_millis()));
        // events: env, event_key, ctx, end
        assert_eq!(q.binds[5], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[6], QueryBind::Str("checkout_completed".into()));
        assert_eq!(q.binds[7], QueryBind::Str("user".into()));
        assert_eq!(q.binds[8], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds.len(), 9);
    }

    #[test]
    fn aggregation_continuous_uses_property_value_expr() {
        let q = build_aggregation_cells_query(&sum_cfg(), EXP_ID, ITER_ID, ENV_ID, "user", end())
            .unwrap();
        assert!(
            q.sql.contains("toFloat64OrNull(e.properties['revenue'])"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn aggregation_where_clause_is_last_bind() {
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(serde_json::json!({"==": [{"var": "status"}, "success"]})),
        };
        let q =
            build_aggregation_cells_query(&cfg, EXP_ID, ITER_ID, ENV_ID, "user", end()).unwrap();
        assert_eq!(q.binds.last(), Some(&QueryBind::Str("success".into())));
        // assignments (5) + events (4) + 1 where literal
        assert_eq!(q.binds.len(), 10, "{:?}", q.binds);
    }

    // ── Ratio ────────────────────────────────────────────────────────────────

    #[test]
    fn ratio_emits_second_moments_per_variant() {
        let q = build_ratio_cells_query(
            &sum_cfg(),
            &den_cfg(),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            "user",
            end(),
        )
        .unwrap();
        for col in [
            "sum(cr.num) AS num_sum",
            "sum(cr.den) AS den_sum",
            "sum(cr.num * cr.num) AS num_sq_sum",
            "sum(cr.den * cr.den) AS den_sq_sum",
            "sum(cr.num * cr.den) AS num_den_sum",
        ] {
            assert!(q.sql.contains(col), "missing `{col}` in:\n{}", q.sql);
        }
        assert!(q.sql.contains("GROUP BY cr.variant_key"), "{}", q.sql);
        // INNER JOIN 1:1 on context_key avoids the M×K fan-out.
        assert!(
            q.sql
                .contains("ON num_ctx.context_key = den_ctx.context_key"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn ratio_itt_lower_bound_on_both_legs() {
        let q = build_ratio_cells_query(
            &sum_cfg(),
            &den_cfg(),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            "user",
            end(),
        )
        .unwrap();
        assert!(
            q.sql
                .contains("sumIf(ne.value, ne.occurred_at >= asg.assigned_at)"),
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("sumIf(de.value, de.occurred_at >= asg.assigned_at)"),
            "{}",
            q.sql
        );
    }

    // ── Funnel ───────────────────────────────────────────────────────────────

    #[test]
    fn funnel_emits_n_and_successes_per_variant() {
        let q = build_funnel_cells_query(&funnel_cfg(), EXP_ID, ITER_ID, ENV_ID, "user", end())
            .unwrap();
        assert!(q.sql.contains("count() AS n"), "{}", q.sql);
        assert!(
            q.sql.contains("countIf(cf.level >= 2) AS successes"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("windowFunnel(3600, 'strict_order')"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("me.occurred_at >= asg.assigned_at"),
            "ITT bound folded into step conditions, got:\n{}",
            q.sql
        );
        assert!(q.sql.contains("GROUP BY cf.variant_key"), "{}", q.sql);
    }

    #[test]
    fn funnel_rejects_too_few_steps() {
        let cfg = FunnelConfig {
            steps: vec![FunnelStep {
                event_key: "only".into(),
                where_clause: None,
            }],
            window_seconds: 60,
            count_repeats: false,
        };
        let err =
            build_funnel_cells_query(&cfg, EXP_ID, ITER_ID, ENV_ID, "user", end()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    // ── Assignment counts ──────────────────────────────────────────────────

    #[test]
    fn assignment_counts_groups_per_variant_with_final() {
        let q = build_assignment_counts_query(EXP_ID, ITER_ID, ENV_ID, "user", end());
        assert!(
            q.sql.contains("FROM experiment_assignments AS a FINAL"),
            "{}",
            q.sql
        );
        assert!(q.sql.contains("count() AS n"), "{}", q.sql);
        assert!(q.sql.contains("GROUP BY asg.variant_key"), "{}", q.sql);
        // No events scan — assignment-only.
        assert!(!q.sql.contains("FROM events"), "{}", q.sql);
        assert_eq!(q.binds.len(), 5, "env, exp, iter, ctx, end");
    }
}
