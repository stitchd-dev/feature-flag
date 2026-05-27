//! Aggregation query builder.
//!
//! Produces a `SELECT context_type, variant_key, <agg>(...) FROM events
//! JOIN experiment_assignments ...` query that:
//!
//! - JOINs `events e` against `experiment_assignments a` on
//!   `(env_id, context_type, context_key)`. Because `events.contexts`
//!   is `Array(Tuple(String, String))`, the events side is first flattened
//!   via `ARRAY JOIN e.contexts AS ctx_pair`, then equi-joined on the
//!   tuple's `.1`/`.2` against `a.context_type`/`a.context_key`. (CH 24's
//!   new analyzer rejects `arrayExists(...)` inside `JOIN ON` with
//!   `Not found column __table2.context_type ...`; the legacy analyzer
//!   rejects it with `INVALID_JOIN_ON_EXPRESSION`. Same semantics — one
//!   event row per matched assignment context — without the analyzer
//!   trap; mirrors the Phase 11 E2E pattern in
//!   `crates/stitchd-experimentation-service/tests/experiment_lifecycle_e2e.rs`.)
//! - Filters by `experiment_id`, `iteration_id`, `env_id`, the iteration
//!   time window (`a.assigned_at <= e.occurred_at < iteration_end`) and
//!   the supplied `variant_keys` allow-list.
//! - Applies the optional `where_clause` (JsonLogic) as an extra
//!   predicate against `events.properties Map(String, String)`.
//! - Reads from raw `events` (not the daily MV) so percentiles,
//!   custom `on_field`, and property-level filters all stay queryable.
//!
//! ## Strict intent-to-treat (ITT)
//!
//! `e.occurred_at >= a.assigned_at` excludes pre-exposure events — only
//! events fired AFTER the context's first matching evaluation count.
//! `e.occurred_at < iteration_end` excludes post-iteration events.
//!
//! ## Per-context-type grouping
//!
//! `GROUP BY (a.context_type, a.variant_key)` returns one row per
//! `(context_type, variant_key)` pair. The caller builds the per-context-type
//! result tree from the row set (Task 5.5).
//!
//! ## Bind order
//!
//! `clickhouse-rs` binds `?` placeholders by SQL position, not by
//! `Vec<QueryBind>` index. Every push MUST happen in left-to-right SQL
//! appearance order. The order this file emits is:
//!
//!   1. `env_id`            (in `WHERE a.env_id = toUUID(?)` — but only
//!      because `env_id` is the first predicate that references it)
//!   2. `experiment_id`     (in `WHERE a.experiment_id = toUUID(?)`)
//!   3. `iteration_id`      (in `WHERE a.iteration_id = toUUID(?)`)
//!   4. `metric_key`        (in `WHERE e.metric_key = ?`)
//!   5. `iteration_end`     (in `e.occurred_at < ?`)
//!   6. variant_keys        (`a.variant_key IN (?, ?, …)`)
//!   7. JsonLogic literals  (appended after the IN list)
//!
//! See the parent module ([`super`]) for the [`BuiltQuery`] /
//! [`QueryBind`] / [`QueryBuildError`] types.
//!
//! [`BuiltQuery`]: super::BuiltQuery
//! [`QueryBind`]: super::QueryBind
//! [`QueryBuildError`]: super::QueryBuildError

use chrono::{DateTime, Utc};
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
/// `iteration_end` is the upper bound on `e.occurred_at`. Callers pass
/// `iteration.ended_at` when the iteration has concluded, or `Utc::now()`
/// for in-progress iterations. The builder cannot derive this from CH
/// because the iteration table is in PostgreSQL.
///
/// # Errors
/// Returns a [`QueryBuildError`] when `variant_keys` is empty or the
/// `where_clause` JsonLogic uses an unsupported operator.
pub fn build_aggregation_query(
    cfg: &AggregationConfig,
    experiment_id: &str,
    iteration_id: &str,
    env_id: &str,
    variant_keys: &[&str],
    iteration_end: DateTime<Utc>,
) -> Result<BuiltQuery, QueryBuildError> {
    if variant_keys.is_empty() {
        return Err(QueryBuildError::InvalidConfig(
            "variant_keys must not be empty".into(),
        ));
    }

    let mut binds = Vec::new();

    // Push binds in SQL-appearance order. The final SQL emits placeholders
    // in this exact sequence:
    //   1. env_id      — `a.env_id = toUUID(?)`
    //   2. experiment  — `a.experiment_id = toUUID(?)`
    //   3. iteration   — `a.iteration_id = toUUID(?)`
    //   4. metric_key  — `e.metric_key = ?`
    //   5. iter_end    — `e.occurred_at < ?`
    //   6. variants    — `a.variant_key IN (?, ?, …)`
    //   7. where_clause literals (last; emitted by jsonlogic_to_sql)
    let env_ph = push_bind(&mut binds, QueryBind::Str(env_id.to_owned()));
    let exp_ph = push_bind(&mut binds, QueryBind::Str(experiment_id.to_owned()));
    let iter_ph = push_bind(&mut binds, QueryBind::Str(iteration_id.to_owned()));
    let event_ph = push_bind(&mut binds, QueryBind::Str(cfg.event_key.clone()));
    let iter_end_ph = push_bind(&mut binds, QueryBind::I64(iteration_end.timestamp_millis()));

    let mut variant_phs = Vec::with_capacity(variant_keys.len());
    for vk in variant_keys {
        variant_phs.push(push_bind(&mut binds, QueryBind::Str((*vk).to_owned())));
    }
    let variant_in_list = variant_phs.join(", ");

    // Optional where_clause. JsonLogic literals must be the LAST binds
    // pushed so the SQL-appearance order matches.
    let extra_where = match cfg.where_clause.as_ref() {
        Some(expr) => {
            let frag = jsonlogic_to_sql(expr, &mut binds)?;
            format!("\n  AND ({frag})")
        }
        None => String::new(),
    };

    // Aggregator expression.
    let agg_expr = render_aggregator(cfg)?;

    // ARRAY JOIN flattens each event into one row per (context_type,
    // context_key) tuple in its `contexts` array; the subsequent INNER
    // JOIN against `experiment_assignments` is then a plain equi-join on
    // (env_id, context_type, context_key). This matches the Phase 11 E2E
    // pattern (`crates/stitchd-experimentation-service/tests/experiment_lifecycle_e2e.rs`)
    // and avoids CH 24's analyzer rejecting `arrayExists(...)` inside
    // `JOIN ON`.
    //
    // The aggregator output is wrapped in `toFloat64(...)` so the wire
    // shape is uniform across operators (`count()` → UInt64, `uniq()` →
    // UInt64, `sum`/`avg`/`quantile` → Float64). Without the cast the
    // result row's `metric_value: f64` deserialisation fails with
    // "ClickHouse type UInt64 as f64 not compatible" for count + uniq.
    let sql = format!(
        "SELECT\n    \
            a.context_type AS context_type,\n    \
            a.variant_key AS variant_key,\n    \
            toFloat64({agg_expr}) AS metric_value\n\
        FROM events AS e\n\
        ARRAY JOIN e.contexts AS ctx_pair\n\
        INNER JOIN experiment_assignments AS a\n    \
            ON e.env_id = a.env_id\n   \
            AND ctx_pair.1 = a.context_type\n   \
            AND ctx_pair.2 = a.context_key\n\
        WHERE a.env_id = toUUID({env_ph})\n  \
          AND a.experiment_id = toUUID({exp_ph})\n  \
          AND a.iteration_id = toUUID({iter_ph})\n  \
          AND e.metric_key = {event_ph}\n  \
          AND e.occurred_at >= a.assigned_at\n  \
          AND e.occurred_at < fromUnixTimestamp64Milli({iter_end_ph})\n  \
          AND a.variant_key IN ({variant_in_list}){extra_where}\n\
        GROUP BY a.context_type, a.variant_key\n\
        HAVING a.variant_key != ''"
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
///
/// All references to property / numeric columns are explicitly qualified
/// with `e.` since the FROM clause aliases `events` as `e` to disambiguate
/// from the JOINed `experiment_assignments AS a`.
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
/// operand.
///
/// `on_field` is now **optional**. The routing logic is:
///
/// - `None`, `""`, `"value"`, `"value_double"`, `"value_int"` →
///   `ifNull(coalesce(e.value_double, CAST(e.value_int AS Nullable(Float64))), 0.0)`
///   — the canonical numeric columns are already typed; `toFloat64OrNull`
///   would be rejected by ClickHouse with `ILLEGAL_TYPE_OF_ARGUMENT`.
///
/// - Any other string → `ifNull(toFloat64OrNull(e.properties['<name>']), 0.0)`
///   — `properties` is `Map(String, String)`, so `toFloat64OrNull` is
///   the right coercion.
fn numeric_value_expr(cfg: &AggregationConfig) -> Result<String, QueryBuildError> {
    let use_canonical = matches!(
        cfg.on_field.as_deref(),
        None | Some("") | Some("value") | Some("value_double") | Some("value_int")
    );
    Ok(if use_canonical {
        // Canonical numeric columns — coalesce returns Nullable(Float64);
        // ifNull makes it non-nullable for the aggregator.
        "ifNull(coalesce(e.value_double, CAST(e.value_int AS Nullable(Float64))), 0.0)".to_owned()
    } else {
        let field = cfg.on_field.as_deref().unwrap();
        format!("ifNull(toFloat64OrNull(e.properties['{field}']), 0.0)")
    })
}

/// Resolve the `on_field` reference to a ClickHouse expression.
///
/// `on_field` is now optional.  The routing logic mirrors
/// `numeric_value_expr`:
///
/// - `None` / `""` / `"value"` / `"value_double"` / `"value_int"` →
///   `coalesce(e.value_double, CAST(e.value_int AS Nullable(Float64)))`.
///   Numeric coercion is applied by the caller (e.g. `uniq(...)` works
///   directly on a Nullable Float64).
/// - Any other string → `e.properties['<name>']` (a `String` map
///   lookup; callers apply further coercion as needed).
fn on_field_expr(cfg: &AggregationConfig) -> Result<String, QueryBuildError> {
    let use_canonical = matches!(
        cfg.on_field.as_deref(),
        None | Some("") | Some("value") | Some("value_double") | Some("value_int")
    );
    Ok(if use_canonical {
        "coalesce(e.value_double, CAST(e.value_int AS Nullable(Float64)))".to_owned()
    } else {
        let field = cfg.on_field.as_deref().unwrap();
        format!("e.properties['{field}']")
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    const ENV_ID: &str = "00000000-0000-0000-0000-000000000001";
    const EXP_ID: &str = "00000000-0000-0000-0000-000000000002";
    const ITER_ID: &str = "00000000-0000-0000-0000-000000000003";

    fn variants() -> Vec<&'static str> {
        vec!["control", "treatment"]
    }

    fn iter_end() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 21, 0, 0, 0).unwrap()
    }

    // ── Golden-SQL snapshot ──────────────────────────────────────────────────

    #[test]
    fn aggregation_count_simple() {
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();

        // Verify the JOIN structure is correct.
        assert!(
            q.sql.contains("FROM events AS e"),
            "must read from events alias e, got:\n{}",
            q.sql
        );
        assert!(
            q.sql.contains("INNER JOIN experiment_assignments AS a"),
            "must JOIN experiment_assignments alias a, got:\n{}",
            q.sql
        );
        assert!(
            q.sql.contains("ARRAY JOIN e.contexts AS ctx_pair"),
            "must ARRAY JOIN e.contexts (CH 24 analyzer rejects arrayExists in JOIN ON), got:\n{}",
            q.sql
        );
        assert!(
            q.sql.contains("ctx_pair.1 = a.context_type"),
            "join predicate must equi-join ctx_pair.1 to a.context_type, got:\n{}",
            q.sql
        );
        assert!(
            q.sql.contains("ctx_pair.2 = a.context_key"),
            "join predicate must equi-join ctx_pair.2 to a.context_key, got:\n{}",
            q.sql
        );
        assert!(
            !q.sql.contains("arrayExists"),
            "no arrayExists in JOIN ON (CH 24 analyzer rejects this shape), got:\n{}",
            q.sql
        );
        // ITT bound + iteration upper bound.
        assert!(
            q.sql.contains("e.occurred_at >= a.assigned_at"),
            "ITT lower bound missing, got:\n{}",
            q.sql
        );
        assert!(
            q.sql
                .contains("e.occurred_at < fromUnixTimestamp64Milli({p4})"),
            "iteration upper bound bind missing, got:\n{}",
            q.sql
        );
        // Per-(context_type, variant_key) GROUP BY.
        assert!(
            q.sql.contains("GROUP BY a.context_type, a.variant_key"),
            "must group by context_type + variant_key, got:\n{}",
            q.sql
        );

        // Bind order check: env, exp, iter, event_key, iter_end_ms, variant_keys
        assert_eq!(q.binds[0], QueryBind::Str(ENV_ID.into()));
        assert_eq!(q.binds[1], QueryBind::Str(EXP_ID.into()));
        assert_eq!(q.binds[2], QueryBind::Str(ITER_ID.into()));
        assert_eq!(q.binds[3], QueryBind::Str("checkout_completed".into()));
        assert_eq!(q.binds[4], QueryBind::I64(iter_end().timestamp_millis()));
        assert_eq!(q.binds[5], QueryBind::Str("control".into()));
        assert_eq!(q.binds[6], QueryBind::Str("treatment".into()));
        assert_eq!(q.binds.len(), 7);
    }

    #[test]
    fn aggregation_no_event_side_experiment_context_tags() {
        // The spec's headline cutover: no more event-side filtering on
        // contexts for 'experiment' / 'iteration' / 'variant' tags. The
        // attribution comes from `experiment_assignments` instead.
        let cfg = AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();

        assert!(
            !q.sql.contains("'experiment'"),
            "event-side experiment context filter must be removed, got:\n{}",
            q.sql
        );
        assert!(
            !q.sql.contains("'iteration'"),
            "event-side iteration context filter must be removed, got:\n{}",
            q.sql
        );
        assert!(
            !q.sql.contains("'variant'"),
            "event-side variant context filter must be removed, got:\n{}",
            q.sql
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
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();
        assert!(
            q.sql.contains(
                "toFloat64(sum(ifNull(toFloat64OrNull(e.properties['revenue']), 0.0))) AS metric_value"
            ),
            "sum aggregator should wrap e.properties['revenue'] in ifNull+toFloat64OrNull and the whole agg in toFloat64, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn aggregation_avg_on_value_column_uses_native_columns() {
        // on_field == "value" routes to the canonical numeric columns
        // (e.value_double / e.value_int) and skips `toFloat64OrNull` — the
        // coalesce already returns Nullable(Float64) and ClickHouse
        // rejects toFloat64OrNull(Float64) with ILLEGAL_TYPE_OF_ARGUMENT.
        let cfg = AggregationConfig {
            event_key: "latency".into(),
            aggregator: AggregationOperator::Avg,
            on_field: Some("value".into()),
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();
        assert!(
            q.sql.contains(
                "toFloat64(avg(ifNull(coalesce(e.value_double, CAST(e.value_int AS Nullable(Float64))), 0.0)))"
            ),
            "avg over 'value' should coalesce native numeric columns directly and be wrapped in toFloat64, got:\n{}",
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
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();
        assert!(
            q.sql.contains(
                "toFloat64(quantile(0.9)(ifNull(toFloat64OrNull(e.properties['latency_ms']), 0.0)))"
            ),
            "P90 should emit quantile(0.9)(...) wrapped in toFloat64, got:\n{}",
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
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();
        assert!(
            q.sql
                .contains("toFloat64(uniq(e.properties['user_id'])) AS metric_value"),
            "uniq aggregator missing or wrong (must be wrapped in toFloat64), got:\n{}",
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
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();

        // Predicate is appended after the IN list; bind value for the
        // literal "US" lands at index 7 (after env / exp / iter / event /
        // iter_end / 2 variants).
        assert!(
            q.sql.contains("AND (properties['country'] = {p7})"),
            "where_clause should be appended with parens, got:\n{}",
            q.sql
        );
        assert_eq!(q.binds.last(), Some(&QueryBind::Str("US".into())));
        assert_eq!(q.binds.len(), 8);
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
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();
        // Bind indices for where literals start at 7 (after env, exp, iter,
        // event, iter_end, ctrl, treat).
        assert!(
            q.sql.contains(
                "AND ((properties['country'] = {p7}) AND \
                 ((properties['tier'] = {p8}) OR (properties['tier'] = {p9})))"
            ),
            "nested and/or should render correctly, got:\n{}",
            q.sql
        );
        assert_eq!(q.binds.len(), 10);
    }

    #[test]
    fn aggregation_unsupported_jsonlogic_returns_error() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: Some(json!({"%": [{"var": "a"}, 2]})),
        };
        let err = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap_err();
        assert_eq!(err, QueryBuildError::UnsupportedJsonLogic("%".into()));
    }

    #[test]
    fn aggregation_sum_without_field_uses_canonical_value_columns() {
        // on_field is now optional — sum without it uses canonical value columns.
        let cfg = AggregationConfig {
            event_key: "purchase".into(),
            aggregator: AggregationOperator::Sum,
            on_field: None,
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();
        assert!(
            q.sql.contains("coalesce(e.value_double"),
            "sum without on_field should use canonical columns, got:\n{}",
            q.sql
        );
        assert!(
            !q.sql.contains("properties["),
            "sum without on_field must NOT read from properties map, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn aggregation_empty_variants_returns_error() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let err =
            build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &[], iter_end()).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidConfig(_)));
    }

    #[test]
    fn aggregation_reads_from_events_not_mv() {
        // The MV doesn't store percentile / on_field states — the builder
        // must read from raw events so all aggregators are queryable.
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();
        assert!(q.sql.contains("FROM events"));
        assert!(!q.sql.contains("events_experiment_daily"));
    }

    #[test]
    fn aggregation_iteration_filter_uses_assignments_join() {
        // Iteration scoping is now derived from experiment_assignments —
        // the iteration_id is bound against `a.iteration_id`, not
        // hand-rolled into the event row's contexts array.
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();
        assert!(
            q.sql.contains("a.iteration_id = toUUID({p2})"),
            "iteration scoping must come from `a.iteration_id`, got:\n{}",
            q.sql
        );
    }

    #[test]
    fn aggregation_groups_by_context_type_and_variant_key() {
        let cfg = AggregationConfig {
            event_key: "x".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        };
        let q = build_aggregation_query(&cfg, EXP_ID, ITER_ID, ENV_ID, &variants(), iter_end())
            .unwrap();
        assert!(q.sql.contains("GROUP BY a.context_type, a.variant_key"));
        assert!(q.sql.contains("HAVING a.variant_key != ''"));
    }
}
