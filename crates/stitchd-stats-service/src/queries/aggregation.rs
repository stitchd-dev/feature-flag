//! Aggregation query builder.
//!
//! Produces a `SELECT variant_key, <agg>(...) FROM events_v2 ...` query
//! that:
//!
//! - Filters by `env_id`, `metric_key` (== `event_key`), iteration time
//!   window, and the supplied `variant_keys` allow-list.
//! - Applies the optional `where_clause` (JsonLogic) as an extra
//!   predicate against the `properties Map(String, String)` column.
//! - Reads from `events_v2` rather than `events_experiment_daily`
//!   because the MV only stores generic count/sum/uniq states — it
//!   cannot satisfy percentiles, custom `on_field` references, or
//!   property-level filters.
//!
//! See the parent module ([`super`]) for the [`BuiltQuery`] /
//! [`QueryBind`] / [`QueryBuildError`] types.
//!
//! [`BuiltQuery`]: super::BuiltQuery
//! [`QueryBind`]: super::QueryBind
//! [`QueryBuildError`]: super::QueryBuildError

use stitchd_core::metric::{AggregationConfig, AggregationOperator};

use super::{BuiltQuery, QueryBind, QueryBuildError, jsonlogic_to_sql, push_bind};

// ── Public API ───────────────────────────────────────────────────────────────

/// Build the ClickHouse aggregation query for the given config + scope.
///
/// `variant_keys` is the experiment-iteration variant allow-list — rows
/// whose `variant_key` is outside this set are filtered out at the SQL
/// layer so the downstream stats code never sees noise from out-of-scope
/// variants (e.g. a previously-renamed variant still emitting events).
///
/// # Errors
/// Returns a [`QueryBuildError`] when the config fails a defensive shape
/// check (e.g. `Sum` without `on_field`) or the `where_clause` JsonLogic
/// uses an unsupported operator.
pub fn build_aggregation_query(
    cfg: &AggregationConfig,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    variant_keys: &[&str],
) -> Result<BuiltQuery, QueryBuildError> {
    // Defensive: re-run the kind-specific validator.
    if cfg.aggregator.requires_field() && cfg.on_field.is_none() {
        return Err(QueryBuildError::InvalidConfig(format!(
            "aggregator `{:?}` requires an on_field",
            cfg.aggregator
        )));
    }

    let mut binds = Vec::new();

    // env_id, experiment_id, iteration_id placeholders.
    let env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let exp_ph = push_bind(&mut binds, QueryBind::Str(experiment_id.to_owned()));
    let iter_ph = push_bind(&mut binds, QueryBind::Str(iteration_id.to_owned()));
    let event_ph = push_bind(&mut binds, QueryBind::Str(cfg.event_key.clone()));

    // variant_keys IN (...) — at least one placeholder must be emitted.
    if variant_keys.is_empty() {
        return Err(QueryBuildError::InvalidConfig(
            "variant_keys must not be empty".into(),
        ));
    }
    let mut variant_phs = Vec::with_capacity(variant_keys.len());
    for vk in variant_keys {
        variant_phs.push(push_bind(&mut binds, QueryBind::Str((*vk).to_owned())));
    }
    let variant_in_list = variant_phs.join(", ");

    // Optional where_clause.
    let extra_where = match cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n  AND ({frag})")
        }
        None => String::new(),
    };

    // Aggregator expression.
    let agg_expr = render_aggregator(cfg)?;

    let sql = format!(
        "SELECT\n    \
            arrayFirst(t -> t.1 = 'variant', contexts).2 AS variant_key,\n    \
            {agg_expr} AS metric_value\n\
        FROM events_v2\n\
        WHERE env_id = toUUID({env_ph})\n  \
          AND arrayExists(t -> t.1 = 'experiment' AND t.2 = {exp_ph}, contexts)\n  \
          AND arrayExists(t -> t.1 = 'iteration'  AND t.2 = {iter_ph}, contexts)\n  \
          AND metric_key = {event_ph}\n  \
          AND arrayFirst(t -> t.1 = 'variant', contexts).2 IN ({variant_in_list}){extra_where}\n\
        GROUP BY variant_key\n\
        HAVING variant_key != ''"
    );

    Ok(BuiltQuery { sql, binds })
}

// ── Aggregator → ClickHouse expression ───────────────────────────────────────

/// Render the aggregator expression for the configured operator + field.
///
/// `Count` ignores `on_field` and emits `count()`. Numeric operators
/// (`Sum`, `Avg`, percentiles) coerce the on-field to a `Float64` via
/// `toFloat64OrNull` and wrap with `ifNull(..., 0.0)` for non-nullable
/// aggregation (per the `db_optim_20260516` pattern; see
/// `conductor/patterns.md` — *AggregatingMergeTree insert/read combiners*).
/// `Uniq` runs on the raw field expression because cardinality counting
/// over strings is the common case.
// `pub(super)` so the sibling `preview` module can reuse the exact same
// aggregator-to-CH-expression mapping rather than duplicating it. Keeping
// it private to `queries/` means it isn't part of the crate's public API.
pub(super) fn render_aggregator(cfg: &AggregationConfig) -> Result<String, QueryBuildError> {
    Ok(match cfg.aggregator {
        AggregationOperator::Count => "count()".to_owned(),
        AggregationOperator::Sum => format!("sum({})", numeric_value_expr(cfg)?),
        AggregationOperator::Avg => format!("avg({})", numeric_value_expr(cfg)?),
        AggregationOperator::P50 => percentile(cfg, "0.5")?,
        AggregationOperator::P90 => percentile(cfg, "0.9")?,
        AggregationOperator::P99 => percentile(cfg, "0.99")?,
        AggregationOperator::Uniq => {
            let field = on_field_expr(cfg)?;
            format!("uniq({field})")
        }
    })
}

/// Render a `quantile(<q>)(...)` expression over a coerced-to-Float64
/// value expression. Uses the same numeric coercion as `Sum` / `Avg`
/// (i.e. preserves the canonical numeric column type when on_field
/// is "value", and applies `toFloat64OrNull` only to property-string
/// references).
fn percentile(cfg: &AggregationConfig, q: &str) -> Result<String, QueryBuildError> {
    Ok(format!("quantile({q})({})", numeric_value_expr(cfg)?))
}

/// Build a `Float64`-valued expression for a numeric aggregator's
/// operand. Branches on whether `on_field` references the canonical
/// numeric columns (`value` → already typed) or a `properties[...]`
/// string (needs `toFloat64OrNull`):
///
/// - `on_field == "value"`:
///   `ifNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))), 0.0)`
///   — both source columns are already numeric, so we skip
///   `toFloat64OrNull` (which only accepts `String` and rejects
///   `Float64` at the SQL layer with
///   `ILLEGAL_TYPE_OF_ARGUMENT`).
///
/// - `on_field == "<property>"`:
///   `ifNull(toFloat64OrNull(properties['<name>']), 0.0)`
///   — `properties` is `Map(String, String)`, so `toFloat64OrNull`
///   is the right coercion.
fn numeric_value_expr(cfg: &AggregationConfig) -> Result<String, QueryBuildError> {
    let field = cfg.on_field.as_ref().ok_or_else(|| {
        QueryBuildError::InvalidConfig(format!(
            "aggregator `{:?}` requires an on_field",
            cfg.aggregator
        ))
    })?;
    Ok(if field == "value" {
        // Canonical numeric columns are already Float64-coercible;
        // `coalesce(...)` returns Nullable(Float64). Just wrap in ifNull
        // so the aggregator sees a non-null float.
        "ifNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))), 0.0)".to_owned()
    } else {
        // properties[<name>] is String — toFloat64OrNull is correct.
        format!("ifNull(toFloat64OrNull(properties['{field}']), 0.0)")
    })
}

/// Resolve the `on_field` reference to a ClickHouse expression.
///
/// We support two shorthand cases:
///
/// - `value` → reads from the canonical numeric column (one of
///   `value_double` / `value_int`). We coalesce to `value_double` first
///   then cast `value_int` to `Float64` when only the integer column is
///   populated. This mirrors the MV definition.
/// - any other name → reads from the `properties Map(String, String)`
///   column at `properties['<name>']`. Numeric coercion happens in the
///   caller.
fn on_field_expr(cfg: &AggregationConfig) -> Result<String, QueryBuildError> {
    let field = cfg.on_field.as_ref().ok_or_else(|| {
        QueryBuildError::InvalidConfig(format!(
            "aggregator `{:?}` requires an on_field",
            cfg.aggregator
        ))
    })?;
    if field == "value" {
        Ok("coalesce(value_double, CAST(value_int AS Nullable(Float64)))".to_owned())
    } else {
        Ok(format!("properties['{field}']"))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";
    const EXP_ID: &str = "00000000-0000-0000-0000-000000000002";
    const ITER_ID: &str = "00000000-0000-0000-0000-000000000003";

    fn variants() -> Vec<&'static str> {
        vec!["control", "treatment"]
    }

    // ── Golden-SQL snapshots ─────────────────────────────────────────────────

    #[test]
    fn aggregation_count_simple() {
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap();

        let expected = "SELECT\n    \
            arrayFirst(t -> t.1 = 'variant', contexts).2 AS variant_key,\n    \
            count() AS metric_value\n\
            FROM events_v2\n\
            WHERE env_id = toUUID({p0})\n  \
              AND arrayExists(t -> t.1 = 'experiment' AND t.2 = {p1}, contexts)\n  \
              AND arrayExists(t -> t.1 = 'iteration'  AND t.2 = {p2}, contexts)\n  \
              AND metric_key = {p3}\n  \
              AND arrayFirst(t -> t.1 = 'variant', contexts).2 IN ({p4}, {p5})\n\
            GROUP BY variant_key\n\
            HAVING variant_key != ''";
        assert_eq!(q.sql, expected);
        assert_eq!(
            q.binds,
            vec![
                QueryBind::Str(ENV_ID.into()),
                QueryBind::Str(EXP_ID.into()),
                QueryBind::Str(ITER_ID.into()),
                QueryBind::Str("checkout_completed".into()),
                QueryBind::Str("control".into()),
                QueryBind::Str("treatment".into()),
            ]
        );
    }

    #[test]
    fn aggregation_sum_with_on_field() {
        let cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: Some("revenue".into()),
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap();
        assert!(
            q.sql.contains(
                "sum(ifNull(toFloat64OrNull(properties['revenue']), 0.0)) AS metric_value"
            ),
            "sum aggregator should wrap properties['revenue'] in ifNull+toFloat64OrNull, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn aggregation_avg_on_value_column_uses_native_columns() {
        // on_field == "value" routes to the canonical numeric columns
        // (value_double / value_int) and skips `toFloat64OrNull` — the
        // coalesce already returns Nullable(Float64) and ClickHouse
        // rejects toFloat64OrNull(Float64) with ILLEGAL_TYPE_OF_ARGUMENT.
        let cfg = AggregationConfig {
            event_key: "latency".into(),
            aggregator: AggregationOperator::Avg,
            on_field: Some("value".into()),
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap();
        assert!(
            q.sql.contains(
                "avg(ifNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))), 0.0))"
            ),
            "avg over 'value' should coalesce native numeric columns directly, got:\n{}",
            q.sql
        );
        assert!(
            !q.sql.contains("toFloat64OrNull(coalesce"),
            "toFloat64OrNull must NOT wrap the coalesce(value_*) expression: {}",
            q.sql,
        );
    }

    #[test]
    fn aggregation_percentile_renders_quantile_call() {
        let cfg = AggregationConfig {
            event_key: "latency".into(),
            aggregator: AggregationOperator::P90,
            on_field: Some("latency_ms".into()),
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap();
        assert!(
            q.sql
                .contains("quantile(0.9)(ifNull(toFloat64OrNull(properties['latency_ms']), 0.0))"),
            "P90 should emit quantile(0.9)(...), got:\n{}",
            q.sql
        );
    }

    #[test]
    fn aggregation_uniq_renders_uniq_call() {
        let cfg = AggregationConfig {
            event_key: "page_view".into(),
            aggregator: AggregationOperator::Uniq,
            on_field: Some("user_id".into()),
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap();
        assert!(
            q.sql
                .contains("uniq(properties['user_id']) AS metric_value"),
            "uniq aggregator missing or wrong, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn aggregation_with_where_clause_appends_predicate() {
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(json!({"==": [{"var": "country"}, "US"]})),
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap();

        // Predicate is appended after the fixed WHERE clauses; bind value for
        // the literal "US" lands at index 6 (after env / exp / iter / event /
        // 2 variants).
        assert!(
            q.sql.contains("AND (properties['country'] = {p6})"),
            "where_clause should be appended with parens, got:\n{}",
            q.sql
        );
        assert_eq!(q.binds.last(), Some(&QueryBind::Str("US".into())));
        assert_eq!(q.binds.len(), 7);
    }

    #[test]
    fn aggregation_where_clause_complex_and_or() {
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(json!({
                "and": [
                    {"==": [{"var": "country"}, "US"]},
                    {"or": [
                        {"==": [{"var": "tier"}, "pro"]},
                        {"==": [{"var": "tier"}, "enterprise"]}
                    ]}
                ]
            })),
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap();
        assert!(
            q.sql.contains(
                "AND ((properties['country'] = {p6}) AND \
                 ((properties['tier'] = {p7}) OR (properties['tier'] = {p8})))"
            ),
            "nested and/or should render correctly, got:\n{}",
            q.sql
        );
        assert_eq!(q.binds.len(), 9);
    }

    #[test]
    fn aggregation_unsupported_jsonlogic_returns_error() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(json!({"%": [{"var": "a"}, 2]})),
        };
        let err = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap_err();
        assert_eq!(err, QueryBuildError::UnsupportedJsonLogic("%".into()));
    }

    #[test]
    fn aggregation_sum_without_field_returns_error() {
        let cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: None,
            where_clause: None,
        };
        let err = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn aggregation_empty_variants_returns_error() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let err = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &[]).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn aggregation_reads_from_events_v2_not_mv() {
        // The MV doesn't store percentile / on_field states — the builder
        // must read from raw events_v2 so all aggregators are queryable.
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap();
        assert!(q.sql.contains("FROM events_v2"));
        assert!(!q.sql.contains("events_experiment_daily"));
    }

    #[test]
    fn aggregation_iteration_filter_present() {
        // We need to scope reads to a specific iteration — the iteration_id
        // appears in a contexts array filter.
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap();
        assert!(
            q.sql.contains("t.1 = 'iteration'"),
            "iteration-scoping filter missing, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn aggregation_groups_by_variant_key() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants()).unwrap();
        assert!(q.sql.contains("GROUP BY variant_key"));
        assert!(q.sql.contains("HAVING variant_key != ''"));
    }
}
