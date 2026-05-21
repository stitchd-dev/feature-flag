//! Ratio query builder.
//!
//! Produces a `WITH numerator AS (...), denominator AS (...) SELECT ...`
//! query that joins the two CTEs per `variant_key`, computes
//! `metric_value = num / den`, and filters rows where `den_count <
//! min_denominator` out via `HAVING`.
//!
//! The caller (Phase 4 Task 2 — `compute_experiment` dispatch) is
//! responsible for resolving the numerator and denominator
//! [`MetricDefinition`]s into their underlying [`AggregationConfig`]s
//! and passing them into this builder. The builder itself does not
//! touch the metric repo.
//!
//! [`MetricDefinition`]: stitchd_core::metric::MetricDefinition

use chrono::{DateTime, Utc};
use stitchd_core::metric::{AggregationConfig, RatioConfig};

use super::{
    BuiltQuery, QueryBind, QueryBuildError, aggregation::build_aggregation_query, push_bind,
};

/// Build a ratio query from the two resolved aggregation legs.
///
/// `ratio_cfg.min_denominator` becomes a `HAVING den_count >= ?` filter
/// — rows with too few denominator events are dropped so the dispatcher
/// can surface "insufficient data" rather than a noisy ratio.
///
/// # Errors
/// Propagates any [`QueryBuildError`] from the two underlying aggregation
/// builds, plus a fresh [`QueryBuildError::InvalidConfig`] when
/// `min_denominator` is negative.
pub fn build_ratio_query(
    ratio_cfg: &RatioConfig,
    numerator_cfg: &AggregationConfig,
    denominator_cfg: &AggregationConfig,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    variant_keys: &[&str],
    iteration_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    if ratio_cfg.min_denominator < 0 {
        return Err(QueryBuildError::InvalidConfig(format!(
            "ratio min_denominator must be non-negative (got {})",
            ratio_cfg.min_denominator
        )));
    }

    // Build the two legs independently so each gets its own placeholder
    // range; we then renumber the denominator's placeholders to continue
    // after the numerator's, and append a final `min_denominator` bind.
    let num_q = build_aggregation_query(
        numerator_cfg,
        experiment_id,
        iteration_id,
        env_id,
        variant_keys,
        iteration_end,
    )?;
    let den_q = build_aggregation_query(
        denominator_cfg,
        experiment_id,
        iteration_id,
        env_id,
        variant_keys,
        iteration_end,
    )?;

    // Numerator binds keep indices [0, num_count); denominator starts at
    // num_count; we rewrite den_q.sql so every `{pN}` becomes `{p(N+offset)}`.
    let offset = num_q.binds.len();
    let den_sql_shifted = shift_placeholders(&den_q.sql, offset);

    // Inner CTEs reuse the per-leg SELECT (variant_key + metric_value) —
    // we relabel `metric_value` to `num_value` / `den_value` and add a
    // `_count` column so we can apply the min_denominator filter.
    //
    // The simplest way to add a count without rebuilding the leg query
    // from scratch is to wrap the leg as a subquery and re-aggregate.
    // But here we already have GROUP BY variant_key with one row per
    // variant, so the row IS the per-variant metric — we don't need an
    // independent count. For the min_denominator filter, however, we
    // need an event-count from the denominator. To avoid leaking the
    // builder's internal structure, we compute the denominator count
    // inline as a second metric in the same CTE.

    // Approach: build a separate denominator-count CTE using the same
    // filters as the denominator metric but with `count()` as the
    // aggregator. We do this by constructing a Count-flavoured config
    // that mirrors the denominator's `event_key` + `where_clause`.
    let denom_count_cfg = AggregationConfig {
        event_key: denominator_cfg.event_key.clone(),
        aggregator: stitchd_core::metric::AggregationOperator::Count,
        on_field: None,
        where_clause: denominator_cfg.where_clause.clone(),
    };
    let den_count_q = build_aggregation_query(
        &denom_count_cfg,
        experiment_id,
        iteration_id,
        env_id,
        variant_keys,
        iteration_end,
    )?;
    let den_count_offset = offset + den_q.binds.len();
    let den_count_sql_shifted = shift_placeholders(&den_count_q.sql, den_count_offset);

    // Combine binds in CTE-emission order.
    let mut binds =
        Vec::with_capacity(num_q.binds.len() + den_q.binds.len() + den_count_q.binds.len() + 1);
    binds.extend(num_q.binds);
    binds.extend(den_q.binds);
    binds.extend(den_count_q.binds);
    let min_den_ph = push_bind(&mut binds, QueryBind::I64(ratio_cfg.min_denominator));

    // Assemble final SQL with three CTEs + ratio computation.
    let sql = format!(
        "WITH numerator AS (\n\
        {num_sql}\n\
        ), denominator AS (\n\
        {den_sql}\n\
        ), denominator_count AS (\n\
        {den_count_sql}\n\
        )\n\
        SELECT\n    \
            numerator.variant_key AS variant_key,\n    \
            numerator.metric_value AS num_value,\n    \
            denominator.metric_value AS den_value,\n    \
            denominator_count.metric_value AS den_count,\n    \
            numerator.metric_value / nullIf(denominator.metric_value, 0) AS metric_value\n\
        FROM numerator\n\
        INNER JOIN denominator ON numerator.variant_key = denominator.variant_key\n\
        INNER JOIN denominator_count ON numerator.variant_key = denominator_count.variant_key\n\
        WHERE den_count >= {min_den_ph}",
        num_sql = indent_lines(&num_q.sql),
        den_sql = indent_lines(&den_sql_shifted),
        den_count_sql = indent_lines(&den_count_sql_shifted),
    );

    Ok(BuiltQuery { sql, binds })
}

// ── Placeholder rewriting ────────────────────────────────────────────────────

/// Rewrite every `{pN}` placeholder in `sql` by adding `offset` to `N`.
///
/// Used to merge per-leg SQL fragments into a single query whose bind
/// list is the concatenation of the legs' bind lists.
fn shift_placeholders(sql: &str, offset: usize) -> String {
    if offset == 0 {
        return sql.to_owned();
    }
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(start) = rest.find("{p") {
        out.push_str(&rest[..start]);
        // Find the closing '}' — placeholders are always `{p<digits>}`.
        let after = &rest[start + 2..];
        let end_rel = after.find('}').unwrap_or(after.len());
        let n: usize = after[..end_rel].parse().unwrap_or(0);
        out.push_str(&format!("{{p{}}}", n + offset));
        rest = &after[end_rel + 1..];
    }
    out.push_str(rest);
    out
}

/// Indent every line in `sql` by two spaces for CTE readability.
fn indent_lines(sql: &str) -> String {
    sql.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use stitchd_core::id::MetricId;
    use stitchd_core::metric::{AggregationConfig, AggregationOperator};

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";
    const EXP_ID: &str = "00000000-0000-0000-0000-000000000002";
    const ITER_ID: &str = "00000000-0000-0000-0000-000000000003";

    fn variants() -> Vec<&'static str> {
        vec!["control", "treatment"]
    }

    fn count_cfg(event_key: &str) -> AggregationConfig {
        AggregationConfig {
            event_key: event_key.into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        }
    }

    fn ratio_cfg(min_den: i64) -> RatioConfig {
        RatioConfig {
            numerator_metric_id: MetricId::new(),
            denominator_metric_id: MetricId::new(),
            min_denominator: min_den,
        }
    }

    fn iter_end() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 21, 0, 0, 0).unwrap()
    }

    #[test]
    fn ratio_two_aggregations_emits_three_ctes() {
        let q = build_ratio_query(
            &ratio_cfg(50),
            &count_cfg("checkout_completed"),
            &count_cfg("checkout_started"),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            &variants(),
            iter_end(),
        )
        .unwrap();
        assert!(q.sql.contains("WITH numerator AS"));
        assert!(q.sql.contains("), denominator AS ("));
        assert!(q.sql.contains("), denominator_count AS ("));
        assert!(q.sql.contains("INNER JOIN denominator"));
        assert!(q.sql.contains("INNER JOIN denominator_count"));
    }

    #[test]
    fn ratio_computes_division_with_nullif_guard() {
        let q = build_ratio_query(
            &ratio_cfg(0),
            &count_cfg("converted"),
            &count_cfg("started"),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            &variants(),
            iter_end(),
        )
        .unwrap();
        assert!(
            q.sql.contains(
                "numerator.metric_value / nullIf(denominator.metric_value, 0) AS metric_value"
            ),
            "ratio query must guard against div-by-zero with nullIf, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn ratio_min_denominator_filter_present() {
        let q = build_ratio_query(
            &ratio_cfg(100),
            &count_cfg("a"),
            &count_cfg("b"),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            &variants(),
            iter_end(),
        )
        .unwrap();

        // The min_denominator bind is the last entry; the WHERE clause uses
        // it as `den_count >= {p<last>}`.
        let last_idx = q.binds.len() - 1;
        let expected_clause = format!("WHERE den_count >= {{p{last_idx}}}");
        assert!(
            q.sql.contains(&expected_clause),
            "expected `{expected_clause}` in SQL, got:\n{}",
            q.sql
        );
        assert_eq!(*q.binds.last().unwrap(), QueryBind::I64(100));
    }

    #[test]
    fn ratio_propagates_numerator_where_clause_binds() {
        let num_cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: Some("revenue".into()),
            where_clause: Some(json!({"==": [{"var": "currency"}, "USD"]})),
        };
        let q = build_ratio_query(
            &ratio_cfg(10),
            &num_cfg,
            &count_cfg("checkout_started"),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            &variants(),
            iter_end(),
        )
        .unwrap();

        // USD literal should appear as a bind in the numerator CTE.
        assert!(
            q.binds.iter().any(|b| b == &QueryBind::Str("USD".into())),
            "USD bind missing from {:?}",
            q.binds
        );
    }

    #[test]
    fn ratio_denominator_placeholders_shifted() {
        // Verify that the denominator CTE's placeholders are renumbered
        // to follow the numerator's. With two simple Count configs over
        // 2 variants, the numerator emits binds [0..6] and the
        // denominator should start at {p6}.
        let q = build_ratio_query(
            &ratio_cfg(0),
            &count_cfg("a"),
            &count_cfg("b"),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            &variants(),
            iter_end(),
        )
        .unwrap();

        // Numerator emits binds 0..=6 → env, exp, iter, event_a, iter_end, ctrl, treat
        // Denominator emits binds 7..=13 → env, exp, iter, event_b, iter_end, ctrl, treat
        // Denominator_count reuses the same Count config → binds 14..=20
        // min_denominator → bind 21
        assert_eq!(q.binds.len(), 22);

        // Spot-check: the denominator CTE must reference {p7} (the env_id
        // of the denominator leg, since each leg starts with env_id).
        let den_cte_start = q.sql.find("), denominator AS (").unwrap();
        let den_cte_end = q.sql.find("), denominator_count AS (").unwrap();
        let den_cte = &q.sql[den_cte_start..den_cte_end];
        assert!(
            den_cte.contains("{p7}"),
            "denominator CTE must use shifted placeholders, got:\n{den_cte}"
        );
    }

    #[test]
    fn ratio_negative_min_denominator_returns_error() {
        let err = build_ratio_query(
            &ratio_cfg(-1),
            &count_cfg("a"),
            &count_cfg("b"),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            &variants(),
            iter_end(),
        )
        .unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn ratio_propagates_unsupported_jsonlogic_error() {
        let bad_num = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(json!({"%": [{"var": "a"}, 2]})),
        };
        let err = build_ratio_query(
            &ratio_cfg(0),
            &bad_num,
            &count_cfg("y"),
            EXP_ID,
            ITER_ID,
            ENV_ID,
            &variants(),
            iter_end(),
        )
        .unwrap_err();
        assert_eq!(err, QueryBuildError::UnsupportedJsonLogic("%".into()));
    }

    // ── shift_placeholders helper ─────────────────────────────────────────────

    #[test]
    fn shift_placeholders_zero_offset_is_identity() {
        let sql = "SELECT {p0} + {p1}";
        assert_eq!(shift_placeholders(sql, 0), sql);
    }

    #[test]
    fn shift_placeholders_renumbers_all_placeholders() {
        let sql = "SELECT {p0} + {p1} + {p2}";
        assert_eq!(shift_placeholders(sql, 5), "SELECT {p5} + {p6} + {p7}");
    }

    #[test]
    fn shift_placeholders_handles_no_placeholders() {
        let sql = "SELECT 1 + 2";
        assert_eq!(shift_placeholders(sql, 10), sql);
    }

    #[test]
    fn shift_placeholders_preserves_surrounding_text() {
        let sql = "a {p0} b {p1} c";
        assert_eq!(shift_placeholders(sql, 1), "a {p1} b {p2} c");
    }
}
