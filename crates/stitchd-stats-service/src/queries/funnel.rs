//! Funnel query builder.
//!
//! Produces a ClickHouse query that evaluates a [`FunnelConfig`] via
//! `windowFunnel(window_seconds[, mode='strict_order'])` over events
//! attributed to assigned contexts in `experiment_assignments`.
//!
//! ## Shape
//!
//! ```sql
//! WITH levels AS (
//!     SELECT
//!         a.context_type,
//!         a.context_key,
//!         a.variant_key,
//!         windowFunnel(<window>[, 'strict_order'])(
//!             toUInt32(toUnixTimestamp(e.occurred_at)),
//!             e.metric_key = <step_0>, …, e.metric_key = <step_N-1>
//!         ) AS level
//!     FROM events e
//!     ARRAY JOIN e.contexts AS ctx_pair
//!     INNER JOIN experiment_assignments a
//!         ON e.env_id = a.env_id
//!        AND ctx_pair.1 = a.context_type
//!        AND ctx_pair.2 = a.context_key
//!     WHERE a.env_id = toUUID(?)
//!       AND a.experiment_id = toUUID(?)
//!       AND a.iteration_id  = toUUID(?)
//!       AND e.occurred_at  >= a.assigned_at
//!       AND e.occurred_at  <  fromUnixTimestamp64Milli(?)
//!       AND (e.metric_key = ? OR e.metric_key = ? OR …)
//!       AND a.variant_key IN (?, ?, …)
//!     GROUP BY a.context_type, a.context_key, a.variant_key
//!     HAVING a.variant_key != '' AND a.context_key != ''
//! )
//! SELECT
//!     context_type,
//!     variant_key,
//!     <step_index> AS step_index,
//!     '<step_event_key>' AS event_key,
//!     countIf(level >= step_index+1)  AS step_count,
//!     countIf(level >= 1)             AS step_total,
//!     <conversion_rate_expr>          AS conversion_rate
//! FROM levels
//! GROUP BY context_type, variant_key
//! HAVING variant_key != ''
//! UNION ALL
//! ... (one SELECT per step)
//! ```
//!
//! The outer SELECT is repeated once per step via `UNION ALL` so the
//! caller gets one row per `(context_type, variant_key, step_index)`.
//! Step index 0's `conversion_rate` is hard-coded to `1.0` because every
//! context that triggered the funnel reached at least step 0.
//! `strict_order` mode is enabled by default; setting `count_repeats:
//! true` on the config disables it.
//!
//! ## Bind-order discipline
//!
//! `clickhouse-rs` binds `?` placeholders by SQL position, NOT by the
//! parallel `Vec<QueryBind>` index. We push binds in left-to-right SQL
//! appearance order:
//!
//!   1. windowFunnel step predicates (`e.metric_key = ?`, …) — N binds
//!      in step order (these appear FIRST in the SELECT list of the CTE)
//!   2. `env_id`         (`a.env_id = toUUID(?)`)
//!   3. `experiment_id`  (`a.experiment_id = toUUID(?)`)
//!   4. `iteration_id`   (`a.iteration_id = toUUID(?)`)
//!   5. `iteration_end`  (`e.occurred_at < fromUnixTimestamp64Milli(?)`)
//!   6. metric_key OR filter — N binds (one per step), same event_keys as
//!      the predicates, used to prune events to the funnel's step set
//!   7. variant_keys     (`a.variant_key IN (?, …)`)
//!
//! Critically: the step predicate binds appear in the SELECT list, which
//! is rendered BEFORE the WHERE clause in the final SQL string — that's
//! why they sit at the head of the bind vec.

use chrono::{DateTime, Utc};
use stitchd_core::metric::FunnelConfig;

use super::{BuiltQuery, QueryBind, QueryBuildError, push_bind};

/// Build a funnel query from the supplied config + iteration context.
///
/// `iteration_end` is the upper bound on `e.occurred_at` (caller passes
/// `iteration.ended_at` or `Utc::now()` for in-progress iterations —
/// ClickHouse can't JOIN to PG to resolve this itself).
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
    if variant_keys.is_empty() {
        return Err(QueryBuildError::InvalidConfig(
            "variant_keys must not be empty".into(),
        ));
    }

    let mut binds = Vec::new();

    // ── Push binds in SQL-appearance order ──
    // 1) windowFunnel step predicates first (they appear in the SELECT
    //    list of the inner CTE, which is rendered before the WHERE clause).
    let mut step_predicate_phs = Vec::with_capacity(cfg.steps.len());
    for step in &cfg.steps {
        step_predicate_phs.push(push_bind(
            &mut binds,
            QueryBind::Str(step.event_key.clone()),
        ));
    }
    let step_predicates = step_predicate_phs
        .iter()
        .map(|ph| format!("e.metric_key = {ph}"))
        .collect::<Vec<_>>()
        .join(",\n            ");

    // 2) env / exp / iter / iter_end (in the WHERE clause).
    let env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let exp_ph = push_bind(&mut binds, QueryBind::Str(experiment_id.to_owned()));
    let iter_ph = push_bind(&mut binds, QueryBind::Str(iteration_id.to_owned()));
    let iter_end_ph = push_bind(&mut binds, QueryBind::I64(iteration_end.timestamp_millis()));

    // 3) metric_key filter — N binds (one per step, in step order), each
    //    bound to its event_key. ClickHouse needs one bind per `?`
    //    regardless of duplicate logical values, so we bind the same
    //    event_keys twice (once for the predicates, once for the
    //    metric_key OR filter).
    let mut metric_key_filter_phs = Vec::with_capacity(cfg.steps.len());
    for step in &cfg.steps {
        metric_key_filter_phs.push(push_bind(
            &mut binds,
            QueryBind::Str(step.event_key.clone()),
        ));
    }
    let metric_key_filter = metric_key_filter_phs
        .iter()
        .map(|ph| format!("e.metric_key = {ph}"))
        .collect::<Vec<_>>()
        .join(" OR ");

    // 4) variant_keys IN list.
    let mut variant_phs = Vec::with_capacity(variant_keys.len());
    for vk in variant_keys {
        variant_phs.push(push_bind(&mut binds, QueryBind::Str((*vk).to_owned())));
    }
    let variant_in_list = variant_phs.join(", ");

    // windowFunnel mode arg.
    let mode_arg = if cfg.count_repeats {
        String::new()
    } else {
        ", 'strict_order'".to_owned()
    };

    let window = cfg.window_seconds;

    // The CTE produces one (context_type, context_key, variant_key, level)
    // row per assigned context that entered the funnel within the iteration
    // window. The dedup key is `(context_type, context_key)` — funnel
    // completion is per-assigned-context.
    // ARRAY JOIN flattens each event into one row per (context_type,
    // context_key) tuple in `e.contexts`; the INNER JOIN against
    // `experiment_assignments` is then a plain equi-join on
    // (env_id, context_type, context_key). Mirrors the Phase 11 E2E
    // pattern — see `experiment_lifecycle_e2e.rs` and the rewrite
    // rationale in [`super::aggregation`]. CH 24's new analyzer rejects
    // the legacy `arrayExists(...)` shape inside `JOIN ON`.
    //
    // Bind order is UNCHANGED relative to the legacy shape: ARRAY JOIN
    // and the equi-join predicates introduce no new placeholders, and
    // the SELECT list (whose `windowFunnel(...)` predicates contribute
    // the head-of-vec binds) is untouched.
    let levels_cte = format!(
        "WITH levels AS (\n    \
            SELECT\n        \
                a.context_type AS context_type,\n        \
                a.context_key AS context_key,\n        \
                a.variant_key AS variant_key,\n        \
                windowFunnel({window}{mode_arg})(\n            \
                    toUInt32(toUnixTimestamp(e.occurred_at)),\n            \
                    {step_predicates}\n        \
                ) AS level\n    \
            FROM events AS e\n    \
            ARRAY JOIN e.contexts AS ctx_pair\n    \
            INNER JOIN experiment_assignments AS a\n        \
                ON e.env_id = a.env_id\n       \
                AND ctx_pair.1 = a.context_type\n       \
                AND ctx_pair.2 = a.context_key\n    \
            WHERE a.env_id = toUUID({env_ph})\n      \
              AND a.experiment_id = toUUID({exp_ph})\n      \
              AND a.iteration_id = toUUID({iter_ph})\n      \
              AND e.occurred_at >= a.assigned_at\n      \
              AND e.occurred_at < fromUnixTimestamp64Milli({iter_end_ph})\n      \
              AND ({metric_key_filter})\n      \
              AND a.variant_key IN ({variant_in_list})\n    \
            GROUP BY a.context_type, a.context_key, a.variant_key\n    \
            HAVING a.variant_key != '' AND a.context_key != ''\n\
        )"
    );

    // Per-step outer SELECTs joined with UNION ALL. Each step renders its
    // index + event_key literal inline; only the inner CTE references
    // bound placeholders. This keeps the bind contract simple (binds are
    // ordered by their SINGLE appearance inside the CTE), and the
    // per-step UNION just multiplies the row count.
    let mut step_selects = Vec::with_capacity(cfg.steps.len());
    for (idx, step) in cfg.steps.iter().enumerate() {
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
                context_type,\n    \
                variant_key,\n    \
                CAST({step_index} AS UInt32) AS step_index,\n    \
                '{event_key_escaped}' AS event_key,\n    \
                {step_count_expr} AS step_count,\n    \
                countIf(level >= 1) AS step_total,\n    \
                {conversion_rate_expr} AS conversion_rate\n\
            FROM levels\n\
            GROUP BY context_type, variant_key\n\
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
    use chrono::{TimeZone, Utc};
    use stitchd_core::metric::{FunnelConfig, FunnelStep};

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";
    const EXP_ID: &str = "00000000-0000-0000-0000-000000000002";
    const ITER_ID: &str = "00000000-0000-0000-0000-000000000003";

    fn variants() -> Vec<&'static str> {
        vec!["control", "treatment"]
    }

    fn iter_end() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 21, 0, 0, 0).unwrap()
    }

    fn step(key: &str) -> FunnelStep {
        FunnelStep {
            event_key: key.into(),
            where_clause: None,
        }
    }

    // ── Bind-order discipline — load-bearing for clickhouse-rs ────────────

    #[test]
    fn funnel_binds_are_in_sql_appearance_order_three_step() {
        // Three-step funnel exercises the head-of-vec step predicates +
        // mid-vec scoping binds + tail-of-vec metric_key filter + variants.
        // The push order this test pins:
        //   p0 = step1 predicate
        //   p1 = step2 predicate
        //   p2 = step3 predicate
        //   p3 = env_id
        //   p4 = experiment_id
        //   p5 = iteration_id
        //   p6 = iteration_end_ms
        //   p7 = step1 metric_key filter
        //   p8 = step2 metric_key filter
        //   p9 = step3 metric_key filter
        //   p10 = variant ctrl
        //   p11 = variant treat
        let cfg = FunnelConfig {
            steps: vec![
                step("view"),
                step("add_to_cart"),
                step("checkout_completed"),
            ],
            window_seconds: 3600,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();
        assert_eq!(q.binds[0], QueryBind::Str("view".into()));
        assert_eq!(q.binds[1], QueryBind::Str("add_to_cart".into()));
        assert_eq!(q.binds[2], QueryBind::Str("checkout_completed".into()));
        assert_eq!(q.binds[3], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[4], QueryBind::Str(EXP_ID.into()));
        assert_eq!(q.binds[5], QueryBind::Str(ITER_ID.into()));
        assert_eq!(q.binds[6], QueryBind::I64(iter_end().timestamp_millis()));
        assert_eq!(q.binds[7], QueryBind::Str("view".into()));
        assert_eq!(q.binds[8], QueryBind::Str("add_to_cart".into()));
        assert_eq!(q.binds[9], QueryBind::Str("checkout_completed".into()));
        assert_eq!(q.binds[10], QueryBind::Str("control".into()));
        assert_eq!(q.binds[11], QueryBind::Str("treatment".into()));
        assert_eq!(q.binds.len(), 12);

        // Cross-check: the SQL must reference the placeholders in the same
        // numeric order they appear in the bind vec. The simplest invariant
        // is that {p0} appears strictly before {p1}, {p1} before {p2}, …
        // when scanned left-to-right.
        let mut last_idx: i64 = -1;
        let mut cursor = 0;
        while let Some(open) = q.sql[cursor..].find("{p") {
            let abs_open = cursor + open;
            let after_p = abs_open + 2;
            let digit_end = q.sql[after_p..]
                .bytes()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digit_end == 0 {
                cursor = after_p;
                continue;
            }
            let n: i64 = q.sql[after_p..after_p + digit_end].parse().expect("digits");
            assert!(
                n > last_idx,
                "placeholder {{p{n}}} appears after {{p{last_idx}}} in SQL but earlier in bind vec — \
                 clickhouse-rs would bind values to the wrong positions. SQL:\n{}",
                q.sql,
            );
            last_idx = n;
            cursor = after_p + digit_end + 1;
        }
        assert_eq!(last_idx, 11, "every bind must appear in SQL");
    }

    #[test]
    fn funnel_two_steps_strict_order() {
        let cfg = FunnelConfig {
            steps: vec![step("page_view"), step("checkout_completed")],
            window_seconds: 3600,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();

        assert!(
            q.sql.contains("windowFunnel(3600, 'strict_order')"),
            "expected strict_order mode, got:\n{}",
            q.sql
        );
        // Step predicates appear as two separate binds in step order.
        assert_eq!(q.binds[0], QueryBind::Str("page_view".into()));
        assert_eq!(q.binds[1], QueryBind::Str("checkout_completed".into()));
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
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();

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
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();

        // Step 0 emits its own event_key literal.
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
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();
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
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();
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
        let err =
            build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn funnel_non_positive_window_rejected() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 0,
            count_repeats: false,
        };
        let err =
            build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn funnel_empty_variants_rejected() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 60,
            count_repeats: false,
        };
        let err = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &[], iter_end()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn funnel_reads_from_events_joined_against_assignments() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();
        assert!(q.sql.contains("FROM events AS e"));
        assert!(q.sql.contains("ARRAY JOIN e.contexts AS ctx_pair"));
        assert!(q.sql.contains("INNER JOIN experiment_assignments AS a"));
        assert!(q.sql.contains("ctx_pair.1 = a.context_type"));
        assert!(q.sql.contains("ctx_pair.2 = a.context_key"));
        assert!(
            !q.sql.contains("arrayExists"),
            "CH 24 analyzer rejects arrayExists in JOIN ON; got:\n{}",
            q.sql
        );
    }

    #[test]
    fn funnel_groups_by_context_type_context_key_variant_key() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();
        // Inner CTE dedupes per (context_type, context_key, variant_key).
        assert!(
            q.sql
                .contains("GROUP BY a.context_type, a.context_key, a.variant_key")
        );
        // Outer SELECTs group per (context_type, variant_key).
        assert!(q.sql.contains("GROUP BY context_type, variant_key"));
    }

    #[test]
    fn funnel_no_event_side_experiment_context_tags() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();
        assert!(
            !q.sql.contains("'experiment'"),
            "no event-side experiment tag filter"
        );
        assert!(
            !q.sql.contains("'iteration'"),
            "no event-side iteration tag filter"
        );
        assert!(
            !q.sql.contains("'variant'"),
            "no event-side variant tag filter"
        );
    }

    #[test]
    fn funnel_emits_step_event_key_literal() {
        let cfg = FunnelConfig {
            steps: vec![step("page_view"), step("checkout_completed")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();
        assert!(q.sql.contains("'page_view' AS event_key"));
        assert!(q.sql.contains("'checkout_completed' AS event_key"));
    }

    #[test]
    fn funnel_iteration_filter_uses_assignments_alias() {
        let cfg = FunnelConfig {
            steps: vec![step("a"), step("b")],
            window_seconds: 60,
            count_repeats: false,
        };
        let q = build_funnel_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end()).unwrap();
        assert!(
            q.sql.contains("a.iteration_id = toUUID("),
            "iteration scoping must come from `a.iteration_id`, got:\n{}",
            q.sql
        );
    }

    // ── escape_sql_literal helper ─────────────────────────────────────────────

    #[test]
    fn escape_sql_literal_doubles_single_quotes() {
        assert_eq!(escape_sql_literal("o'brien"), "o''brien");
        assert_eq!(escape_sql_literal("plain"), "plain");
    }
}
