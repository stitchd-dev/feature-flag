//! Evaluation analytics route — time-bucketed flag evaluation statistics.
//!
//! GET /v1/projects/{project_id}/flags/{flag_id}/eval-stats
//!
//! Query parameters:
//! - `from` — RFC 3339 start timestamp
//! - `to`   — RFC 3339 end timestamp
//! - `granularity` — `"hour"` | `"day"` (auto-overridden to `"day"` when range > 24 h)
//!
//! Validation:
//! - Range > 90 days → HTTP 400
//! - Range > 24 h → granularity forced to `"day"` regardless of query param

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::error::GatewayError;
use crate::state::GatewayState;

/// Query parameters for eval-stats.
#[derive(Debug, Deserialize)]
pub struct EvalStatsQuery {
    /// Start of the time range (RFC 3339).
    pub from: DateTime<Utc>,
    /// End of the time range (RFC 3339).
    pub to: DateTime<Utc>,
    /// Requested granularity: `"hour"` or `"day"`.
    #[serde(default = "default_granularity")]
    pub granularity: String,
}

fn default_granularity() -> String {
    "hour".to_string()
}

/// One time bucket in the response.
#[derive(Debug, Serialize)]
pub struct EvalStatsBucket {
    /// Bucket start timestamp (ISO 8601 UTC).
    pub ts: String,
    /// Total evaluations in this bucket.
    pub total: u64,
    /// Count per variant key.
    pub by_variant: HashMap<String, u64>,
    /// Evaluations where the flag was disabled.
    pub disabled_count: u64,
    /// Approximate distinct context keys seen in this bucket.
    pub unique_context_keys: u64,
}

/// Response body for eval-stats.
#[derive(Debug, Serialize)]
pub struct EvalStatsResponse {
    /// Resolved granularity actually used (may differ from requested).
    pub granularity: String,
    /// Time-ordered list of bucketed statistics.
    pub buckets: Vec<EvalStatsBucket>,
}

/// ClickHouse row returned by the analytics query.
#[derive(clickhouse::Row, Deserialize)]
struct EvalStatRow {
    /// Bucket start as Unix timestamp (seconds).
    ts: i64,
    /// Variant key for this row.
    variant_key: String,
    /// Whether the flag was disabled.
    is_disabled: u8,
    /// Evaluation count in this bucket.
    count: u64,
    /// Approximate distinct context_key count in this bucket.
    unique_ctx: u64,
}


/// Resolve the effective granularity.
///
/// Forces `"day"` when the requested range exceeds 24 hours.
pub fn resolve_granularity(from: DateTime<Utc>, to: DateTime<Utc>, requested: &str) -> String {
    if to - from > Duration::hours(24) {
        "day".to_string()
    } else {
        requested.to_string()
    }
}

/// GET /v1/projects/{project_id}/flags/{flag_id}/eval-stats
pub async fn get_eval_stats(
    State(state): State<Arc<GatewayState>>,
    Path((_project_id, flag_id)): Path<(String, String)>,
    Query(params): Query<EvalStatsQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let from = params.from;
    let to = params.to;

    // Reject ranges > 90 days.
    if to - from > Duration::days(90) {
        return Err(GatewayError::BadRequest(
            "Time range must not exceed 90 days".to_string(),
        ));
    }

    if to <= from {
        return Err(GatewayError::BadRequest(
            "'to' must be after 'from'".to_string(),
        ));
    }

    let granularity = resolve_granularity(from, to, &params.granularity);

    let bucket_fn = if granularity == "day" {
        "toStartOfDay"
    } else {
        "toStartOfHour"
    };

    let from_ms = from.timestamp_millis();
    let to_ms = to.timestamp_millis();
    let flag_id_str = flag_id.clone();

    let sql = format!(
        r"
        SELECT
            toUnixTimestamp({bucket_fn}(evaluated_at, 'UTC')) AS ts,
            variant_key,
            toUInt8(is_disabled)                              AS is_disabled,
            COUNT(*)                                          AS count,
            uniqApprox(context_key)                          AS unique_ctx
        FROM flag_evaluation_log
        WHERE flag_id  = '{flag_id_str}'
          AND evaluated_at >= fromUnixTimestamp64Milli({from_ms})
          AND evaluated_at <  fromUnixTimestamp64Milli({to_ms})
        GROUP BY ts, variant_key, is_disabled
        ORDER BY ts ASC
        "
    );

    let rows = state
        .ch_client
        .query(&sql)
        .fetch_all::<EvalStatRow>()
        .await
        .map_err(|e| GatewayError::Upstream(format!("ClickHouse error: {e}")))?;

    // Merge rows into time-bucket map.
    let mut bucket_map: std::collections::BTreeMap<i64, EvalStatsBucket> =
        std::collections::BTreeMap::new();

    for row in rows {
        let bucket = bucket_map.entry(row.ts).or_insert_with(|| EvalStatsBucket {
            ts: DateTime::from_timestamp(row.ts, 0)
                .unwrap_or_default()
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            total: 0,
            by_variant: HashMap::new(),
            disabled_count: 0,
            unique_context_keys: 0,
        });

        bucket.total += row.count;
        *bucket.by_variant.entry(row.variant_key).or_insert(0) += row.count;
        if row.is_disabled != 0 {
            bucket.disabled_count += row.count;
        }
        // Take max of approximate uniq counts across sub-rows for this bucket.
        bucket.unique_context_keys = bucket.unique_context_keys.max(row.unique_ctx);
    }

    Ok((
        StatusCode::OK,
        Json(EvalStatsResponse {
            granularity,
            buckets: bucket_map.into_values().collect(),
        }),
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn granularity_stays_hour_within_24h() {
        let from = ts(2026, 5, 1, 0);
        let to = ts(2026, 5, 1, 23);
        assert_eq!(resolve_granularity(from, to, "hour"), "hour");
    }

    #[test]
    fn granularity_auto_switches_to_day_when_range_exceeds_24h() {
        let from = ts(2026, 5, 1, 0);
        let to = ts(2026, 5, 3, 0); // 48 h
        assert_eq!(resolve_granularity(from, to, "hour"), "day");
    }

    #[test]
    fn granularity_day_request_stays_day_within_24h() {
        let from = ts(2026, 5, 1, 0);
        let to = ts(2026, 5, 1, 12);
        assert_eq!(resolve_granularity(from, to, "day"), "day");
    }

    #[test]
    fn granularity_exactly_24h_stays_hour() {
        let from = ts(2026, 5, 1, 0);
        let to = ts(2026, 5, 2, 0); // exactly 24 h — not > 24 h
        assert_eq!(resolve_granularity(from, to, "hour"), "hour");
    }

    #[test]
    fn granularity_30_day_range_forces_day() {
        let from = ts(2026, 4, 1, 0);
        let to = ts(2026, 5, 1, 0);
        assert_eq!(resolve_granularity(from, to, "hour"), "day");
    }
}
