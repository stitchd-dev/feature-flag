//! ClickHouse fetch for CUPED pre-period covariates.
//!
//! Queries `events` to aggregate a numeric `properties['value']`-style
//! signal per `(context_type, context_key)` over a pre-experiment time window.
//! The result feeds `stitchd_core::experimentation::stats::cuped::apply_cuped`
//! one observation per assigned context with `x_pre` set from the returned
//! aggregate.
//!
//! ## Contract
//!
//! - Filtered by `env_id`, `metric_key`, the pre-period bounds
//!   `[pre_period_start, pre_period_end)`, and the experiment's
//!   `unit_context_types`.
//! - Grouped by `(context_type, context_key)` (the join key with the
//!   post-period assignment table).
//! - Each context's aggregated pre-value is the SUM of the metric's
//!   numeric column (preferring `value_double`, falling back to `value_int`,
//!   then `properties['value']` cast to Float64). Callers can switch to AVG /
//!   COUNT by composing their own SQL — this helper covers the canonical
//!   SUM-aggregation case used by the spec §3 CUPED pipeline.
//!
//! Returns a `HashMap<(context_type, context_key), x_pre>` so the caller
//! can join against the post-period observed values for each
//! `assigned_context`. Contexts with no pre-period events do not appear in
//! the map; the caller should treat those as `x_pre = 0` (or filter them
//! out, depending on the metric's interpretation of "no events" — see the
//! Phase 6 / CUPED documentation in the track spec).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use clickhouse::Client;
use serde::Deserialize;
use uuid::Uuid;

// ── Public types ─────────────────────────────────────────────────────────────

/// Errors returned by [`fetch_pre_period_observations`].
#[derive(Debug, thiserror::Error)]
pub enum CupedFetchError {
    /// ClickHouse query / network failure.
    #[error("clickhouse error: {0}")]
    Clickhouse(#[from] clickhouse::error::Error),

    /// Invalid arguments — caught before the network round-trip.
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
}

/// One aggregated pre-period row returned by ClickHouse.
#[derive(Debug, Clone, Deserialize, clickhouse::Row)]
struct PrePeriodRow {
    context_type: String,
    context_key: String,
    x_pre: f64,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Fetch per-context aggregated pre-period values for CUPED variance
/// reduction.
///
/// Returns a map keyed by `(context_type, context_key)`. Callers join this
/// against the post-period assignment list to build `CupedObservation`
/// vectors per variant.
///
/// `assigned_contexts` is an optional whitelist of `(context_type, context_key)`
/// pairs — when non-empty, the SQL `WHERE` clause filters on it via a
/// `(context_type, context_key) IN (…)` tuple match so we don't pull
/// pre-period events for contexts that aren't in the experiment.
pub async fn fetch_pre_period_observations(
    ch: &Client,
    env_id: Uuid,
    metric_key: &str,
    pre_period_start: DateTime<Utc>,
    pre_period_end: DateTime<Utc>,
    unit_context_types: &[String],
    assigned_contexts: &[(String, String)],
) -> Result<HashMap<(String, String), f64>, CupedFetchError> {
    if metric_key.is_empty() {
        return Err(CupedFetchError::InvalidArgument("metric_key is empty"));
    }
    if unit_context_types.is_empty() {
        return Err(CupedFetchError::InvalidArgument(
            "unit_context_types is empty",
        ));
    }
    if pre_period_end <= pre_period_start {
        return Err(CupedFetchError::InvalidArgument(
            "pre_period_end must be strictly after pre_period_start",
        ));
    }

    // No assigned contexts → return empty (nothing to join against).
    if assigned_contexts.is_empty() {
        return Ok(HashMap::new());
    }

    let env_id_str = env_id.to_string();
    let metric_key_esc = clickhouse_escape(metric_key);
    let lower_ms = pre_period_start.timestamp_millis();
    let upper_ms = pre_period_end.timestamp_millis();

    let ctx_types_in = unit_context_types
        .iter()
        .map(|t| format!("'{}'", clickhouse_escape(t)))
        .collect::<Vec<_>>()
        .join(", ");

    let assigned_in = assigned_contexts
        .iter()
        .map(|(t, k)| format!("('{}','{}')", clickhouse_escape(t), clickhouse_escape(k)))
        .collect::<Vec<_>>()
        .join(", ");

    // Per-context pre-period aggregate. We unroll `contexts` (Array(Tuple))
    // and filter to the experiment's `unit_context_types`, then SUM the
    // metric's numeric value with priority value_double > value_int >
    // toFloat64OrNull(properties['value']).
    let sql = format!(
        r"
        SELECT
            ctx.1 AS context_type,
            ctx.2 AS context_key,
            sum(
              coalesce(
                value_double,
                toNullable(toFloat64OrZero(toString(value_int))),
                toFloat64OrNull(properties['value']),
                0.0
              )
            ) AS x_pre
        FROM events
        ARRAY JOIN contexts AS ctx
        WHERE env_id = '{env_id_str}'
          AND metric_key = '{metric_key_esc}'
          AND timestamp >= fromUnixTimestamp64Milli({lower_ms})
          AND timestamp <  fromUnixTimestamp64Milli({upper_ms})
          AND ctx.1 IN ({ctx_types_in})
          AND (ctx.1, ctx.2) IN ({assigned_in})
        GROUP BY context_type, context_key
        "
    );

    let rows: Vec<PrePeriodRow> = ch.query(&sql).fetch_all::<PrePeriodRow>().await?;

    let mut map = HashMap::with_capacity(rows.len());
    for r in rows {
        map.insert((r.context_type, r.context_key), r.x_pre);
    }
    Ok(map)
}

/// Escape ClickHouse string literals — backslash + single quote only. This
/// is sufficient for the values we emit here (context types, keys, metric
/// keys); not a general-purpose SQL sanitizer.
fn clickhouse_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn now_minus(hours: i64) -> DateTime<Utc> {
        Utc::now() - Duration::hours(hours)
    }

    #[tokio::test]
    async fn empty_metric_key_returns_invalid_argument() {
        let ch = Client::default();
        let r = fetch_pre_period_observations(
            &ch,
            Uuid::new_v4(),
            "",
            now_minus(48),
            now_minus(0),
            &["user".to_string()],
            &[("user".to_string(), "ctx-1".to_string())],
        )
        .await;
        assert!(matches!(r, Err(CupedFetchError::InvalidArgument(_))));
    }

    #[tokio::test]
    async fn empty_unit_context_types_returns_invalid_argument() {
        let ch = Client::default();
        let r = fetch_pre_period_observations(
            &ch,
            Uuid::new_v4(),
            "metric.x",
            now_minus(48),
            now_minus(0),
            &[],
            &[("user".to_string(), "ctx-1".to_string())],
        )
        .await;
        assert!(matches!(r, Err(CupedFetchError::InvalidArgument(_))));
    }

    #[tokio::test]
    async fn inverted_pre_period_returns_invalid_argument() {
        let ch = Client::default();
        // end <= start — invalid window.
        let r = fetch_pre_period_observations(
            &ch,
            Uuid::new_v4(),
            "metric.x",
            now_minus(0),
            now_minus(48), // before start
            &["user".to_string()],
            &[("user".to_string(), "ctx-1".to_string())],
        )
        .await;
        assert!(matches!(r, Err(CupedFetchError::InvalidArgument(_))));
    }

    /// Empty assigned_contexts short-circuits without touching ClickHouse.
    #[tokio::test]
    async fn empty_assigned_contexts_returns_empty_without_db() {
        let ch = Client::default();
        let r = fetch_pre_period_observations(
            &ch,
            Uuid::new_v4(),
            "metric.x",
            now_minus(48),
            now_minus(0),
            &["user".to_string()],
            &[],
        )
        .await
        .expect("empty assigned_contexts must succeed without a DB round-trip");
        assert!(r.is_empty());
    }

    #[test]
    fn clickhouse_escape_handles_quotes() {
        assert_eq!(clickhouse_escape("hello"), "hello");
        assert_eq!(clickhouse_escape("o'brien"), "o\\'brien");
        assert_eq!(clickhouse_escape("a\\b"), "a\\\\b");
    }
}
