//! Per-(variant_A × variant_B) **metric** cell aggregation for cross-experiment
//! interaction analysis.
//!
//! [`super::interaction`] produces the bare contingency table of *shared
//! contexts* (how many contexts landed in each `(a_variant, b_variant)` cell).
//! That tells us the population overlap but says nothing about the *metric*.
//! This module goes one step further: for a single shared metric it aggregates
//! the metric outcome per cell so the interaction significance math
//! ([`stitchd_core::experimentation::stats::interaction`]) can run.
//!
//! Two cell shapes are produced depending on the metric:
//!
//! - **Conversion** (an `Aggregation` metric whose aggregator counts
//!   occurrences — `count`/`uniq`): each shared context either fired ≥1
//!   qualifying event or not. The cell row is `(a_variant, b_variant, n,
//!   successes)` where `n` is the number of shared contexts in the cell and
//!   `successes` is the number of those contexts with at least one matching
//!   event → a [`stitchd_core::experimentation::stats::interaction::BinaryCell`].
//! - **Continuous** (a numeric `Aggregation` metric — `sum`/`avg`/percentiles):
//!   each shared context carries a per-context metric value (its summed event
//!   value over the window). The cell row is `(a_variant, b_variant, n, sum,
//!   sum_sq)` → a
//!   [`stitchd_core::experimentation::stats::interaction::ContinuousCell`].
//!
//! Funnel and ratio metrics are intentionally **out of scope** for interaction
//! v1 — the orchestrator skips them (see [`crate::interaction_compute`]); this
//! builder only handles `Aggregation` metrics and classifies them via
//! [`MetricCellKind`].
//!
//! ## Query shape
//!
//! 1. `shared` CTE — `experiment_assignments AS a FINAL` INNER JOIN
//!    `experiment_assignments AS b FINAL` on the shared context identity
//!    `(env_id, context_type, context_key)`, restricted to experiment A / B and
//!    bounded by the overlap-window upper bound `assigned_at < interaction_end`.
//!    Mirrors [`super::interaction`]'s self-join but **also carries**
//!    `greatest(a.assigned_at, b.assigned_at) AS joint_assigned_at` — the
//!    timestamp at which the context was simultaneously exposed to *both*
//!    experiments. This is the first-exposure (ITT) lower bound for the cell.
//! 2. `matched_events` CTE — the candidate events for the metric, flattened to
//!    `(context_key, value, occurred_at)`. Events are matched on
//!    `(env_id, context_type)` and the metric's `event_key` via
//!    `ARRAY JOIN e.contexts` (CH 24's analyzer rejects `arrayExists` inside
//!    `JOIN ON`; mirrors [`super::aggregation`]) and upper-bounded by
//!    `occurred_at < interaction_end`. The per-context *lower* bound is applied
//!    in the next step (it depends on the joint-exposure time, which is per
//!    context).
//! 3. `ctx_events` CTE — per shared context, the metric outcome. `shared` is
//!    `LEFT JOIN`ed to `matched_events` on `context_key` (equality only — CH
//!    24's analyzer rejects a mixed equality+inequality `JOIN ON` across the two
//!    tables, `INVALID_JOIN_ON_EXPRESSION`). The ITT lower bound is instead
//!    applied as a conditional-aggregation predicate
//!    `me.occurred_at >= s.joint_assigned_at` inside `countIf` / `sumIf`, so only
//!    events at/after the context's JOINT exposure are counted — strict
//!    first-exposure ITT, matching every other experiment metric on this
//!    platform. The `LEFT JOIN` keeps contexts with zero qualifying events (they
//!    still count toward `n`, just not `successes`). Aggregates per context:
//!    `event_count` (`>0` ⇒ conversion success) and `value_sum` / `value_sq_sum`.
//! 4. Outer `SELECT` — `GROUP BY (a_variant, b_variant)` producing the per-cell
//!    aggregates.
//!
//! ## Bind order
//!
//! `clickhouse-rs` binds `?` by SQL position, so binds are pushed left-to-right
//! in appearance order. The per-context ITT lower bound (`joint_assigned_at`) is
//! a *column reference* carried out of `shared`, not a bind, so the bind list is
//! unchanged from the lifetime-events version:
//!
//!   1. `env_id`          (`a.env_id = toUUID(?)`)
//!   2. `exp_a`           (`a.experiment_id = toUUID(?)`)
//!   3. `context_type`    (`a.context_type = ?`)
//!   4. `interaction_end` (`a.assigned_at < ?` — A side)
//!   5. `exp_b`           (`b.experiment_id = toUUID(?)`)
//!   6. `interaction_end` (`b.assigned_at < ?` — B side)
//!   7. `env_id`          (`e.env_id = toUUID(?)` — events side)
//!   8. `event_key`       (`e.metric_key = ?`)
//!   9. `context_type`    (`ctx_pair.1 = ?` — events context filter)
//!  10. `interaction_end` (`e.occurred_at < ?` — events window upper bound)

use chrono::{DateTime, Utc};
use stitchd_core::metric::{AggregationConfig, AggregationOperator};

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

/// Build the per-(a_variant, b_variant) metric-cell aggregation query for the
/// interaction between experiments `exp_a` and `exp_b` within `env_id`,
/// restricted to one `context_type` and one shared `Aggregation` metric.
///
/// The result rows always carry five columns —
/// `(a_variant_key, b_variant_key, n, successes, value_sum, value_sq_sum)` —
/// regardless of [`MetricCellKind`]. The caller reads only the columns relevant
/// to the kind:
///
/// - [`MetricCellKind::Conversion`] → `n`, `successes` (→ `BinaryCell`),
/// - [`MetricCellKind::Continuous`] → `n`, `value_sum`, `value_sq_sum`
///   (→ `ContinuousCell`).
///
/// Carrying both shapes in one row keeps the SQL identical across kinds (only
/// the metric value-expression differs) and lets the integration tests assert
/// the full grid in a single fetch.
///
/// `interaction_end` upper-bounds both the assignment overlap window and the
/// event window. Callers pass the end of the overlap between the two
/// experiments' active periods.
///
/// # Errors
/// Returns [`QueryBuildError::InvalidConfig`] when `exp_a == exp_b` (an
/// experiment cannot interact with itself).
pub fn build_interaction_metric_cells_query(
    cfg: &AggregationConfig,
    env_id: &str,
    exp_a: &str,
    exp_b: &str,
    context_type: &str,
    interaction_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    if exp_a == exp_b {
        return Err(QueryBuildError::InvalidConfig(
            "exp_a and exp_b must be distinct experiments".into(),
        ));
    }

    let end_ms = interaction_end.timestamp_millis();
    let mut binds = Vec::new();

    // ── shared CTE: assignment self-join (same shape as super::interaction) ──
    let env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let exp_a_ph = push_bind(&mut binds, QueryBind::Str(exp_a.to_owned()));
    let ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let a_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));
    let exp_b_ph = push_bind(&mut binds, QueryBind::Str(exp_b.to_owned()));
    let b_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));

    // ── ctx_events CTE: per-context metric outcome over events ──────────────
    let ev_env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let event_ph = push_bind(&mut binds, QueryBind::Str(cfg.event_key.clone()));
    let ev_ctx_ph = push_bind(&mut binds, QueryBind::Str(context_type.to_owned()));
    let ev_end_ph = push_bind(&mut binds, QueryBind::I64(end_ms));

    // The numeric value expression mirrors super::aggregation's canonical
    // column routing: None/""/value/value_double/value_int read the typed
    // columns; any other on_field is a properties[<name>] string coerced to
    // Float64.
    let value_expr = numeric_value_expr(cfg);

    let sql = format!(
        "WITH\n\
        shared AS (\n    \
            SELECT\n        \
                a.context_key AS context_key,\n        \
                a.variant_key AS a_variant_key,\n        \
                b.variant_key AS b_variant_key,\n        \
                greatest(a.assigned_at, b.assigned_at) AS joint_assigned_at\n    \
            FROM experiment_assignments AS a FINAL\n    \
            INNER JOIN experiment_assignments AS b FINAL\n        \
                ON a.env_id = b.env_id\n       \
                AND a.context_type = b.context_type\n       \
                AND a.context_key = b.context_key\n    \
            WHERE a.env_id = toUUID({env_ph})\n      \
              AND a.experiment_id = toUUID({exp_a_ph})\n      \
              AND a.context_type = {ctx_ph}\n      \
              AND a.assigned_at < fromUnixTimestamp64Milli({a_end_ph})\n      \
              AND b.experiment_id = toUUID({exp_b_ph})\n      \
              AND b.assigned_at < fromUnixTimestamp64Milli({b_end_ph})\n      \
              AND a.variant_key != ''\n      \
              AND b.variant_key != ''\n\
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
                s.a_variant_key AS a_variant_key,\n        \
                s.b_variant_key AS b_variant_key,\n        \
                countIf(me.occurred_at >= s.joint_assigned_at) AS event_count,\n        \
                sumIf(me.value, me.occurred_at >= s.joint_assigned_at) AS value_sum,\n        \
                sumIf(me.value * me.value, me.occurred_at >= s.joint_assigned_at) AS value_sq_sum\n    \
            FROM shared AS s\n    \
            LEFT JOIN matched_events AS me\n        \
                ON me.context_key = s.context_key\n    \
            GROUP BY s.context_key, s.a_variant_key, s.b_variant_key\n\
        )\n\
        SELECT\n    \
            ce.a_variant_key AS a_variant_key,\n    \
            ce.b_variant_key AS b_variant_key,\n    \
            count() AS n,\n    \
            countIf(ce.event_count > 0) AS successes,\n    \
            sum(ce.value_sum) AS value_sum,\n    \
            sum(ce.value_sq_sum) AS value_sq_sum\n\
        FROM ctx_events AS ce\n\
        GROUP BY ce.a_variant_key, ce.b_variant_key"
    );

    Ok(BuiltQuery { sql, binds })
}

/// Build a `Float64`-valued per-event value expression for the metric.
///
/// Mirrors [`super::aggregation`]'s `numeric_value_expr` routing so a
/// continuous interaction's per-cell `sum`/`sum_sq` match the per-experiment
/// aggregation semantics. Conversion metrics ignore the value (they only check
/// `event_count > 0`), but the expression is always emitted so the SQL shape is
/// uniform.
fn numeric_value_expr(cfg: &AggregationConfig) -> String {
    let use_canonical = matches!(
        cfg.on_field.as_deref(),
        None | Some("") | Some("value") | Some("value_double") | Some("value_int")
    );
    if use_canonical {
        "ifNull(coalesce(e.value_double, CAST(e.value_int AS Nullable(Float64))), 0.0)".to_owned()
    } else {
        let field = cfg.on_field.as_deref().unwrap_or_default();
        format!("ifNull(toFloat64OrNull(e.properties['{field}']), 0.0)")
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";
    const EXP_A: &str = "00000000-0000-0000-0000-0000000000aa";
    const EXP_B: &str = "00000000-0000-0000-0000-0000000000bb";

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

    #[test]
    fn query_self_joins_assignments_final_on_shared_context() {
        let q =
            build_interaction_metric_cells_query(&count_cfg(), ENV_ID, EXP_A, EXP_B, "user", end())
                .unwrap();
        assert!(
            q.sql.contains("FROM experiment_assignments AS a FINAL"),
            "{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("INNER JOIN experiment_assignments AS b FINAL"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("AND a.context_key = b.context_key"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn query_joins_events_via_array_join_not_arrayexists() {
        let q =
            build_interaction_metric_cells_query(&count_cfg(), ENV_ID, EXP_A, EXP_B, "user", end())
                .unwrap();
        assert!(
            q.sql.contains("ARRAY JOIN e.contexts AS ctx_pair"),
            "{}",
            q.sql
        );
        assert!(!q.sql.contains("arrayExists"), "{}", q.sql);
    }

    #[test]
    fn query_emits_both_conversion_and_continuous_columns() {
        let q =
            build_interaction_metric_cells_query(&count_cfg(), ENV_ID, EXP_A, EXP_B, "user", end())
                .unwrap();
        assert!(q.sql.contains("count() AS n"), "{}", q.sql);
        assert!(
            q.sql.contains("countIf(ce.event_count > 0) AS successes"),
            "{}",
            q.sql
        );
        assert!(q.sql.contains("AS value_sum"), "{}", q.sql);
        assert!(q.sql.contains("AS value_sq_sum"), "{}", q.sql);
        assert!(
            q.sql
                .contains("GROUP BY ce.a_variant_key, ce.b_variant_key"),
            "{}",
            q.sql
        );
    }

    #[test]
    fn query_left_joins_so_zero_event_contexts_still_count_in_n() {
        let q =
            build_interaction_metric_cells_query(&count_cfg(), ENV_ID, EXP_A, EXP_B, "user", end())
                .unwrap();
        assert!(
            q.sql.contains("LEFT JOIN matched_events AS me"),
            "{}",
            q.sql
        );
        assert!(
            q.sql.contains("ON me.context_key = s.context_key"),
            "{}",
            q.sql
        );
    }

    /// HIGH-2 regression: the metric must be strict first-exposure (ITT). The
    /// `shared` CTE carries `greatest(a.assigned_at, b.assigned_at)` as the
    /// per-context joint-exposure timestamp, and the event join is bounded below
    /// by it — so pre-exposure events cannot be counted.
    #[test]
    fn query_bounds_events_below_by_joint_exposure_itt() {
        let q =
            build_interaction_metric_cells_query(&count_cfg(), ENV_ID, EXP_A, EXP_B, "user", end())
                .unwrap();
        // Joint-exposure timestamp carried out of `shared`.
        assert!(
            q.sql
                .contains("greatest(a.assigned_at, b.assigned_at) AS joint_assigned_at"),
            "{}",
            q.sql
        );
        // Per-context ITT lower bound on the event join.
        assert!(
            q.sql.contains("me.occurred_at >= s.joint_assigned_at"),
            "{}",
            q.sql
        );
        // Upper bound still present (overlap-window end).
        assert!(
            q.sql.contains("e.occurred_at < fromUnixTimestamp64Milli("),
            "{}",
            q.sql
        );
    }

    #[test]
    fn query_excludes_empty_variant_keys() {
        let q =
            build_interaction_metric_cells_query(&count_cfg(), ENV_ID, EXP_A, EXP_B, "user", end())
                .unwrap();
        assert!(q.sql.contains("a.variant_key != ''"), "{}", q.sql);
        assert!(q.sql.contains("b.variant_key != ''"), "{}", q.sql);
    }

    #[test]
    fn bind_order_is_left_to_right() {
        let q =
            build_interaction_metric_cells_query(&count_cfg(), ENV_ID, EXP_A, EXP_B, "user", end())
                .unwrap();
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str(EXP_A.into()));
        assert_eq!(q.binds[2], QueryBind::Str("user".into()));
        assert_eq!(q.binds[3], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds[4], QueryBind::Str(EXP_B.into()));
        assert_eq!(q.binds[5], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds[6], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[7], QueryBind::Str("checkout_completed".into()));
        assert_eq!(q.binds[8], QueryBind::Str("user".into()));
        assert_eq!(q.binds[9], QueryBind::I64(end().timestamp_millis()));
        assert_eq!(q.binds.len(), 10);
    }

    #[test]
    fn continuous_metric_uses_property_value_expr() {
        let q =
            build_interaction_metric_cells_query(&sum_cfg(), ENV_ID, EXP_A, EXP_B, "user", end())
                .unwrap();
        assert!(
            q.sql.contains("toFloat64OrNull(e.properties['revenue'])"),
            "{}",
            q.sql
        );
        assert_eq!(q.binds[7], QueryBind::Str("purchase".into()));
    }

    #[test]
    fn canonical_value_metric_uses_value_columns() {
        let cfg = AggregationConfig {
            event_key: "latency".into(),
            aggregator: AggregationOperator::Avg,
            on_field: Some("value".into()),
            where_clause: None,
        };
        let q = build_interaction_metric_cells_query(&cfg, ENV_ID, EXP_A, EXP_B, "user", end())
            .unwrap();
        assert!(q.sql.contains("coalesce(e.value_double"), "{}", q.sql);
        assert!(!q.sql.contains("properties["), "{}", q.sql);
    }

    #[test]
    fn rejects_self_interaction() {
        let err =
            build_interaction_metric_cells_query(&count_cfg(), ENV_ID, EXP_A, EXP_A, "user", end())
                .unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn context_type_is_bound_not_inlined() {
        let q =
            build_interaction_metric_cells_query(&count_cfg(), ENV_ID, EXP_A, EXP_B, "user", end())
                .unwrap();
        assert!(!q.sql.contains("'user'"), "{}", q.sql);
    }
}
