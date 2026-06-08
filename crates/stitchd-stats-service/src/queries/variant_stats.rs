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

/// Build the **per-assigned-unit contextual reward** query for one experiment +
/// iteration restricted to a single `context_type`, a single `Aggregation`
/// reward metric, and a single context FEATURE read from event properties.
///
/// For each assigned unit (deduped `FINAL` first-exposure assignments) this
/// returns:
/// * `variant_key` — the unit's assigned arm,
/// * `feature_value` — the value of `events.properties[feature_param]` taken from
///   any of the unit's post-exposure metric events (`''` when the unit fired no
///   event or the property is absent — the contextual encoder maps `''` to the
///   numeric `0.0` / the one-hot baseline), and
/// * `reward` — the post-exposure metric value sum over `[assigned_at,
///   iteration_end)` (`0` for a non-firing unit, ITT).
///
/// One row per assigned unit (NOT pre-aggregated per variant) so the
/// stats-service can fit a per-variant ridge regression over `(feature, reward)`
/// design rows. Mirrors [`build_aggregation_cells_query`]'s ITT discipline.
///
/// # Errors
/// Returns [`QueryBuildError`] when the metric `where_clause` JsonLogic cannot
/// be translated.
pub fn build_contextual_reward_rows_query(
    cfg: &AggregationConfig,
    feature_param: &str,
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
    // The feature property key is admin-controlled free-form; escape it into the
    // `properties['…']` map-key literal exactly like `on_field`.
    let feature_key = super::clickhouse_escape(feature_param);

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
                e.properties['{feature_key}'] AS feature_value,\n        \
                e.occurred_at AS occurred_at\n    \
            FROM events AS e\n    \
            ARRAY JOIN e.contexts AS ctx_pair\n    \
            WHERE e.env_id = toUUID({ev_env_ph})\n      \
              AND e.metric_key = {event_ph}\n      \
              AND ctx_pair.1 = {ev_ctx_ph}\n      \
              AND e.occurred_at < fromUnixTimestamp64Milli({ev_end_ph}){extra_where}\n\
        )\n\
        SELECT\n    \
            asg.variant_key AS variant_key,\n    \
            anyIf(me.feature_value, me.occurred_at >= asg.assigned_at) AS feature_value,\n    \
            sumIf(me.value, me.occurred_at >= asg.assigned_at) AS reward\n\
        FROM assignments AS asg\n\
        LEFT JOIN matched_events AS me\n    \
            ON me.context_key = asg.context_key\n\
        GROUP BY asg.context_type, asg.context_key, asg.variant_key"
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

    // CRITICAL: bind push order MUST match the order the `{pN}` placeholders
    // appear TEXTUALLY in the assembled SQL, because
    // `dispatch::rewrite_placeholders_to_clickhouse` rewrites `{pN}` → `?` in
    // text order and ClickHouse binds positionally. The emitted SQL lays out
    // `matched_events` (env / ctx / metric-key filters / end) BEFORE
    // `ctx_funnel` (the windowFunnel step predicates), so the matched_events
    // WHERE binds are pushed FIRST and the step-predicate binds LAST. (A prior
    // version pushed the step predicates before the matched_events binds, which
    // cross-bound every funnel query.)
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

    // step predicates for windowFunnel (ITT bound folded into each step) —
    // pushed LAST since `ctx_funnel` is the last CTE textually.
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

/// Build the per-`(context_type, variant_key)` **raw per-unit sample** query
/// for one experiment + iteration restricted to a single `context_type` and a
/// single `Aggregation` metric — the input a percentile significance test (a
/// bootstrap CI on the q-th quantile) needs but the scalar sufficient-stats
/// path cannot reconstruct.
///
/// Mirrors [`build_aggregation_cells_query`] exactly (same `assignments` CTE,
/// same ITT `me.occurred_at >= asg.assigned_at` bound, same coalesced
/// [`super::numeric_value_expr`] value, same per-unit reduction), but instead of
/// the second moments it collects each assigned unit's per-unit metric value
/// (`value_sum` over its post-exposure events — a non-firing unit contributes
/// `0`, ITT) into `groupArray(value)` per variant. The array is capped at
/// `sample_cap` via `groupArray(sample_cap)(...)` so a hot variant cannot
/// stream an unbounded array back into the compute pass; the caller notes when
/// the returned length equals the cap (possible truncation).
///
/// Result rows carry `(variant_key, samples)` where `samples` is an
/// `Array(Float64)` of per-unit values (length ≤ `sample_cap`).
///
/// # Errors
/// Returns [`QueryBuildError`] when the metric `where_clause` JsonLogic cannot
/// be translated.
pub fn build_percentile_samples_query(
    cfg: &AggregationConfig,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    context_type: &str,
    iteration_end: DateTime<Utc>,
    sample_cap: u64,
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

    let extra_where = match cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n      AND ({frag})")
        }
        None => String::new(),
    };

    // The per-unit value is the unit's post-exposure metric sum (ITT — a unit
    // with no qualifying event contributes 0), reduced per assigned context,
    // then collected per variant via groupArray (capped). `sample_cap` is a
    // SQL literal (a trusted internal bound, not user input).
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
        ctx_values AS (\n    \
            SELECT\n        \
                asg.context_type AS context_type,\n        \
                asg.variant_key AS variant_key,\n        \
                sumIf(me.value, me.occurred_at >= asg.assigned_at) AS unit_value\n    \
            FROM assignments AS asg\n    \
            LEFT JOIN matched_events AS me\n        \
                ON me.context_key = asg.context_key\n    \
            GROUP BY asg.context_type, asg.context_key, asg.variant_key\n\
        )\n\
        SELECT\n    \
            cv.variant_key AS variant_key,\n    \
            groupArray({sample_cap})(cv.unit_value) AS samples\n\
        FROM ctx_values AS cv\n\
        GROUP BY cv.variant_key"
    );

    Ok(BuiltQuery { sql, binds })
}

/// Build the per-**unit** CUPED query for one experiment + iteration restricted
/// to a single `context_type` and a single `Aggregation` metric: each assigned
/// unit's post-period `Y` AND its pre-period covariate `X_pre`, in **one** query
/// over **one** shared `events` scan.
///
/// This MERGES what used to be two separate per-unit scans (FIX B5):
///
/// * `Y`  — post-exposure metric sum over `[assigned_at, iteration_end)` (ITT);
/// * `X_pre` — pre-period metric sum over the ABSOLUTE window
///   `[pre_start, pre_end)` (measured BEFORE exposure, NOT gated on `assigned_at`).
///
/// Both windows are carved out of a single `matched_events` CTE whose WHERE spans
/// their **union** `[pre_start, iteration_end)`, then split with two `sumIf`s in
/// one `GROUP BY (context_type, context_key, variant_key)`. The two windows stay
/// disjoint because `pre_end <= assigned_at` (the pre-period ends at the
/// experiment start, exposure happens at/after it): a pre event has
/// `occurred_at < pre_end <= assigned_at` so it can never satisfy the `Y`
/// predicate `occurred_at >= assigned_at`, and a post event has
/// `occurred_at >= assigned_at >= pre_end` so it can never satisfy the `X_pre`
/// predicate `occurred_at < pre_end`. This halves the ClickHouse reads vs. the
/// two-query version and lets the caller skip the Rust `HashMap` re-join (each
/// row already carries both `y` and `x_pre`).
///
/// Mirrors [`build_aggregation_cells_query`]'s `assignments` CTE + coalesced
/// [`super::numeric_value_expr`] value exactly, but keeps `context_key` so each
/// row is one assigned unit (NOT reduced per variant). A unit with no qualifying
/// event in a window contributes `0` for that window (ITT — the LEFT JOIN keeps
/// it). The `on_field`-aware value expression means a property-valued metric
/// reads `properties[on_field]` for BOTH `Y` and `X_pre` (no silent `X_pre = 0`).
///
/// Result rows carry `(context_key, variant_key, y, x_pre)`. The `context_type`
/// is not re-projected (the query is already scoped to one `context_type` — the
/// caller supplies it).
///
/// # Errors
/// Returns [`QueryBuildError`] when the metric `where_clause` JsonLogic cannot
/// be translated.
#[allow(clippy::too_many_arguments)]
pub fn build_unit_values_with_pre_query(
    cfg: &AggregationConfig,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    context_type: &str,
    iteration_end: DateTime<Utc>,
    pre_start: DateTime<Utc>,
    pre_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    let end_ms = iteration_end.timestamp_millis();
    let pre_start_ms = pre_start.timestamp_millis();
    let pre_end_ms = pre_end.timestamp_millis();
    let mut binds = Vec::new();

    let assignments_cte = emit_assignments_cte(
        &mut binds,
        experiment_id,
        iteration_id,
        env_id,
        context_type,
        iteration_end,
    );

    // Binds pushed in SQL-appearance order (clickhouse-rs binds `?` positionally):
    //   …assignments CTE binds…,
    //   ev_env, event_key, ev_ctx, span_lo (= pre_start), span_hi (= iter_end),
    //   y_hi (= iter_end, the post upper bound inside the sumIf),
    //   x_lo (= pre_start), x_hi (= pre_end),
    //   then any where_clause literals.
    let ev_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let event_ph = push_bind(&mut binds, QueryBind::Str(cfg.event_key.clone()));
    let ev_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    // Union span for the single scan: [pre_start, iteration_end).
    let span_lo_ph = push_bind(&mut binds, QueryBind::I64(pre_start_ms));
    let span_hi_ph = push_bind(&mut binds, QueryBind::I64(end_ms));
    let value_expr = super::numeric_value_expr(cfg);

    // The where_clause literals must be the LAST binds, but they are emitted
    // textually INSIDE the `matched_events` WHERE (before the SELECT's sumIf
    // bounds). To keep push order == textual order, the sumIf-bound binds
    // (y_hi / x_lo / x_hi) are pushed AFTER the where fragment. We therefore
    // build the fragment first, then push the sumIf bounds.
    let extra_where = match cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n      AND ({frag})")
        }
        None => String::new(),
    };

    // Post upper bound (Y is `[assigned_at, iteration_end)`; the lower bound is
    // the per-row `asg.assigned_at`, not a bind) and the absolute pre window
    // bounds for X_pre.
    let y_hi_ph = push_bind(&mut binds, QueryBind::I64(end_ms));
    let x_lo_ph = push_bind(&mut binds, QueryBind::I64(pre_start_ms));
    let x_hi_ph = push_bind(&mut binds, QueryBind::I64(pre_end_ms));

    // One scan over the union span [pre_start, iteration_end); two sumIf carve
    // out the post (Y) and pre (X_pre) windows per assigned unit. The windows
    // are disjoint by construction (pre_end <= assigned_at), so no event is
    // double-counted across Y and X_pre.
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
              AND e.occurred_at >= fromUnixTimestamp64Milli({span_lo_ph})\n      \
              AND e.occurred_at <  fromUnixTimestamp64Milli({span_hi_ph}){extra_where}\n\
        )\n\
        SELECT\n    \
            asg.context_key AS context_key,\n    \
            asg.variant_key AS variant_key,\n    \
            sumIf(\n        \
                me.value,\n        \
                me.occurred_at >= asg.assigned_at\n          \
                  AND me.occurred_at < fromUnixTimestamp64Milli({y_hi_ph})\n    \
            ) AS y,\n    \
            sumIf(\n        \
                me.value,\n        \
                me.occurred_at >= fromUnixTimestamp64Milli({x_lo_ph})\n          \
                  AND me.occurred_at < fromUnixTimestamp64Milli({x_hi_ph})\n    \
            ) AS x_pre\n\
        FROM assignments AS asg\n\
        LEFT JOIN matched_events AS me\n    \
            ON me.context_key = asg.context_key\n\
        GROUP BY asg.context_type, asg.context_key, asg.variant_key"
    );

    Ok(BuiltQuery { sql, binds })
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

    // ── Contextual reward rows ───────────────────────────────────────────────

    #[test]
    fn contextual_reward_rows_projects_feature_value_and_reward_per_unit() {
        let q = build_contextual_reward_rows_query(
            &count_cfg(),
            "plan",
            EXP_ID,
            ITER_ID,
            ENV_ID,
            "user",
            end(),
        )
        .unwrap();
        // Feature value pulled from the event properties map.
        assert!(
            q.sql.contains("e.properties['plan'] AS feature_value"),
            "{}",
            q.sql
        );
        // One row per assigned unit (grouped by context_key), not per variant.
        assert!(
            q.sql
                .contains("GROUP BY asg.context_type, asg.context_key, asg.variant_key"),
            "{}",
            q.sql
        );
        // ITT post-exposure reward sum.
        assert!(
            q.sql
                .contains("sumIf(me.value, me.occurred_at >= asg.assigned_at) AS reward"),
            "{}",
            q.sql
        );
        // The feature value is taken from a post-exposure event.
        assert!(
            q.sql.contains(
                "anyIf(me.feature_value, me.occurred_at >= asg.assigned_at) AS feature_value"
            ),
            "{}",
            q.sql
        );
    }

    #[test]
    fn contextual_reward_rows_escapes_feature_property_key() {
        // A feature name with a quote must be escaped into the map-key literal.
        let q = build_contextual_reward_rows_query(
            &count_cfg(),
            "pl'an",
            EXP_ID,
            ITER_ID,
            ENV_ID,
            "user",
            end(),
        )
        .unwrap();
        assert!(q.sql.contains("e.properties['pl\\'an']"), "{}", q.sql);
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

    /// Collect the `{pN}` placeholder indices in the order they appear
    /// TEXTUALLY in the SQL string (first char of each `{pN}` token).
    fn placeholder_order(sql: &str) -> Vec<usize> {
        let mut out = Vec::new();
        let bytes = sql.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'{' && bytes.get(i + 1) == Some(&b'p') {
                let mut j = i + 2;
                let mut num = String::new();
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    num.push(bytes[j] as char);
                    j += 1;
                }
                if bytes.get(j) == Some(&b'}') && !num.is_empty() {
                    out.push(num.parse().unwrap());
                    i = j + 1;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    /// FIX 1 REGRESSION GUARD: the funnel query's `{pN}` placeholders must
    /// appear in STRICTLY ASCENDING order in the emitted SQL, and each bind in
    /// `binds[N]` must be the value referenced at textual position N.
    /// `dispatch::rewrite_placeholders_to_clickhouse` rewrites `{pN}` → `?` in
    /// TEXT order and ClickHouse binds positionally, so any out-of-order `{pN}`
    /// (the prior bug — step predicates pushed before the matched_events WHERE
    /// binds even though `ctx_funnel` is emitted after `matched_events`)
    /// cross-binds every funnel query.
    #[test]
    fn funnel_bind_order_is_left_to_right() {
        let q = build_funnel_cells_query(&funnel_cfg(), EXP_ID, ITER_ID, ENV_ID, "user", end())
            .unwrap();

        // (a) placeholders appear textually in strictly ascending order.
        let order = placeholder_order(&q.sql);
        assert_eq!(
            order,
            (0..order.len()).collect::<Vec<_>>(),
            "funnel {{pN}} must be textually ascending (push order == text order); got {order:?}\n{}",
            q.sql
        );

        // (b) the bind vector aligns with that textual order.
        // assignments CTE: env, exp, iter, ctx, end
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str(EXP_ID.into()));
        assert_eq!(q.binds[2], QueryBind::Str(ITER_ID.into()));
        assert_eq!(q.binds[3], QueryBind::Str("user".into()));
        assert_eq!(q.binds[4], QueryBind::I64(end().timestamp_millis()));
        // matched_events WHERE: env, ctx, step metric_key filters (view, checkout), end
        assert_eq!(q.binds[5], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[6], QueryBind::Str("user".into()));
        assert_eq!(q.binds[7], QueryBind::Str("view".into()));
        assert_eq!(q.binds[8], QueryBind::Str("checkout".into()));
        assert_eq!(q.binds[9], QueryBind::I64(end().timestamp_millis()));
        // ctx_funnel windowFunnel step predicates (view, checkout) — LAST.
        assert_eq!(q.binds[10], QueryBind::Str("view".into()));
        assert_eq!(q.binds[11], QueryBind::Str("checkout".into()));
        assert_eq!(q.binds.len(), 12);

        // The two windowFunnel step placeholders are the highest-numbered ones
        // and live inside the windowFunnel(...) call (textually after the
        // matched_events WHERE), proving the ordering fix concretely.
        assert!(
            q.sql
                .contains("me.metric_key = {p10} AND me.occurred_at >= asg.assigned_at"),
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("me.metric_key = {p11} AND me.occurred_at >= asg.assigned_at"),
            "{}",
            q.sql
        );
    }

    // ── Percentile raw samples ─────────────────────────────────────────────

    #[test]
    fn percentile_samples_group_arrays_per_variant_capped() {
        let q =
            build_percentile_samples_query(&sum_cfg(), EXP_ID, ITER_ID, ENV_ID, "user", end(), 100)
                .unwrap();
        assert!(
            q.sql.contains("groupArray(100)(cv.unit_value) AS samples"),
            "{}",
            q.sql
        );
        assert!(q.sql.contains("GROUP BY cv.variant_key"), "{}", q.sql);
        // Same ITT discipline as the aggregation cells query.
        assert!(
            q.sql.contains("FROM experiment_assignments AS a FINAL"),
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("sumIf(me.value, me.occurred_at >= asg.assigned_at) AS unit_value"),
            "{}",
            q.sql
        );
        // Continuous metric uses the property value expr (same as aggregation).
        assert!(
            q.sql.contains("toFloat64OrNull(e.properties['revenue'])"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn percentile_samples_bind_order_is_left_to_right() {
        let q = build_percentile_samples_query(
            &count_cfg(),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            "user",
            end(),
            50,
        )
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

    // ── Per-unit CUPED Y + X_pre (single merged query — FIX B5) ─────────────

    fn pre_window() -> (DateTime<Utc>, DateTime<Utc>) {
        // started_at = end()-30d, pre-period = 14d before that.
        let started = Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap();
        let pre_start = started - chrono::Duration::days(14);
        (pre_start, started)
    }

    /// The merged query keeps per-unit granularity, emits BOTH `y` and `x_pre`
    /// from ONE shared `matched_events` scan via two `sumIf`s, and groups once.
    #[test]
    fn unit_values_with_pre_emits_both_sums_from_one_scan() {
        let (lo, hi) = pre_window();
        let q = build_unit_values_with_pre_query(
            &sum_cfg(),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            "user",
            end(),
            lo,
            hi,
        )
        .unwrap();
        // Per-UNIT: groups by context_key (NOT collapsed to per-variant) and
        // projects (context_key, variant_key, y, x_pre).
        assert!(
            q.sql
                .contains("GROUP BY asg.context_type, asg.context_key, asg.variant_key"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("asg.context_key AS context_key"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("asg.variant_key AS variant_key"),
            "{}",
            q.sql
        );
        // BOTH sums present.
        assert!(
            q.sql.contains(") AS y,"),
            "missing y sumIf; got:\n{}",
            q.sql
        );
        assert!(
            q.sql.contains(") AS x_pre"),
            "missing x_pre sumIf; got:\n{}",
            q.sql
        );
        // ONE event scan / ONE LEFT JOIN — not two CTEs.
        assert_eq!(
            q.sql.matches("FROM events AS e").count(),
            1,
            "must be a single events scan; got:\n{}",
            q.sql
        );
        assert_eq!(
            q.sql.matches("LEFT JOIN matched_events").count(),
            1,
            "must be a single join; got:\n{}",
            q.sql
        );
        // Same ITT discipline + assignments CTE as the aggregation cells query,
        // no inlined IN-list.
        assert!(
            q.sql.contains("FROM experiment_assignments AS a FINAL"),
            "{}",
            q.sql
        );
        assert!(
            !q.sql.contains(" IN ("),
            "must not inline an IN-list; got:\n{}",
            q.sql
        );
        // Continuous / property-valued metric uses the shared coalesced value
        // expr for BOTH y and x_pre (FIX 7 — no silent X_pre=0).
        assert!(
            q.sql.contains("toFloat64OrNull(e.properties['revenue'])"),
            "property-valued metric must read properties[on_field]; got:\n{}",
            q.sql
        );
    }

    /// The two windows are disjoint by construction: Y is gated on the per-row
    /// `asg.assigned_at` (post-exposure), X_pre on the ABSOLUTE pre window. The
    /// single CTE WHERE spans their union `[pre_start, iteration_end)`.
    #[test]
    fn unit_values_with_pre_windows_are_disjoint() {
        let (lo, hi) = pre_window();
        let q = build_unit_values_with_pre_query(
            &count_cfg(),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            "user",
            end(),
            lo,
            hi,
        )
        .unwrap();
        // Y sumIf is gated on exposure (assigned_at).
        assert!(
            q.sql.contains("me.occurred_at >= asg.assigned_at"),
            "Y must be post-exposure; got:\n{}",
            q.sql
        );
        // X_pre sumIf uses absolute bounds, NOT assigned_at.
        assert!(
            q.sql.contains("me.occurred_at >= fromUnixTimestamp64Milli")
                && q.sql.contains("me.occurred_at < fromUnixTimestamp64Milli"),
            "X_pre must use the absolute pre window; got:\n{}",
            q.sql
        );
        // The shared scan spans the union [pre_start, iteration_end).
        assert!(
            q.sql.contains("e.occurred_at >= fromUnixTimestamp64Milli")
                && q.sql.contains("e.occurred_at <  fromUnixTimestamp64Milli"),
            "scan must span the union window; got:\n{}",
            q.sql
        );
    }

    #[test]
    fn unit_values_with_pre_bind_order_is_left_to_right() {
        let (lo, hi) = pre_window();
        let q = build_unit_values_with_pre_query(
            &count_cfg(),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            "user",
            end(),
            lo,
            hi,
        )
        .unwrap();
        // assignments CTE: env, exp, iter, ctx, end
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str(EXP_ID.into()));
        assert_eq!(q.binds[2], QueryBind::Str(ITER_ID.into()));
        assert_eq!(q.binds[3], QueryBind::Str("user".into()));
        assert_eq!(q.binds[4], QueryBind::I64(end().timestamp_millis()));
        // matched_events: env, event_key, ctx, span_lo (=pre_start), span_hi (=iter_end)
        assert_eq!(q.binds[5], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[6], QueryBind::Str("checkout_completed".into()));
        assert_eq!(q.binds[7], QueryBind::Str("user".into()));
        assert_eq!(q.binds[8], QueryBind::I64(lo.timestamp_millis()));
        assert_eq!(q.binds[9], QueryBind::I64(end().timestamp_millis()));
        // SELECT sumIf bounds: y_hi (=iter_end), x_lo (=pre_start), x_hi (=pre_end)
        assert_eq!(q.binds[10], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds[11], QueryBind::I64(lo.timestamp_millis()));
        assert_eq!(q.binds[12], QueryBind::I64(hi.timestamp_millis()));
        assert_eq!(q.binds.len(), 13);
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
