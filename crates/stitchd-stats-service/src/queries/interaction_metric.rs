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

use super::{BuiltQuery, QueryBind, QueryBuildError, jsonlogic_to_sql, push_bind};

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

/// Validate the k-way experiment list: at least 2 (a 1-way "tuple" is a single
/// experiment, no interaction) and all distinct. The upper bound is the
/// operator-configured `STITCHD_STATS_MAX_INTERACTION_ORDER` cap enforced by the
/// caller (`compute_and_persist_interactions`) — the SQL CTE itself is arity-
/// general (it loops `0..k`), so no fixed upper bound is imposed here.
fn validate_exp_ids(exp_ids: &[&str]) -> Result<(), QueryBuildError> {
    if exp_ids.len() < 2 {
        return Err(QueryBuildError::InvalidConfig(format!(
            "interaction order must be at least 2 (got {} experiments)",
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
/// Returns [`QueryBuildError::InvalidConfig`] when `exp_ids` has fewer than 2
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

    // Optional metric `where_clause` (JsonLogic) — applied to the events
    // stream exactly as super::aggregation does so a property-filtered metric
    // (e.g. count WHERE status=='success') is computed over the matching
    // events only, not every event of `metric_key`. The literal binds emitted
    // by `jsonlogic_to_sql` MUST be the LAST binds pushed (they land last in
    // SQL-appearance order — inside the matched_events WHERE).
    let extra_where = match cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n      AND ({frag})")
        }
        None => String::new(),
    };

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
              AND e.occurred_at < fromUnixTimestamp64Milli({ev_end_ph}){extra_where}\n\
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

/// Build a **consolidated** aggregation metric-cell query that produces the
/// per-variant-tuple cells for EVERY candidate tuple in `tuples` — all sharing
/// the same `Aggregation` metric, `env_id`, and `context_type` — in ONE
/// ClickHouse query (review #12).
///
/// The per-tuple builder ([`build_interaction_metric_cells_query`]) issues one
/// FINAL self-join scan of `experiment_assignments` per tuple. For `k`
/// experiments sharing a metric on a context that is `C(k,2) + C(k,3)` separate
/// scans (4 for k=3, 10 for k=4, …), each re-scanning + re-deduping the same
/// assignment rows. This builder collapses them:
///
/// 1. **One** `experiment_assignments FINAL` scan over the *union* of every
///    experiment id across `tuples` (env + context_type + window-bounded),
///    pivoted per `context_key` into a `Map(exp_id → (variant_key, assigned_at))`
///    plus the array of experiment ids present on that context.
/// 2. **One** `events` scan for the metric (the shared `matched_events` CTE).
/// 3. Per tuple a `g{i}` / `ce{i}` CTE pair that *replicates the per-tuple logic
///    exactly*: it keeps only contexts whose `present` array contains ALL of
///    THAT tuple's experiments (`hasAll` — the same intersection the per-tuple
///    INNER JOIN computes), reads each experiment's variant via the map to build
///    the tuple-ordered `variant_keys`, takes its own ITT lower bound
///    `greatest(assigned_at over THAT tuple)`, and groups over its own variant
///    tuple. The per-grid aggregations are `UNION ALL`-ed, each row tagged with
///    its 0-based `tuple_idx` so the caller routes cells back to the right tuple.
///
/// The result rows carry `(tuple_idx: u32, variant_keys: Array(String), n,
/// successes, value_sum, value_sq_sum)` — the per-tuple columns plus the routing
/// index. Each grid branch emits the *identical* cells the per-tuple query emits
/// for that tuple (proven by the live-CH equivalence test).
///
/// `tuples` must be non-empty; each tuple must be ≥ 2 distinct experiments
/// (same contract as the per-tuple builder). The experiment ids are the sorted
/// `Vec<&str>` the sweep already carries (tuple order is preserved in
/// `variant_keys`).
///
/// All SQL is parameterized via [`push_bind`] / [`QueryBind`]; binds are pushed
/// in SQL-appearance order (base scan, then events scan, then the optional
/// metric `where_clause` literals, then each grid's experiment-id references).
///
/// # Errors
/// Returns [`QueryBuildError::InvalidConfig`] when `tuples` is empty or any tuple
/// has fewer than 2 distinct experiments, or [`QueryBuildError::UnsupportedJsonLogic`]
/// / [`QueryBuildError::InvalidJsonLogic`] when the metric `where_clause` cannot
/// be translated.
pub fn build_consolidated_aggregation_cells_query(
    cfg: &AggregationConfig,
    env_id: &str,
    tuples: &[&[&str]],
    context_type: &str,
    interaction_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    if tuples.is_empty() {
        return Err(QueryBuildError::InvalidConfig(
            "consolidated aggregation query needs at least one tuple".into(),
        ));
    }
    for exp_ids in tuples {
        validate_exp_ids(exp_ids)?;
    }

    let end_ms = interaction_end.timestamp_millis();
    let mut binds = Vec::new();

    // Union of experiment ids across all tuples, deduplicated but order-stable
    // (first appearance). Drives the single base scan's `experiment_id IN (…)`.
    let mut union_ids: Vec<&str> = Vec::new();
    for exp_ids in tuples {
        for id in *exp_ids {
            if !union_ids.contains(id) {
                union_ids.push(id);
            }
        }
    }

    // ── base CTE: ONE assignment FINAL scan, pivoted per context ────────────
    // Binds in appearance order: env, context_type, each union experiment id,
    // assigned_at upper bound.
    let base_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let base_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let in_list = union_ids
        .iter()
        .map(|id| {
            let ph = push_bind(&mut binds, QueryBind::Str((*id).to_owned()));
            format!("toUUID({ph})")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let base_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));

    // The pivoted per-context tuple array is materialized once and reused for
    // both the map (variant + assigned_at by experiment id) and the membership
    // array (`present`). `assigned_at` is DateTime64(3,'UTC') in the schema.
    let asg_array = "groupArray((toString(experiment_id), (variant_key, assigned_at)))".to_owned();
    let base_cte = format!(
        "    SELECT\n        \
            context_key AS context_key,\n        \
            CAST({asg_array}, 'Map(String, Tuple(String, DateTime64(3, \\'UTC\\')))') AS m,\n        \
            arrayMap(x -> x.1, {asg_array}) AS present\n    \
        FROM experiment_assignments FINAL\n    \
        WHERE env_id = toUUID({base_env_ph})\n      \
          AND context_type = {base_ctx_ph}\n      \
          AND experiment_id IN ({in_list})\n      \
          AND assigned_at < fromUnixTimestamp64Milli({base_end_ph})\n      \
          AND variant_key != ''\n    \
        GROUP BY context_key\n"
    );

    // ── matched_events CTE: ONE events scan for the metric ──────────────────
    let ev_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let event_ph = push_bind(&mut binds, QueryBind::Str(cfg.event_key.clone()));
    let ev_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let ev_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));
    let value_expr = super::numeric_value_expr(cfg);

    // Optional metric `where_clause` — applied once to the shared events stream
    // (every tuple shares the SAME metric, so a single filtered scan is exactly
    // equivalent to filtering per tuple). Its literals are the LAST binds before
    // the per-grid experiment-id references (it sits inside matched_events, which
    // precedes the grids in SQL-appearance order).
    let extra_where = match cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n      AND ({frag})")
        }
        None => String::new(),
    };

    let matched_events_cte = format!(
        "    SELECT\n        \
            ctx_pair.2 AS context_key,\n        \
            {value_expr} AS value,\n        \
            e.occurred_at AS occurred_at\n    \
        FROM events AS e\n    \
        ARRAY JOIN e.contexts AS ctx_pair\n    \
        WHERE e.env_id = toUUID({ev_env_ph})\n      \
          AND e.metric_key = {event_ph}\n      \
          AND ctx_pair.1 = {ev_ctx_ph}\n      \
          AND e.occurred_at < fromUnixTimestamp64Milli({ev_end_ph}){extra_where}\n"
    );

    // ── per-tuple grid CTEs (g{i} + ce{i}) ──────────────────────────────────
    // `clickhouse-rs` binds `?` positionally — one bind is consumed per `?`, so
    // a reused placeholder is NOT allowed. Each tuple references its experiment
    // ids three times (the `.1` variant reads, the `.2` assigned-at reads, then
    // the `hasAll` membership list), so we push one bind PER occurrence in exact
    // SQL-appearance order: all `.1` reads, then all `.2` reads, then the
    // membership list. These land after the events/where binds (the grids follow
    // matched_events in the SQL text).
    let mut grid_ctes: Vec<String> = Vec::with_capacity(tuples.len());
    let mut union_selects: Vec<String> = Vec::with_capacity(tuples.len());
    for (i, exp_ids) in tuples.iter().enumerate() {
        let variant_array = exp_ids
            .iter()
            .map(|id| {
                let ph = push_bind(&mut binds, QueryBind::Str((*id).to_owned()));
                format!("b.m[{ph}].1")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let assigned_list = exp_ids
            .iter()
            .map(|id| {
                let ph = push_bind(&mut binds, QueryBind::Str((*id).to_owned()));
                format!("b.m[{ph}].2")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let has_all_list = exp_ids
            .iter()
            .map(|id| push_bind(&mut binds, QueryBind::Str((*id).to_owned())))
            .collect::<Vec<_>>()
            .join(", ");

        grid_ctes.push(format!(
            "g{i} AS (\n    \
                SELECT\n        \
                    b.context_key AS context_key,\n        \
                    array({variant_array}) AS variant_keys,\n        \
                    greatest({assigned_list}) AS joint_assigned_at\n    \
                FROM base AS b\n    \
                WHERE hasAll(b.present, array({has_all_list}))\n\
            ),\n\
            ce{i} AS (\n    \
                SELECT\n        \
                    g{i}.variant_keys AS variant_keys,\n        \
                    countIf(me.occurred_at >= g{i}.joint_assigned_at) AS event_count,\n        \
                    sumIf(me.value, me.occurred_at >= g{i}.joint_assigned_at) AS value_sum,\n        \
                    sumIf(me.value * me.value, me.occurred_at >= g{i}.joint_assigned_at) AS value_sq_sum\n    \
                FROM g{i}\n    \
                LEFT JOIN matched_events AS me\n        \
                    ON me.context_key = g{i}.context_key\n    \
                GROUP BY g{i}.context_key, g{i}.variant_keys\n\
            )"
        ));

        // `toUInt32(i)` pins the routing tag's wire type — a bare integer
        // literal infers as UInt8 for small `i`, which the RowBinary `u32`
        // decode rejects.
        union_selects.push(format!(
            "SELECT\n    \
                toUInt32({i}) AS tuple_idx,\n    \
                ce{i}.variant_keys AS variant_keys,\n    \
                count() AS n,\n    \
                countIf(ce{i}.event_count > 0) AS successes,\n    \
                sum(ce{i}.value_sum) AS value_sum,\n    \
                sum(ce{i}.value_sq_sum) AS value_sq_sum\n\
            FROM ce{i}\n\
            GROUP BY ce{i}.variant_keys"
        ));
    }

    let grid_ctes = grid_ctes.join(",\n");
    let union_body = union_selects.join("\nUNION ALL\n");

    let sql = format!(
        "WITH\n\
        base AS (\n\
        {base_cte}\
        ),\n\
        matched_events AS (\n\
        {matched_events_cte}\
        ),\n\
        {grid_ctes}\n\
        {union_body}"
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
/// Returns [`QueryBuildError::InvalidConfig`] when `exp_ids` has fewer than 2
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
    // The per-context ITT lower bound (`me.occurred_at >= s.joint_assigned_at`)
    // is AND-ed into EACH step condition rather than placed in the JOIN ON:
    // ClickHouse rejects a mixed equality+inequality `JOIN ... ON` with
    // INVALID_JOIN_ON_EXPRESSION (see the aggregation/ratio paths, which gate
    // inside the aggregate). Folding it into the step conditions means only
    // post-exposure events can advance the funnel, preserving ITT semantics.
    let step_predicate_phs: Vec<String> = cfg
        .steps
        .iter()
        .map(|step| push_bind(&mut binds, QueryBind::Str(step.event_key.clone())))
        .collect();
    let step_predicates = step_predicate_phs
        .iter()
        .map(|ph| format!("me.metric_key = {ph} AND me.occurred_at >= s.joint_assigned_at"))
        .collect::<Vec<_>>()
        .join(",\n                ");

    // ── matched_events bounds (env + context + metric_key OR-filter + end) ──
    // The OR-filter prunes the event stream to the funnel's step set INSIDE the
    // matched_events CTE, whose table is aliased `e` — so it references
    // `e.metric_key` (NOT `me`, which only exists in the ctx_funnel join).
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

    // ctx_funnel computes per shared context the windowFunnel level, with the
    // step-event stream flattened from events. matched_events is joined back to
    // shared on context_key EQUALITY only (ClickHouse rejects a mixed
    // equality+inequality JOIN ON); the per-context ITT lower bound
    // (joint_assigned_at) instead gates each windowFunnel step condition, so
    // only post-exposure events advance the funnel.
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
                ON me.context_key = s.context_key\n    \
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
/// Returns [`QueryBuildError::InvalidConfig`] when `exp_ids` has fewer than 2
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
    // Numerator leg's optional where_clause — literals MUST be pushed right
    // after the numerator's own binds so they land before any denominator bind
    // in SQL-appearance order (the num_events CTE precedes den_events).
    let num_extra_where = match num_cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n      AND ({frag})")
        }
        None => String::new(),
    };

    // ── denominator events ──────────────────────────────────────────────────
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

    // Pre-aggregate EACH leg to ONE per-context point value FIRST (num_ctx /
    // den_ctx), then INNER-JOIN the two on context_key 1:1 before accumulating
    // the delta-method second moments. Joining BOTH legs to `shared`
    // simultaneously would fan out: a context with M numerator and K
    // denominator events yields M×K rows, inflating num by K and den by M.
    // Because num_ctx and den_ctx both derive from the same `shared`, every
    // shared context appears in each exactly once, so the join is lossless and
    // each context contributes exactly one (num_ctx, den_ctx) pair. The ITT
    // lower bound is applied inside each leg's sumIf (post-exposure events
    // only). The outer SELECT squares / cross-multiplies the per-context totals
    // and sums them across contexts per cell.
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
                s.context_key AS context_key,\n        \
                any(s.variant_keys) AS variant_keys,\n        \
                sumIf(ne.value, ne.occurred_at >= s.joint_assigned_at) AS num\n    \
            FROM shared AS s\n    \
            LEFT JOIN num_events AS ne\n        \
                ON ne.context_key = s.context_key\n    \
            GROUP BY s.context_key\n\
        ),\n\
        den_ctx AS (\n    \
            SELECT\n        \
                s.context_key AS context_key,\n        \
                sumIf(de.value, de.occurred_at >= s.joint_assigned_at) AS den\n    \
            FROM shared AS s\n    \
            LEFT JOIN den_events AS de\n        \
                ON de.context_key = s.context_key\n    \
            GROUP BY s.context_key\n\
        ),\n\
        ctx_ratio AS (\n    \
            SELECT\n        \
                num_ctx.variant_keys AS variant_keys,\n        \
                num_ctx.num AS num,\n        \
                den_ctx.den AS den\n    \
            FROM num_ctx\n    \
            INNER JOIN den_ctx\n        \
                ON num_ctx.context_key = den_ctx.context_key\n\
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

    // ── BUG #2: where_clause must be applied to matched_events ───────────────

    #[test]
    fn agg_without_where_clause_emits_no_extra_predicate() {
        // Baseline: a metric with no where_clause must not reference any
        // `properties[...]` filter (count_cfg/sum_cfg use canonical columns).
        let q = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(
            !q.sql.contains("properties["),
            "count with no where_clause must not read properties, got:\n{}",
            q.sql
        );
        // Only the shared (6) + event (4) binds, no where literal.
        assert_eq!(q.binds.len(), 10, "{:?}", q.binds);
    }

    #[test]
    fn agg_applies_where_clause_to_matched_events() {
        // A property-filtered count (status == 'success') must be computed
        // over the matching events only — the where_clause predicate is
        // appended (parenthesised) inside the matched_events WHERE, and its
        // literal is the LAST bind (SQL-appearance order).
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(serde_json::json!({"==": [{"var": "status"}, "success"]})),
        };
        let q = build_interaction_metric_cells_query(&cfg, ENV_ID, &[EXP_A, EXP_B], "user", end())
            .unwrap();
        // The translator emits `properties['status'] = {pN}`; the placeholder
        // is the last bind index.
        let last_idx = q.binds.len() - 1;
        let expected = format!("AND (properties['status'] = {{p{last_idx}}})");
        assert!(
            q.sql.contains(&expected),
            "where_clause should be appended with parens, got:\n{}",
            q.sql
        );
        // The where literal must land AFTER the matched_events upper bound so
        // it is the last bind in SQL-appearance order.
        assert!(
            q.sql
                .find("fromUnixTimestamp64Milli")
                .is_some_and(|end_pos| {
                    q.sql
                        .find("properties['status']")
                        .is_some_and(|w| w > end_pos)
                }),
            "where_clause must appear after the event upper bound, got:\n{}",
            q.sql
        );
        assert_eq!(q.binds.last(), Some(&QueryBind::Str("success".into())));
        // shared (6) + event (4) + 1 where literal.
        assert_eq!(q.binds.len(), 11, "{:?}", q.binds);
    }

    #[test]
    fn agg_propagates_unsupported_jsonlogic_error() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(serde_json::json!({"%": [{"var": "a"}, 2]})),
        };
        let err =
            build_interaction_metric_cells_query(&cfg, ENV_ID, &[EXP_A, EXP_B], "user", end())
                .unwrap_err();
        assert_eq!(err, QueryBuildError::UnsupportedJsonLogic("%".into()));
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

    // ── Consolidated aggregation (review #12) ────────────────────────────────

    #[test]
    fn consolidated_single_assignment_scan_for_union_of_experiments() {
        // Three experiments → 4 tuples (3 pairs + 1 triple). The base scan must
        // be a SINGLE `experiment_assignments FINAL` over the UNION (A, B, C) —
        // not one self-join per tuple.
        let tuples: Vec<&[&str]> = vec![
            &[EXP_A, EXP_B],
            &[EXP_A, EXP_C],
            &[EXP_B, EXP_C],
            &[EXP_A, EXP_B, EXP_C],
        ];
        let q = build_consolidated_aggregation_cells_query(
            &count_cfg(),
            ENV_ID,
            &tuples,
            "user",
            end(),
        )
        .unwrap();
        // Exactly one assignment scan (no INNER JOIN self-joins at all).
        assert_eq!(
            q.sql.matches("FROM experiment_assignments FINAL").count(),
            1,
            "{}",
            q.sql
        );
        assert!(
            !q.sql.contains("INNER JOIN experiment_assignments"),
            "consolidated path must not self-join assignments, got:\n{}",
            q.sql
        );
        // One events scan shared across all grids.
        assert_eq!(q.sql.matches("FROM events AS e").count(), 1, "{}", q.sql);
        // The union IN-list binds A, B, C exactly once each (3 ids).
        let in_pos = q.sql.find("experiment_id IN (").unwrap();
        let in_frag = &q.sql[in_pos..in_pos + 60];
        assert_eq!(in_frag.matches("toUUID(").count(), 3, "{in_frag}");
    }

    #[test]
    fn consolidated_emits_one_grid_per_tuple_tagged_with_index() {
        let tuples: Vec<&[&str]> = vec![&[EXP_A, EXP_B], &[EXP_A, EXP_B, EXP_C]];
        let q = build_consolidated_aggregation_cells_query(
            &count_cfg(),
            ENV_ID,
            &tuples,
            "user",
            end(),
        )
        .unwrap();
        // One grid CTE pair per tuple.
        assert!(q.sql.contains("g0 AS ("), "{}", q.sql);
        assert!(q.sql.contains("ce0 AS ("), "{}", q.sql);
        assert!(q.sql.contains("g1 AS ("), "{}", q.sql);
        assert!(q.sql.contains("ce1 AS ("), "{}", q.sql);
        // Each UNION branch tags its rows with the tuple index.
        assert!(q.sql.contains("toUInt32(0) AS tuple_idx"), "{}", q.sql);
        assert!(q.sql.contains("toUInt32(1) AS tuple_idx"), "{}", q.sql);
        assert_eq!(q.sql.matches("UNION ALL").count(), 1, "{}", q.sql);
        // Grid 0 (pair AB) over union {A,B,C}: base binds p0..p5, events p6..p9,
        // then the pair's `.1` reads (p10,p11) and `.2` reads (p12,p13). The two
        // assigned-at reads land in their own fresh placeholders (clickhouse-rs
        // binds positionally, so a placeholder is never reused).
        assert!(
            q.sql.contains("greatest(b.m[{p12}].2, b.m[{p13}].2)"),
            "pair grid greatest over its 2 experiments, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn consolidated_grid_keeps_only_contexts_with_all_tuple_experiments() {
        // The per-tuple INNER-join intersection is replicated by `hasAll` over
        // the membership array — a context survives a grid only if ALL that
        // tuple's experiments assigned it.
        let tuples: Vec<&[&str]> = vec![&[EXP_A, EXP_B], &[EXP_A, EXP_B, EXP_C]];
        let q = build_consolidated_aggregation_cells_query(
            &count_cfg(),
            ENV_ID,
            &tuples,
            "user",
            end(),
        )
        .unwrap();
        assert_eq!(
            q.sql.matches("WHERE hasAll(b.present, array(").count(),
            2,
            "one hasAll membership filter per grid, got:\n{}",
            q.sql
        );
        // Per-context ITT bound carried into each ce grid's aggregate gate.
        assert!(
            q.sql
                .contains("countIf(me.occurred_at >= g0.joint_assigned_at)"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn consolidated_output_schema_matches_per_tuple_plus_index() {
        let tuples: Vec<&[&str]> = vec![&[EXP_A, EXP_B]];
        let q = build_consolidated_aggregation_cells_query(
            &count_cfg(),
            ENV_ID,
            &tuples,
            "user",
            end(),
        )
        .unwrap();
        for col in [
            "AS tuple_idx",
            "AS variant_keys",
            "count() AS n",
            "AS successes",
            "AS value_sum",
            "AS value_sq_sum",
        ] {
            assert!(q.sql.contains(col), "missing `{col}` in:\n{}", q.sql);
        }
    }

    #[test]
    fn consolidated_bind_order_base_then_events_then_grids() {
        // Two tuples (AB, ABC) over the union (A, B, C). Binds appear in SQL
        // order: base scan (env, ctx, A, B, C, end), then events (env, event,
        // ctx, end), then each grid's experiment-id references pushed PER
        // occurrence (all `.1` reads, then all `.2` reads, then the hasAll list)
        // because clickhouse-rs binds `?` positionally and never reuses one.
        let tuples: Vec<&[&str]> = vec![&[EXP_A, EXP_B], &[EXP_A, EXP_B, EXP_C]];
        let q = build_consolidated_aggregation_cells_query(
            &count_cfg(),
            ENV_ID,
            &tuples,
            "user",
            end(),
        )
        .unwrap();
        let ms = end().timestamp_millis();
        // base: env, ctx, union A/B/C, end
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str("user".into()));
        assert_eq!(q.binds[2], QueryBind::Str(EXP_A.into()));
        assert_eq!(q.binds[3], QueryBind::Str(EXP_B.into()));
        assert_eq!(q.binds[4], QueryBind::Str(EXP_C.into()));
        assert_eq!(q.binds[5], QueryBind::I64(ms));
        // events: env, event_key, ctx, end
        assert_eq!(q.binds[6], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[7], QueryBind::Str("checkout_completed".into()));
        assert_eq!(q.binds[8], QueryBind::Str("user".into()));
        assert_eq!(q.binds[9], QueryBind::I64(ms));
        // grid 0 (AB): .1 reads (A,B), .2 reads (A,B), hasAll (A,B).
        assert_eq!(
            &q.binds[10..16],
            &[
                QueryBind::Str(EXP_A.into()),
                QueryBind::Str(EXP_B.into()),
                QueryBind::Str(EXP_A.into()),
                QueryBind::Str(EXP_B.into()),
                QueryBind::Str(EXP_A.into()),
                QueryBind::Str(EXP_B.into()),
            ]
        );
        // grid 1 (ABC): .1 reads (A,B,C), .2 reads (A,B,C), hasAll (A,B,C).
        assert_eq!(
            &q.binds[16..25],
            &[
                QueryBind::Str(EXP_A.into()),
                QueryBind::Str(EXP_B.into()),
                QueryBind::Str(EXP_C.into()),
                QueryBind::Str(EXP_A.into()),
                QueryBind::Str(EXP_B.into()),
                QueryBind::Str(EXP_C.into()),
                QueryBind::Str(EXP_A.into()),
                QueryBind::Str(EXP_B.into()),
                QueryBind::Str(EXP_C.into()),
            ]
        );
        assert_eq!(q.binds.len(), 25);
    }

    #[test]
    fn consolidated_applies_where_clause_once_before_grids() {
        // A property-filtered metric: the where literal lands inside the shared
        // matched_events scan, before any grid experiment-id reference.
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(serde_json::json!({"==": [{"var": "status"}, "success"]})),
        };
        let tuples: Vec<&[&str]> = vec![&[EXP_A, EXP_B]];
        let q = build_consolidated_aggregation_cells_query(&cfg, ENV_ID, &tuples, "user", end())
            .unwrap();
        assert!(
            q.sql.contains("AND (properties['status'] = "),
            "where_clause must be applied to the shared events scan, got:\n{}",
            q.sql
        );
        // Single tuple AB → union is just {A, B}. The where literal precedes the
        // grid's experiment-id binds. base(env,ctx,A,B,end = 5) + events(4) +
        // 1 where literal at index 9, then grid AB adds 3×2 = 6 → 16 total.
        assert_eq!(q.binds[9], QueryBind::Str("success".into()));
        assert_eq!(q.binds[10], QueryBind::Str(EXP_A.into()));
        assert_eq!(q.binds[11], QueryBind::Str(EXP_B.into()));
        assert_eq!(q.binds.len(), 16);
    }

    #[test]
    fn consolidated_continuous_uses_property_value_expr() {
        let tuples: Vec<&[&str]> = vec![&[EXP_A, EXP_B]];
        let q =
            build_consolidated_aggregation_cells_query(&sum_cfg(), ENV_ID, &tuples, "user", end())
                .unwrap();
        assert!(
            q.sql.contains("toFloat64OrNull(e.properties['revenue'])"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn consolidated_rejects_empty_tuples() {
        let tuples: Vec<&[&str]> = vec![];
        let err = build_consolidated_aggregation_cells_query(
            &count_cfg(),
            ENV_ID,
            &tuples,
            "user",
            end(),
        )
        .unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn consolidated_rejects_bad_tuple_arity() {
        let tuples: Vec<&[&str]> = vec![&[EXP_A]]; // order < 2
        let err = build_consolidated_aggregation_cells_query(
            &count_cfg(),
            ENV_ID,
            &tuples,
            "user",
            end(),
        )
        .unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn consolidated_context_type_is_bound_not_inlined() {
        let tuples: Vec<&[&str]> = vec![&[EXP_A, EXP_B]];
        let q = build_consolidated_aggregation_cells_query(
            &count_cfg(),
            ENV_ID,
            &tuples,
            "user",
            end(),
        )
        .unwrap();
        assert!(!q.sql.contains("'user'"), "{}", q.sql);
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

    // ── BUG #4: funnel JOIN ON must NOT carry an inequality ──────────────────

    #[test]
    fn funnel_join_on_is_context_key_equality_only() {
        // ClickHouse rejects a mixed equality+inequality `JOIN ... ON`
        // (INVALID_JOIN_ON_EXPRESSION). The matched_events join-back must key
        // on context_key equality ONLY; the ITT lower bound moves into the
        // windowFunnel step conditions.
        let q = build_interaction_funnel_cells_query(
            &funnel_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(
            q.sql.contains("ON me.context_key = s.context_key"),
            "funnel must join on context_key equality, got:\n{}",
            q.sql
        );
        // Isolate the JOIN ON clause (between `ON me.context_key` and the
        // following `GROUP BY`) and assert it contains no occurred_at bound.
        let on_start = q.sql.find("ON me.context_key = s.context_key").unwrap();
        let on_end = q.sql[on_start..].find("GROUP BY").unwrap() + on_start;
        let join_on = &q.sql[on_start..on_end];
        assert!(
            !join_on.contains("occurred_at"),
            "JOIN ON must NOT carry an inequality on occurred_at, got:\n{join_on}"
        );
    }

    #[test]
    fn funnel_itt_bound_folded_into_each_step_condition() {
        // The per-context ITT lower bound is AND-ed into EVERY windowFunnel
        // step condition so only post-exposure events advance the funnel.
        let q = build_interaction_funnel_cells_query(
            &funnel_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        // funnel_cfg has 2 steps → 2 gated step conditions.
        let gated = q
            .sql
            .matches("AND me.occurred_at >= s.joint_assigned_at")
            .count();
        assert_eq!(
            gated, 2,
            "each of the 2 step conditions must be ITT-gated, got {gated} in:\n{}",
            q.sql
        );
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

    // ── BUG #3: ratio must pre-aggregate each leg per context (no fan-out) ───

    #[test]
    fn ratio_pre_aggregates_each_leg_per_context_before_combining() {
        // Each leg is reduced to ONE per-context point value in its own CTE
        // (num_ctx / den_ctx), then the two are INNER-JOINed 1:1 on
        // context_key. Joining BOTH event streams to `shared` simultaneously
        // would fan out (M×K rows for M num + K den events per context).
        let q = build_interaction_ratio_cells_query(
            &count_cfg(),
            &sum_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert!(q.sql.contains("num_ctx AS ("), "{}", q.sql);
        assert!(q.sql.contains("den_ctx AS ("), "{}", q.sql);
        // The combine step joins the two pre-aggregated legs 1:1 on context_key.
        assert!(
            q.sql
                .contains("ON num_ctx.context_key = den_ctx.context_key"),
            "ctx_ratio must INNER JOIN the pre-aggregated legs on context_key, got:\n{}",
            q.sql
        );
        // The num/den event streams must each be joined to `shared` in their
        // OWN leg-CTE, never both together in a single CTE body.
        assert!(
            !q.sql.contains("LEFT JOIN den_events AS de\n        ON de.context_key = s.context_key\n    GROUP BY s.context_key, s.variant_keys"),
            "den must not be double-joined alongside num in one CTE, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn ratio_legs_pre_aggregated_with_separate_shared_joins() {
        // Structural guard against re-introducing the cartesian fan-out: the
        // num leg and den leg each LEFT JOIN `shared` exactly once, in their
        // own CTE (so there are two `FROM shared AS s` occurrences total).
        let q = build_interaction_ratio_cells_query(
            &count_cfg(),
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        assert_eq!(
            q.sql.matches("FROM shared AS s").count(),
            2,
            "expected exactly two per-leg shared joins (num + den), got:\n{}",
            q.sql
        );
        // Per-context grouping is by context_key in each leg.
        assert_eq!(
            q.sql.matches("GROUP BY s.context_key\n").count(),
            2,
            "each leg must group per context, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn ratio_per_cell_output_schema_unchanged() {
        // interaction_compute deserialises by column name — the output schema
        // (variant_keys, n, num_sum, den_sum, num_sq_sum, den_sq_sum,
        // num_den_sum) must be preserved exactly after the fan-out fix.
        let q = build_interaction_ratio_cells_query(
            &count_cfg(),
            &sum_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        for col in [
            "cr.variant_keys AS variant_keys",
            "count() AS n",
            "sum(cr.num) AS num_sum",
            "sum(cr.den) AS den_sum",
            "sum(cr.num * cr.num) AS num_sq_sum",
            "sum(cr.den * cr.den) AS den_sq_sum",
            "sum(cr.num * cr.den) AS num_den_sum",
        ] {
            assert!(q.sql.contains(col), "missing `{col}` in:\n{}", q.sql);
        }
    }

    // ── BUG #2: ratio legs must each apply their where_clause ────────────────

    #[test]
    fn ratio_applies_where_clause_to_each_leg() {
        let num_cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: Some("revenue".into()),
            where_clause: Some(serde_json::json!({"==": [{"var": "currency"}, "USD"]})),
        };
        let den_cfg = AggregationConfig {
            event_key: "session".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(serde_json::json!({"==": [{"var": "platform"}, "web"]})),
        };
        let q = build_interaction_ratio_cells_query(
            &num_cfg,
            &den_cfg,
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap();
        // Both leg filters appear in their respective CTE WHEREs.
        assert!(
            q.sql.contains("AND (properties['currency'] ="),
            "numerator where_clause missing, got:\n{}",
            q.sql
        );
        assert!(
            q.sql.contains("AND (properties['platform'] ="),
            "denominator where_clause missing, got:\n{}",
            q.sql
        );
        // Both literals are bound; numerator literal precedes denominator
        // literal in SQL-appearance order (num_events CTE precedes den_events).
        let usd_pos = q.sql.find("properties['currency']").unwrap();
        let web_pos = q.sql.find("properties['platform']").unwrap();
        assert!(
            usd_pos < web_pos,
            "numerator filter must precede denominator"
        );
        assert!(q.binds.iter().any(|b| b == &QueryBind::Str("USD".into())));
        assert!(q.binds.iter().any(|b| b == &QueryBind::Str("web".into())));
    }

    #[test]
    fn ratio_propagates_unsupported_jsonlogic_error() {
        let bad_den = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(serde_json::json!({">": [{"var": "a"}, 2]})),
        };
        let err = build_interaction_ratio_cells_query(
            &count_cfg(),
            &bad_den,
            ENV_ID,
            &[EXP_A, EXP_B],
            "user",
            end(),
        )
        .unwrap_err();
        assert_eq!(err, QueryBuildError::UnsupportedJsonLogic(">".into()));
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

    /// Phase 10: order ≥ 4 is now ACCEPTED (the CTE is arity-general); the
    /// operator-bounded cap is enforced by the caller, not this builder. A 4-way
    /// build succeeds and emits four aliased assignment joins (a0..a3) with the
    /// four-element variant_keys array.
    #[test]
    fn accepts_order_four() {
        let built = build_interaction_metric_cells_query(
            &count_cfg(),
            ENV_ID,
            &[EXP_A, EXP_B, EXP_C, "00000000-0000-0000-0000-0000000000dd"],
            "user",
            end(),
        )
        .expect("order-4 must build");
        // Four aliased assignment sources participate in the shared join.
        for alias in ["a0", "a1", "a2", "a3"] {
            assert!(
                built.sql.contains(&format!("AS {alias} FINAL")),
                "missing assignment alias {alias} in order-4 CTE"
            );
        }
        // The variant tuple array spans all four aliases.
        assert!(built.sql.contains("a3.variant_key"));
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
