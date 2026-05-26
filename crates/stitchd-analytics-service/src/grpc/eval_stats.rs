//! GetEvalStats gRPC handler — ported from gateway's eval_stats route.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use tonic::{Request, Response, Status};

use stitchd_proto::analytics::v1::{EvalStatsBucket, GetEvalStatsRequest, GetEvalStatsResponse};

/// Resolve the effective granularity — forces "day" when range exceeds 24 hours.
pub fn resolve_granularity(from: DateTime<Utc>, to: DateTime<Utc>, requested: &str) -> String {
    if to - from > Duration::hours(24) {
        "day".to_string()
    } else {
        requested.to_string()
    }
}

/// Build parameterized SQL for eval-stats.
///
/// `bucket_fn` must be one of the two trusted strings produced by
/// [`resolve_granularity`] (`"toStartOfDay"` / `"toStartOfHour"`).
///
/// # Bind order
/// 1. `flag_id` — UUID
/// 2. `from_ms` — i64 millisecond epoch
/// 3. `to_ms`   — i64 millisecond epoch
pub fn build_eval_stats_sql(bucket_fn: &str) -> String {
    format!(
        r"
        SELECT
            toUnixTimestamp({bucket_fn}(evaluated_at, 'UTC')) AS ts,
            variant_key,
            toUInt8(NOT targeting_on)                         AS is_disabled,
            COUNT(*)                                          AS count,
            uniq(context_key)                                AS unique_ctx
        FROM flag_evaluation_log
        WHERE flag_id  = ?
          AND evaluated_at >= fromUnixTimestamp64Milli(?)
          AND evaluated_at <  fromUnixTimestamp64Milli(?)
        GROUP BY ts, variant_key, is_disabled
        ORDER BY ts ASC
        "
    )
}

#[derive(clickhouse::Row, Deserialize)]
struct EvalStatRow {
    ts: u32,
    variant_key: String,
    is_disabled: u8,
    count: u64,
    unique_ctx: u64,
}

pub async fn handle_get_eval_stats(
    ch_client: &Arc<clickhouse::Client>,
    request: Request<GetEvalStatsRequest>,
) -> Result<Response<GetEvalStatsResponse>, Status> {
    let req = request.into_inner();

    let flag_uuid = req
        .flag_id
        .parse::<uuid::Uuid>()
        .map_err(|_| Status::invalid_argument(format!("invalid flag_id: {}", req.flag_id)))?;

    let from = req
        .from
        .parse::<DateTime<Utc>>()
        .map_err(|_| Status::invalid_argument("invalid 'from' timestamp"))?;

    let to = req
        .to
        .parse::<DateTime<Utc>>()
        .map_err(|_| Status::invalid_argument("invalid 'to' timestamp"))?;

    if to <= from {
        return Err(Status::invalid_argument("'to' must be after 'from'"));
    }

    if to - from > Duration::days(90) {
        return Err(Status::invalid_argument(
            "time range must not exceed 90 days",
        ));
    }

    let granularity = resolve_granularity(from, to, &req.granularity);
    let bucket_fn = if granularity == "day" {
        "toStartOfDay"
    } else {
        "toStartOfHour"
    };

    let sql = build_eval_stats_sql(bucket_fn);

    let rows = ch_client
        .query(&sql)
        .bind(flag_uuid)
        .bind(from.timestamp_millis())
        .bind(to.timestamp_millis())
        .fetch_all::<EvalStatRow>()
        .await
        .map_err(|e| Status::internal(format!("ClickHouse error: {e}")))?;

    let mut bucket_map: BTreeMap<i64, EvalStatsBucket> = BTreeMap::new();

    for row in rows {
        let bucket = bucket_map
            .entry(row.ts as i64)
            .or_insert_with(|| EvalStatsBucket {
                ts: DateTime::from_timestamp(row.ts as i64, 0)
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
        bucket.unique_context_keys = bucket.unique_context_keys.max(row.unique_ctx);
    }

    Ok(Response::new(GetEvalStatsResponse {
        granularity,
        buckets: bucket_map.into_values().collect(),
    }))
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
        assert_eq!(
            resolve_granularity(ts(2026, 5, 1, 0), ts(2026, 5, 1, 23), "hour"),
            "hour"
        );
    }

    #[test]
    fn granularity_auto_switches_to_day_over_24h() {
        assert_eq!(
            resolve_granularity(ts(2026, 5, 1, 0), ts(2026, 5, 3, 0), "hour"),
            "day"
        );
    }

    #[test]
    fn granularity_exactly_24h_stays_hour() {
        assert_eq!(
            resolve_granularity(ts(2026, 5, 1, 0), ts(2026, 5, 2, 0), "hour"),
            "hour"
        );
    }

    #[test]
    fn granularity_30_day_range_forces_day() {
        assert_eq!(
            resolve_granularity(ts(2026, 4, 1, 0), ts(2026, 5, 1, 0), "hour"),
            "day"
        );
    }

    #[test]
    fn sql_has_three_bind_slots() {
        let sql = build_eval_stats_sql("toStartOfHour");
        assert_eq!(sql.chars().filter(|&c| c == '?').count(), 3);
    }

    #[test]
    fn sql_hour_uses_tostartofhour() {
        assert!(build_eval_stats_sql("toStartOfHour").contains("toStartOfHour"));
    }

    #[test]
    fn sql_day_uses_tostartofday() {
        assert!(build_eval_stats_sql("toStartOfDay").contains("toStartOfDay"));
    }

    #[test]
    fn sql_uses_bind_not_interpolation() {
        let sql = build_eval_stats_sql("toStartOfHour");
        assert!(sql.contains('?'));
        assert!(!sql.contains("flag_uuid"));
        assert!(!sql.contains("from_ms"));
        assert!(!sql.contains("to_ms"));
    }

    #[tokio::test]
    async fn rejects_invalid_flag_id() {
        let ch = Arc::new(clickhouse::Client::default().with_url("http://127.0.0.1:1"));
        let req = Request::new(GetEvalStatsRequest {
            flag_id: "not-a-uuid".into(),
            project_id: String::new(),
            from: "2026-05-01T00:00:00Z".into(),
            to: "2026-05-02T00:00:00Z".into(),
            granularity: "hour".into(),
        });
        let err = handle_get_eval_stats(&ch, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn rejects_invalid_from_timestamp() {
        let ch = Arc::new(clickhouse::Client::default().with_url("http://127.0.0.1:1"));
        let req = Request::new(GetEvalStatsRequest {
            flag_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            project_id: String::new(),
            from: "not-a-date".into(),
            to: "2026-05-02T00:00:00Z".into(),
            granularity: "hour".into(),
        });
        let err = handle_get_eval_stats(&ch, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn rejects_to_before_from() {
        let ch = Arc::new(clickhouse::Client::default().with_url("http://127.0.0.1:1"));
        let req = Request::new(GetEvalStatsRequest {
            flag_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            project_id: String::new(),
            from: "2026-05-02T00:00:00Z".into(),
            to: "2026-05-01T00:00:00Z".into(),
            granularity: "hour".into(),
        });
        let err = handle_get_eval_stats(&ch, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn rejects_range_over_90_days() {
        let ch = Arc::new(clickhouse::Client::default().with_url("http://127.0.0.1:1"));
        let req = Request::new(GetEvalStatsRequest {
            flag_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            project_id: String::new(),
            from: "2026-01-01T00:00:00Z".into(),
            to: "2026-05-01T00:00:00Z".into(),
            granularity: "hour".into(),
        });
        let err = handle_get_eval_stats(&ch, req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("90 days"));
    }
}
