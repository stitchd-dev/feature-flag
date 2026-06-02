//! ClickHouse query builders for the experiment-stats metric dispatcher.
//!
//! Three pure functions live under this module — one per [`MetricKind`]:
//!
//! - [`aggregation::build_aggregation_query`] — count / sum / avg / percentiles
//!   / uniq over a single event stream, optionally filtered by a JsonLogic
//!   `where_clause`.
//! - [`ratio::build_ratio_query`] — derived metric expressed as
//!   `numerator / denominator`; both legs are resolved `AggregationConfig`s
//!   joined via two CTEs.
//! - [`funnel::build_funnel_query`] — ordered event sequence evaluated with
//!   ClickHouse's `windowFunnel` function.
//!
//! Each builder returns a [`BuiltQuery`] — a SQL string with positional
//! placeholders (`{p0}`, `{p1}`, …) plus a parallel [`QueryBind`] vector.
//! The dispatcher (Phase 4 Task 2) is responsible for binding the values
//! against the live `clickhouse::Client` — the builders themselves perform
//! **no** I/O and are 100% pure, which makes them straightforward to cover
//! with golden-SQL snapshot tests.
//!
//! [`MetricKind`]: stitchd_core::metric::MetricKind

pub mod aggregation;
pub mod funnel;
pub mod interaction;
pub mod preview;
pub mod ratio;

use thiserror::Error;

// ── BuiltQuery ───────────────────────────────────────────────────────────────

/// The output of a metric query builder.
///
/// Carries a SQL string with positional placeholders (`{p0}`, `{p1}`, …) and
/// the parallel vector of bind values. The dispatcher rewrites placeholders
/// to `?` (or to a literal substitution, see Phase 4 Task 2) and calls
/// `clickhouse::Client::query(...).bind(...)` for each value in order.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltQuery {
    /// ClickHouse SQL with `{p0}`, `{p1}`, … positional placeholders.
    pub sql: String,
    /// Bind values in `{p0}`, `{p1}`, … order — one entry per placeholder.
    pub binds: Vec<QueryBind>,
}

/// A single bind value for a [`BuiltQuery`].
///
/// The three native ClickHouse scalar shapes we need are: string (event/key
/// payloads, properties values), 64-bit int (counts, timestamps, IDs cast
/// to numeric) and 64-bit float (numeric metric payloads).
#[derive(Debug, Clone, PartialEq)]
pub enum QueryBind {
    /// UTF-8 string — ClickHouse `String` / `LowCardinality(String)` /
    /// `FixedString` columns.
    Str(String),
    /// 64-bit signed integer — ClickHouse `Int64`, `Int32` (widened),
    /// `UInt32` (widened), `DateTime` (epoch seconds), …
    I64(i64),
    /// 64-bit float — ClickHouse `Float64`.
    F64(f64),
}

impl QueryBind {
    /// Borrow the underlying string slice if this bind is a [`Self::Str`].
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Return the underlying integer if this bind is a [`Self::I64`].
    #[must_use]
    pub const fn as_i64(&self) -> Option<i64> {
        match self {
            Self::I64(n) => Some(*n),
            _ => None,
        }
    }

    /// Return the underlying float if this bind is a [`Self::F64`].
    #[must_use]
    pub const fn as_f64(&self) -> Option<f64> {
        match self {
            Self::F64(f) => Some(*f),
            _ => None,
        }
    }
}

// ── QueryBuildError ──────────────────────────────────────────────────────────

/// Errors returned by the metric query builders.
///
/// Builders fail in two situations:
///
/// 1. The caller passed an unsupported / malformed `where_clause` JsonLogic
///    payload. The supported subset is intentionally tiny — see
///    [`UnsupportedJsonLogic`] — so reviewers must extend it deliberately
///    when product needs grow.
/// 2. The caller passed a structurally-invalid metric config (e.g. funnel
///    with `steps.len() < 2`). The kind-specific config validators in
///    `stitchd_core::metric` would normally catch these earlier; the
///    builders re-check defensively because they are pure functions and
///    can be called with hand-rolled fixtures in tests.
///
/// [`UnsupportedJsonLogic`]: QueryBuildError::UnsupportedJsonLogic
#[derive(Debug, Error, PartialEq)]
pub enum QueryBuildError {
    /// The supplied JsonLogic operator is outside the supported subset
    /// (`==`, `!=`, `in`, `and`, `or`).
    #[error("unsupported JsonLogic operator: `{0}`")]
    UnsupportedJsonLogic(String),

    /// The JsonLogic payload had a structurally-invalid shape — e.g. an
    /// operator that should take two args was passed three, or the LHS of
    /// a comparison was not a `{"var": "..."}` reference.
    #[error("invalid JsonLogic payload: {0}")]
    InvalidJsonLogic(String),

    /// The metric config itself failed a shape check (e.g. funnel with one
    /// step).
    #[error("invalid metric config: {0}")]
    InvalidConfig(String),
}

// ── Internal helpers used across builders ────────────────────────────────────

/// Push a bind value and return its placeholder string (`{p<n>}`).
///
/// Builders accumulate binds into a `Vec<QueryBind>` and weave placeholders
/// into the emitted SQL. This helper keeps the indexing logic in one place
/// so the three modules can't drift out of step.
pub(crate) fn push_bind(binds: &mut Vec<QueryBind>, value: QueryBind) -> String {
    let idx = binds.len();
    binds.push(value);
    format!("{{p{idx}}}")
}

/// Translate a JsonLogic expression into a ClickHouse SQL predicate
/// fragment, emitting bind values via [`push_bind`].
///
/// Supported operators:
///
/// - `{"==": [{"var": "<field>"}, "<literal>"]}`
/// - `{"!=": [{"var": "<field>"}, "<literal>"]}`
/// - `{"in": [{"var": "<field>"}, ["<lit1>", "<lit2>", …]]}`
/// - `{"and": [<expr1>, <expr2>, …]}`
/// - `{"or":  [<expr1>, <expr2>, …]}`
///
/// `<field>` is treated as a key in the `events.properties Map` —
/// references are emitted as `properties['<field>']`. Literals are bound
/// (never inlined) to keep the SQL injection-safe. `<lit>` may be a
/// number, string or bool — booleans / numbers are coerced to strings
/// because `properties` is `Map(String, String)`.
///
/// # Errors
/// Returns [`QueryBuildError::UnsupportedJsonLogic`] if an operator is
/// outside the supported set, or [`QueryBuildError::InvalidJsonLogic`]
/// if the payload is structurally malformed.
pub(crate) fn jsonlogic_to_sql(
    expr: &serde_json::Value,
    binds: &mut Vec<QueryBind>,
) -> Result<String, QueryBuildError> {
    let obj = expr
        .as_object()
        .ok_or_else(|| QueryBuildError::InvalidJsonLogic("expected JSON object".into()))?;

    if obj.len() != 1 {
        return Err(QueryBuildError::InvalidJsonLogic(
            "JsonLogic node must have exactly one operator key".into(),
        ));
    }

    let (op, args) = obj.iter().next().expect("len == 1 checked above");
    let args = args.as_array().ok_or_else(|| {
        QueryBuildError::InvalidJsonLogic(format!("operator `{op}` requires an array of args"))
    })?;

    match op.as_str() {
        "==" | "!=" => emit_binary_cmp(op, args, binds),
        "in" => emit_in(args, binds),
        "and" => emit_combinator("AND", args, binds),
        "or" => emit_combinator("OR", args, binds),
        other => Err(QueryBuildError::UnsupportedJsonLogic(other.to_string())),
    }
}

/// Render a `==` / `!=` JsonLogic node.
fn emit_binary_cmp(
    op: &str,
    args: &[serde_json::Value],
    binds: &mut Vec<QueryBind>,
) -> Result<String, QueryBuildError> {
    if args.len() != 2 {
        return Err(QueryBuildError::InvalidJsonLogic(format!(
            "`{op}` requires exactly 2 args (got {})",
            args.len()
        )));
    }
    let field = extract_var(&args[0])?;
    let literal = extract_literal(&args[1])?;
    let placeholder = push_bind(binds, QueryBind::Str(literal));
    let sql_op = if op == "==" { "=" } else { "!=" };
    Ok(format!("properties['{field}'] {sql_op} {placeholder}"))
}

/// Render an `in` JsonLogic node.
fn emit_in(
    args: &[serde_json::Value],
    binds: &mut Vec<QueryBind>,
) -> Result<String, QueryBuildError> {
    if args.len() != 2 {
        return Err(QueryBuildError::InvalidJsonLogic(format!(
            "`in` requires exactly 2 args (got {})",
            args.len()
        )));
    }
    let field = extract_var(&args[0])?;
    let needle_list = args[1]
        .as_array()
        .ok_or_else(|| QueryBuildError::InvalidJsonLogic("`in` RHS must be an array".into()))?;
    if needle_list.is_empty() {
        return Err(QueryBuildError::InvalidJsonLogic(
            "`in` RHS array must be non-empty".into(),
        ));
    }
    let mut placeholders = Vec::with_capacity(needle_list.len());
    for v in needle_list {
        let literal = extract_literal(v)?;
        placeholders.push(push_bind(binds, QueryBind::Str(literal)));
    }
    Ok(format!(
        "properties['{field}'] IN ({})",
        placeholders.join(", ")
    ))
}

/// Render an `and` / `or` JsonLogic combinator.
fn emit_combinator(
    sql_op: &str,
    args: &[serde_json::Value],
    binds: &mut Vec<QueryBind>,
) -> Result<String, QueryBuildError> {
    if args.is_empty() {
        return Err(QueryBuildError::InvalidJsonLogic(format!(
            "`{sql_op}` requires at least one operand"
        )));
    }
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(format!("({})", jsonlogic_to_sql(arg, binds)?));
    }
    Ok(parts.join(&format!(" {sql_op} ")))
}

/// Pull a field name out of a `{"var": "..."}` node.
fn extract_var(node: &serde_json::Value) -> Result<String, QueryBuildError> {
    node.as_object()
        .and_then(|m| {
            if m.len() != 1 {
                return None;
            }
            m.get("var")?.as_str().map(str::to_owned)
        })
        .ok_or_else(|| {
            QueryBuildError::InvalidJsonLogic(
                "expected a `{\"var\": \"<field>\"}` reference".into(),
            )
        })
}

/// Pull a scalar literal out of a JSON node. Numbers and bools are
/// coerced to strings — the `properties` column is `Map(String, String)`.
fn extract_literal(node: &serde_json::Value) -> Result<String, QueryBuildError> {
    match node {
        serde_json::Value::String(s) => Ok(s.clone()),
        serde_json::Value::Bool(b) => Ok(b.to_string()),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        _ => Err(QueryBuildError::InvalidJsonLogic(
            "literal must be a string, bool or number".into(),
        )),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── BuiltQuery / QueryBind accessors ─────────────────────────────────────

    #[test]
    fn query_bind_str_accessors() {
        let b = QueryBind::Str("hello".into());
        assert_eq!(b.as_str(), Some("hello"));
        assert_eq!(b.as_i64(), None);
        assert_eq!(b.as_f64(), None);
    }

    #[test]
    fn query_bind_i64_accessors() {
        let b = QueryBind::I64(42);
        assert_eq!(b.as_i64(), Some(42));
        assert_eq!(b.as_str(), None);
    }

    #[test]
    fn query_bind_f64_accessors() {
        let b = QueryBind::F64(2.5);
        assert!(b.as_f64().is_some_and(|v| (v - 2.5).abs() < f64::EPSILON));
        assert_eq!(b.as_str(), None);
    }

    #[test]
    fn built_query_equality() {
        let q1 = BuiltQuery {
            sql: "SELECT 1".into(),
            binds: vec![QueryBind::I64(1)],
        };
        let q2 = BuiltQuery {
            sql: "SELECT 1".into(),
            binds: vec![QueryBind::I64(1)],
        };
        assert_eq!(q1, q2);
    }

    // ── push_bind ─────────────────────────────────────────────────────────────

    #[test]
    fn push_bind_returns_indexed_placeholder() {
        let mut binds = Vec::new();
        assert_eq!(push_bind(&mut binds, QueryBind::Str("a".into())), "{p0}");
        assert_eq!(push_bind(&mut binds, QueryBind::I64(7)), "{p1}");
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].as_str(), Some("a"));
        assert_eq!(binds[1].as_i64(), Some(7));
    }

    // ── jsonlogic_to_sql: eq / ne ────────────────────────────────────────────

    #[test]
    fn jsonlogic_eq_emits_equality_with_bind() {
        let mut binds = Vec::new();
        let sql = jsonlogic_to_sql(&json!({"==": [{"var": "country"}, "US"]}), &mut binds).unwrap();
        assert_eq!(sql, "properties['country'] = {p0}");
        assert_eq!(binds, vec![QueryBind::Str("US".into())]);
    }

    #[test]
    fn jsonlogic_ne_emits_inequality_with_bind() {
        let mut binds = Vec::new();
        let sql = jsonlogic_to_sql(&json!({"!=": [{"var": "tier"}, "free"]}), &mut binds).unwrap();
        assert_eq!(sql, "properties['tier'] != {p0}");
        assert_eq!(binds, vec![QueryBind::Str("free".into())]);
    }

    // ── jsonlogic_to_sql: in ─────────────────────────────────────────────────

    #[test]
    fn jsonlogic_in_emits_in_list_with_binds() {
        let mut binds = Vec::new();
        let sql = jsonlogic_to_sql(
            &json!({"in": [{"var": "plan"}, ["pro", "team", "enterprise"]]}),
            &mut binds,
        )
        .unwrap();
        assert_eq!(sql, "properties['plan'] IN ({p0}, {p1}, {p2})");
        assert_eq!(
            binds,
            vec![
                QueryBind::Str("pro".into()),
                QueryBind::Str("team".into()),
                QueryBind::Str("enterprise".into()),
            ]
        );
    }

    #[test]
    fn jsonlogic_in_empty_array_rejected() {
        let mut binds = Vec::new();
        let err = jsonlogic_to_sql(&json!({"in": [{"var": "x"}, []]}), &mut binds).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidJsonLogic(_)));
    }

    // ── jsonlogic_to_sql: and / or ───────────────────────────────────────────

    #[test]
    fn jsonlogic_and_combines_with_parentheses() {
        let mut binds = Vec::new();
        let sql = jsonlogic_to_sql(
            &json!({
                "and": [
                    {"==": [{"var": "country"}, "US"]},
                    {"!=": [{"var": "tier"}, "free"]}
                ]
            }),
            &mut binds,
        )
        .unwrap();
        assert_eq!(
            sql,
            "(properties['country'] = {p0}) AND (properties['tier'] != {p1})"
        );
        assert_eq!(
            binds,
            vec![QueryBind::Str("US".into()), QueryBind::Str("free".into())]
        );
    }

    #[test]
    fn jsonlogic_or_combines_with_parentheses() {
        let mut binds = Vec::new();
        let sql = jsonlogic_to_sql(
            &json!({
                "or": [
                    {"==": [{"var": "country"}, "US"]},
                    {"==": [{"var": "country"}, "CA"]}
                ]
            }),
            &mut binds,
        )
        .unwrap();
        assert_eq!(
            sql,
            "(properties['country'] = {p0}) OR (properties['country'] = {p1})"
        );
    }

    // ── jsonlogic_to_sql: unsupported / malformed ────────────────────────────

    #[test]
    fn jsonlogic_unsupported_operator_returns_typed_error() {
        let mut binds = Vec::new();
        let err = jsonlogic_to_sql(&json!({"%": [1, 2]}), &mut binds).unwrap_err();
        assert_eq!(err, QueryBuildError::UnsupportedJsonLogic("%".to_string()));
    }

    #[test]
    fn jsonlogic_eq_with_wrong_arity_rejected() {
        let mut binds = Vec::new();
        let err =
            jsonlogic_to_sql(&json!({"==": [{"var": "a"}, "b", "c"]}), &mut binds).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidJsonLogic(_)));
    }

    #[test]
    fn jsonlogic_eq_without_var_lhs_rejected() {
        let mut binds = Vec::new();
        let err = jsonlogic_to_sql(&json!({"==": ["literal", "literal"]}), &mut binds).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidJsonLogic(_)));
    }

    #[test]
    fn jsonlogic_top_level_non_object_rejected() {
        let mut binds = Vec::new();
        let err = jsonlogic_to_sql(&json!([1, 2, 3]), &mut binds).unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidJsonLogic(_)));
    }

    #[test]
    fn jsonlogic_multiple_keys_in_node_rejected() {
        let mut binds = Vec::new();
        let err = jsonlogic_to_sql(
            &json!({"==": [{"var": "a"}, "b"], "!=": [{"var": "c"}, "d"]}),
            &mut binds,
        )
        .unwrap_err();
        assert!(matches!(err, QueryBuildError::InvalidJsonLogic(_)));
    }

    #[test]
    fn jsonlogic_literal_number_coerced_to_string_bind() {
        let mut binds = Vec::new();
        let sql = jsonlogic_to_sql(&json!({"==": [{"var": "age"}, 42]}), &mut binds).unwrap();
        assert_eq!(sql, "properties['age'] = {p0}");
        assert_eq!(binds, vec![QueryBind::Str("42".into())]);
    }

    #[test]
    fn jsonlogic_literal_bool_coerced_to_string_bind() {
        let mut binds = Vec::new();
        let sql = jsonlogic_to_sql(&json!({"==": [{"var": "active"}, true]}), &mut binds).unwrap();
        assert_eq!(sql, "properties['active'] = {p0}");
        assert_eq!(binds, vec![QueryBind::Str("true".into())]);
    }
}
