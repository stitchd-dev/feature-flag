//! Funnel query builder.
//!
//! Produces a ClickHouse query that evaluates a [`FunnelConfig`] via
//! `windowFunnel(window_seconds[, mode='strict_order'])` and reports
//! the per-step conversion rate as
//! `countIf(level >= N) / countIf(level >= 1)`.
//!
//! The query shape is:
//!
//! ```sql
//! WITH levels AS (
//!     SELECT
//!         variant_key,
//!         windowFunnel(<window>[, 'strict_order'])(
//!             timestamp, metric_key = <step_0>, metric_key = <step_1>, …
//!         ) AS level
//!     FROM events_v2
//!     WHERE …
//!     GROUP BY context_key, variant_key
//! )
//! SELECT
//!     variant_key,
//!     <N> AS step_index,
//!     '<step_event_key>' AS event_key,
//!     countIf(level >= N) AS step_count,
//!     countIf(level >= 1) AS step_total,
//!     countIf(level >= N) / nullIf(countIf(level >= 1), 0) AS conversion_rate
//! FROM levels
//! GROUP BY variant_key
//! ```
//!
//! The outer SELECT is repeated once per step via `UNION ALL` so the
//! caller gets one row per `(variant_key, step_index)`. Step index 0 is
//! always emitted with `step_count == step_total` and a `conversion_rate`
//! of `1.0` (because every context that triggered the funnel reached at
//! least step 0). The mode `strict_order` is enabled by default — set
//! `count_repeats: true` on the config to disable it.

use chrono::{DateTime, Utc};
use stitchd_core::metric::FunnelConfig;

use super::{BuiltQuery, QueryBind, QueryBuildError, push_bind};

/// Build a funnel query from the supplied config + iteration context.
///
/// # Errors
/// Returns [`QueryBuildError::InvalidConfig`] when `steps.len() < 2`,
/// `window_seconds <= 0`, or `variant_keys` is empty.
pub fn build_funnel_query(
    cfg: &FunnelConfig,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    variant_keys: &[&str],
    _iteration_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    // Phase 5 Task 5.3 (subsequent commit) cuts this body over to the
    // `experiment_assignments` JOIN model + uses `_iteration_end`. The
    // signature lands first so the dispatcher and ratio builder compile.
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
    if variant_keys.is_empty() {
        return Err(QueryBuildError::InvalidConfig(
            "variant_keys must not be empty".into(),
        ));
    }

    let mut binds = Vec::new();

    let env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let exp_ph = push_bind(&mut binds, QueryBind::Str(experiment_id.to_owned()));
    let iter_ph = push_bind(&mut binds, QueryBind::Str(iteration_id.to_owned()));

    let mut variant_phs = Vec::with_capacity(variant_keys.len());
    for vk in variant_keys {
        variant_phs.push(push_bind(&mut binds, QueryBind::Str((*vk).to_owned())));
    }
    let variant_in_list = variant_phs.join(", ");

    // One bind per step's event_key, then one bind per step's event_key for
    // the metric_key IN (...) filter.
    let mut step_event_phs = Vec::with_capacity(cfg.steps.len());
    for step in &cfg.steps {
        step_event_phs.push(push_bind(
            &mut binds,
            QueryBind::Str(step.event_key.clone()),
        ));
    }

    // Build `metric_key = {p_step_0} OR metric_key = {p_step_1} OR ...` —
    // we use OR rather than IN to keep parity with windowFunnel's per-step
    // predicates (which use `=`).
    let metric_key_filter = step_event_phs
        .iter()
        .map(|ph| format!("metric_key = {ph}"))
        .collect::<Vec<_>>()
        .join(" OR ");

    // windowFunnel call. Modes are passed as positional args after the
    // window: `windowFunnel(window[, 'strict_order'])(...)`. Spec says
    // strict_order is the default and `count_repeats: true` disables it.
    let mode_arg = if cfg.count_repeats {
        String::new()
    } else {
        ", 'strict_order'".to_owned()
    };

    let step_predicates = step_event_phs
        .iter()
        .map(|ph| format!("metric_key = {ph}"))
        .collect::<Vec<_>>()
        .join(", ");

    let window = cfg.window_seconds;

    // The CTE produces one (context_key, variant_key, level) row per
    // distinct context that entered the funnel. We then group by
    // variant_key in the outer SELECT to produce per-variant counts.
    let levels_cte = format!(
        "WITH levels AS (\n    \
            SELECT\n        \
                arrayFirst(t -> t.1 = 'variant', contexts).2 AS variant_key,\n        \
                arrayFirst(t -> t.2 != '', contexts).2 AS context_key,\n        \
                windowFunnel({window}{mode_arg})(\n            \
                    toUInt32(toUnixTimestamp(timestamp)),\n            \
                    {step_predicates}\n        \
                ) AS level\n    \
            FROM events_v2\n    \
            WHERE env_id = toUUID({env_ph})\n      \
                AND arrayExists(t -> t.1 = 'experiment' AND t.2 = {exp_ph}, contexts)\n      \
                AND arrayExists(t -> t.1 = 'iteration'  AND t.2 = {iter_ph}, contexts)\n      \
                AND ({metric_key_filter})\n      \
                AND arrayFirst(t -> t.1 = 'variant', contexts).2 IN ({variant_in_list})\n    \
            GROUP BY context_key, variant_key\n    \
            HAVING variant_key != '' AND context_key != ''\n\
        )"
    );

    // Per-step outer SELECTs joined with UNION ALL.
    let mut step_selects = Vec::with_capacity(cfg.steps.len());
    for (idx, step) in cfg.steps.iter().enumerate() {
        // step_index >= 0 — the step's lift over step 0 is
        // countIf(level >= idx+1) / countIf(level >= 1).
        //
        // For idx == 0 we emit conversion_rate = 1.0 (every funnel entry
        // reached step 0 by definition) so consumers don't need to special
        // case it.
        let step_index = idx;
        let level_threshold = (idx as i64) + 1;
        let event_key_escaped = escape_sql_literal(&step.event_key);

        let conversion_rate_expr = if step_index == 0 {
            "1.0".to_owned()
        } else {
            format!("countIf(level >= {level_threshold}) / nullIf(countIf(level >= 1), 0)")
        };

        let step_count_expr = if step_index == 0 {
            "countIf(level >= 1)".to_owned()
        } else {
            format!("countIf(level >= {level_threshold})")
        };

        step_selects.push(format!(
            "SELECT\n    \
                variant_key,\n    \
                CAST({step_index} AS UInt32) AS step_index,\n    \
                '{event_key_escaped}' AS event_key,\n    \
                {step_count_expr} AS step_count,\n    \
                countIf(level >= 1) AS step_total,\n    \
                {conversion_rate_expr} AS conversion_rate\n\
            FROM levels\n\
            GROUP BY variant_key\n\
            HAVING variant_key != ''"
        ));
    }
    let union_body = step_selects.join("\nUNION ALL\n");

    let sql = format!("{levels_cte}\n{union_body}");

    Ok(BuiltQuery { sql, binds })
}

/// Escape a string literal for safe inline embedding in SQL.
///
/// Event keys are validated server-side to be slug-shaped
/// (alphanumeric + `_` + `-`), so single-quote escaping is enough.
fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use stitchd_core::metric::{FunnelConfig, FunnelStep};

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";
    const EXP_ID: &str = "00000000-0000-0000-0000-000000000002";
    const ITER_ID: &str = "00000000-0000-0000-0000-000000000003";

    fn variants() -> Vec<&'static str> {
        vec!["control", "treatment"]
    }

    fn step(key: &str) -> FunnelStep {
        FunnelStep {
            event_key: key.into(),
            where_clause: None,
        }
    }

    #[test]
    fn funnel_two_steps_strict_order() {
        let cfg = FunnelConfig {
            steps: vec![step("page_view"), step("checkout_completed")],
            window_seconds: 3600,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap();

        // windowFunnel call includes the window and strict_order mode.
        assert!(
            q.sql.contains("windowFunnel(3600, 'strict_order')"),
            "expected strict_order mode, got:\n{}",
            q.sql
        );
        // Step predicates are bound as separate placeholders.
        assert_eq!(
            q.binds
                .iter()
                .filter(|b| matches!(b, QueryBind::Str(s) if s == "page_view"))
                .count(),
            1
        );
        assert_eq!(
            q.binds
                .iter()
                .filter(|b| matches!(b, QueryBind::Str(s) if s == "checkout_completed"))
                .count(),
            1
        );
    }

    #[test]
    fn funnel_three_steps_count_repeats() {
        let cfg = FunnelConfig {
            steps: vec![
                step("view"),
                step("add_to_cart"),
                step("checkout_completed"),
            ],
            window_seconds: 7200,
            count_repeats: true,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap();

        // count_repeats: true → no strict_order mode argument.
        assert!(
            q.sql.contains("windowFunnel(7200)("),
            "count_repeats=true should omit the strict_order mode, got:\n{}",
            q.sql
        );
        assert!(
            !q.sql.contains("strict_order"),
            "strict_order should NOT appear when count_repeats=true"
        );
    }

    #[test]
    fn funnel_conversion_rate_per_step() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b"), step("c")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap();

        // Step 0: conversion_rate is 1.0 (everyone in the funnel reached
        // step 0 by definition).
        assert!(
            q.sql.contains("'a' AS event_key"),
            "step 0 event_key literal missing, got:\n{}",
            q.sql
        );
        // Step 1: countIf(level >= 2) / nullIf(countIf(level >= 1), 0)
        assert!(
            q.sql
                .contains("countIf(level >= 2) / nullIf(countIf(level >= 1), 0)"),
            "step 1 conversion-rate expr missing, got:\n{}",
            q.sql
        );
        // Step 2: countIf(level >= 3) / nullIf(countIf(level >= 1), 0)
        assert!(
            q.sql
                .contains("countIf(level >= 3) / nullIf(countIf(level >= 1), 0)"),
            "step 2 conversion-rate expr missing, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn funnel_emits_one_select_per_step() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b"), step("c"), step("d")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap();
        // 4 steps → 3 UNION ALL separators.
        let unions = q.sql.matches("UNION ALL").count();
        assert_eq!(
            unions, 3,
            "expected 3 UNION ALL, got {unions} in:\n{}",
            q.sql
        );
    }

    #[test]
    fn funnel_step_index_literal_in_select() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b"), step("c")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap();
        // Each step's outer SELECT must emit its index as a literal.
        assert!(q.sql.contains("CAST(0 AS UInt32) AS step_index"));
        assert!(q.sql.contains("CAST(1 AS UInt32) AS step_index"));
        assert!(q.sql.contains("CAST(2 AS UInt32) AS step_index"));
    }

    #[test]
    fn funnel_fewer_than_two_steps_rejected() {
        let cfg = FunnelConfig {
            steps: vec![step("only")],
            window_seconds: 60,
            count_repeats: false,
        };
        let err = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn funnel_non_positive_window_rejected() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 0,
            count_repeats: false,
        };
        let err = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn funnel_empty_variants_rejected() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 60,
            count_repeats: false,
        };
        let err = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &[], Utc::now()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn funnel_reads_from_events_v2() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap();
        assert!(q.sql.contains("FROM events_v2"));
    }

    #[test]
    fn funnel_groups_by_context_then_variant() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap();
        // The inner CTE groups per (context, variant) so windowFunnel runs
        // over each context's timeline; the outer SELECT then aggregates
        // across contexts per variant.
        assert!(q.sql.contains("GROUP BY context_key, variant_key"));
        // Outer SELECT groups by variant_key only.
        assert!(q.sql.contains("GROUP BY variant_key"));
    }

    #[test]
    fn funnel_emits_step_event_key_literal() {
        let cfg = FunnelConfig {
            steps: vec![step("page_view"), step("checkout_completed")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap();
        assert!(q.sql.contains("'page_view' AS event_key"));
        assert!(q.sql.contains("'checkout_completed' AS event_key"));
    }

    #[test]
    fn funnel_iteration_filter_present() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), Utc::now()).unwrap();
        assert!(q.sql.contains("t.1 = 'iteration'"));
    }

    // ── escape_sql_literal helper ─────────────────────────────────────────────

    #[test]
    fn escape_sql_literal_doubles_single_quotes() {
        assert_eq!(escape_sql_literal("o'brien"), "o''brien");
        assert_eq!(escape_sql_literal("plain"), "plain");
    }
}
