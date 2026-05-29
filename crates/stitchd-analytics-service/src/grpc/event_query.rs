//! Event-firings + stats handlers — back the admin UI's EventDetail + TestEventWidget.
//!
//! Both endpoints query the canonical `events` ClickHouse table, scoped by
//! the trusted `env_id` resolved at the gateway (JWT tier) and the
//! `event_key` from the URL path. They are read-only — no writes, no PG
//! lookups; the gateway has already verified the caller is authorised.
//!
//! # Limits
//! - `GetEventFirings.limit` defaults to 50, capped at 200.
//! - `GetEventStats.days` defaults to 14, capped at 90.
//!
//! # Query patterns
//! See `conductor/patterns.md` — *ClickHouse parameterized queries*: bind
//! values via `?` placeholders rather than `format!()` interpolation.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tonic::{Request, Response, Status};

use stitchd_proto::analytics::v1::{
    EventFiring, EventStatsBucket, GetEventFiringsRequest, GetEventFiringsResponse,
    GetEventStatsRequest, GetEventStatsResponse,
};

/// Default page size for `GetEventFirings.limit` when 0.
pub const DEFAULT_FIRINGS_LIMIT: u32 = 50;
/// Hard cap for `GetEventFirings.limit` (per spec).
pub const MAX_FIRINGS_LIMIT: u32 = 200;

/// Default window for `GetEventStats.days` when 0.
pub const DEFAULT_STATS_DAYS: u32 = 14;
/// Hard cap for `GetEventStats.days`.
pub const MAX_STATS_DAYS: u32 = 90;

/// Resolve the requested limit against the (default, cap) policy.
#[must_use]
pub fn resolve_limit(requested: u32) -> u32 {
    let n = if requested == 0 {
        DEFAULT_FIRINGS_LIMIT
    } else {
        requested
    };
    n.min(MAX_FIRINGS_LIMIT)
}

/// Resolve the requested day-window against the (default, cap) policy.
#[must_use]
pub fn resolve_days(requested: u32) -> u32 {
    let n = if requested == 0 {
        DEFAULT_STATS_DAYS
    } else {
        requested
    };
    n.min(MAX_STATS_DAYS)
}

/// SQL for `GetEventFirings`. Reads the first contexts pair (dimension-1)
/// and all three nullable value columns; the handler renders the typed
/// `value_json` from whichever column is non-null.
///
/// # Bind order
/// 1. `env_id` — UUID
/// 2. `event_key` — String
/// 3. `limit` — u32
#[must_use]
pub fn build_firings_sql() -> &'static str {
    // `contexts` ships as `Array(Tuple(String, String))` over the
    // RowBinary wire (deserialised into `Vec<(String, String)>` on the
    // Rust side, then flattened into the proto's `map<string, string>`
    // by the handler).
    r"
    SELECT
        contexts                                                 AS contexts,
        value_bool,
        value_int,
        value_double,
        properties,
        toUnixTimestamp64Milli(occurred_at)                      AS occurred_at_ms,
        toUnixTimestamp64Milli(timestamp)                        AS ingested_at_ms
    FROM events
    WHERE env_id     = ?
      AND metric_key = ?
    ORDER BY timestamp DESC
    LIMIT ?
    "
}

/// SQL for `GetEventStats`. Buckets to UTC day starts.
///
/// # Bind order
/// 1. `env_id` — UUID
/// 2. `event_key` — String
/// 3. `days` — u32 (passed as `INTERVAL ? DAY` via `toIntervalDay`)
#[must_use]
pub fn build_stats_sql() -> &'static str {
    r"
    SELECT
        toUnixTimestamp(toStartOfDay(timestamp, 'UTC'))   AS day_ts,
        count()                                           AS count,
        uniq(contexts[1].2)                               AS unique_context_keys
    FROM events
    WHERE env_id     = ?
      AND metric_key = ?
      AND timestamp >= now() - toIntervalDay(?)
    GROUP BY day_ts
    ORDER BY day_ts ASC
    "
}

#[derive(clickhouse::Row, Deserialize)]
struct FiringRow {
    /// Full multi-context attribution. `Array(Tuple(String, String))`
    /// in CH → `Vec<(String, String)>` over the RowBinary wire; the
    /// handler flattens into the proto's `map<string, string>`.
    contexts: Vec<(String, String)>,
    value_bool: Option<bool>,
    value_int: Option<i64>,
    value_double: Option<f64>,
    /// `properties` is `Map(String, String)` — RowBinary deserialises as a
    /// `Vec<(String, String)>` (same pattern as the `EventRow` write side).
    properties: Vec<(String, String)>,
    occurred_at_ms: i64,
    ingested_at_ms: i64,
}

#[derive(clickhouse::Row, Deserialize)]
struct StatsRow {
    day_ts: u32,
    count: u64,
    unique_context_keys: u64,
}

/// Convert the three nullable typed columns into a serialised JSON scalar.
/// Returns an empty string when all three are null (occurrence-only event).
fn render_value_json(
    value_bool: Option<bool>,
    value_int: Option<i64>,
    value_double: Option<f64>,
) -> String {
    if let Some(b) = value_bool {
        serde_json::Value::Bool(b).to_string()
    } else if let Some(i) = value_int {
        serde_json::Value::from(i).to_string()
    } else if let Some(d) = value_double {
        serde_json::Value::from(d).to_string()
    } else {
        String::new()
    }
}

/// Convert a list of `(k, v)` pairs into a serialised JSON object. Always
/// emits `{}` for empty maps (never an empty string) so callers can rely on
/// JSON parsing.
fn render_properties_json(pairs: &[(String, String)]) -> String {
    let obj: serde_json::Map<String, serde_json::Value> = pairs
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    serde_json::Value::Object(obj).to_string()
}

fn ms_to_rfc3339(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

fn day_ts_to_rfc3339(secs: u32) -> String {
    DateTime::<Utc>::from_timestamp(i64::from(secs), 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Handle `GetEventFirings`.
#[allow(clippy::result_large_err)] // tonic::Status is external.
pub async fn handle_get_event_firings(
    ch_client: &Arc<clickhouse::Client>,
    request: Request<GetEventFiringsRequest>,
) -> Result<Response<GetEventFiringsResponse>, Status> {
    let req = request.into_inner();

    let env_uuid = req
        .env_id
        .parse::<uuid::Uuid>()
        .map_err(|_| Status::invalid_argument(format!("invalid env_id: {}", req.env_id)))?;

    if req.event_key.is_empty() {
        return Err(Status::invalid_argument("event_key must be non-empty"));
    }

    let limit = resolve_limit(req.limit);

    let rows = ch_client
        .query(build_firings_sql())
        .bind(env_uuid)
        .bind(&req.event_key)
        .bind(limit)
        .fetch_all::<FiringRow>()
        .await
        .map_err(|e| Status::internal(format!("ClickHouse error: {e}")))?;

    // Flatten the CH `Vec<(type, key)>` into the proto's
    // `map<string, string>`. If a single event had multiple firings
    // with the same context_type (e.g. two different users), the map
    // collapses to one — but the SDK contract enforces a single
    // dimension per type per event, so this is by design. Last-write-
    // wins on conflict (HashMap::insert semantic).
    let firings: Vec<EventFiring> = rows
        .into_iter()
        .map(|r| EventFiring {
            value_json: render_value_json(r.value_bool, r.value_int, r.value_double),
            properties_json: render_properties_json(&r.properties),
            occurred_at: ms_to_rfc3339(r.occurred_at_ms),
            ingested_at: ms_to_rfc3339(r.ingested_at_ms),
            contexts: r.contexts.into_iter().collect(),
        })
        .collect();

    Ok(Response::new(GetEventFiringsResponse { firings }))
}

/// Handle `GetEventStats`.
#[allow(clippy::result_large_err)] // tonic::Status is external.
pub async fn handle_get_event_stats(
    ch_client: &Arc<clickhouse::Client>,
    request: Request<GetEventStatsRequest>,
) -> Result<Response<GetEventStatsResponse>, Status> {
    let req = request.into_inner();

    let env_uuid = req
        .env_id
        .parse::<uuid::Uuid>()
        .map_err(|_| Status::invalid_argument(format!("invalid env_id: {}", req.env_id)))?;

    if req.event_key.is_empty() {
        return Err(Status::invalid_argument("event_key must be non-empty"));
    }

    let days = resolve_days(req.days);

    let rows = ch_client
        .query(build_stats_sql())
        .bind(env_uuid)
        .bind(&req.event_key)
        .bind(days)
        .fetch_all::<StatsRow>()
        .await
        .map_err(|e| Status::internal(format!("ClickHouse error: {e}")))?;

    let buckets: Vec<EventStatsBucket> = rows
        .into_iter()
        .map(|r| EventStatsBucket {
            day: day_ts_to_rfc3339(r.day_ts),
            count: r.count,
            unique_context_keys: r.unique_context_keys,
        })
        .collect();

    Ok(Response::new(GetEventStatsResponse { buckets }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as JsonValue;

    // ── Pure helpers ─────────────────────────────────────────────────────────

    #[test]
    fn resolve_limit_zero_uses_default() {
        assert_eq!(resolve_limit(0), DEFAULT_FIRINGS_LIMIT);
    }

    #[test]
    fn resolve_limit_caps_at_max() {
        assert_eq!(resolve_limit(500), MAX_FIRINGS_LIMIT);
    }

    #[test]
    fn resolve_limit_in_range_passes_through() {
        assert_eq!(resolve_limit(17), 17);
    }

    #[test]
    fn resolve_days_zero_uses_default() {
        assert_eq!(resolve_days(0), DEFAULT_STATS_DAYS);
    }

    #[test]
    fn resolve_days_caps_at_max() {
        assert_eq!(resolve_days(365), MAX_STATS_DAYS);
    }

    #[test]
    fn render_value_json_bool() {
        assert_eq!(render_value_json(Some(true), None, None), "true");
        assert_eq!(render_value_json(Some(false), None, None), "false");
    }

    #[test]
    fn render_value_json_int() {
        assert_eq!(render_value_json(None, Some(42), None), "42");
    }

    #[test]
    fn render_value_json_double() {
        let s = render_value_json(None, None, Some(1.5));
        let v: JsonValue = serde_json::from_str(&s).unwrap();
        assert!((v.as_f64().unwrap() - 1.5).abs() < 1e-9);
    }

    #[test]
    fn render_value_json_empty_when_all_none() {
        assert_eq!(render_value_json(None, None, None), "");
    }

    #[test]
    fn render_properties_json_empty_is_object() {
        assert_eq!(render_properties_json(&[]), "{}");
    }

    #[test]
    fn render_properties_json_round_trip() {
        let pairs = vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
        ];
        let s = render_properties_json(&pairs);
        let parsed: JsonValue = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed["a"], "1");
        assert_eq!(parsed["b"], "2");
    }

    #[test]
    fn firings_sql_has_three_bind_slots() {
        assert_eq!(build_firings_sql().chars().filter(|&c| c == '?').count(), 3);
    }

    #[test]
    fn firings_sql_orders_desc() {
        assert!(build_firings_sql().contains("ORDER BY timestamp DESC"));
    }

    #[test]
    fn firings_sql_reads_events() {
        assert!(build_firings_sql().contains("FROM events"));
    }

    #[test]
    fn stats_sql_has_three_bind_slots() {
        assert_eq!(build_stats_sql().chars().filter(|&c| c == '?').count(), 3);
    }

    #[test]
    fn stats_sql_buckets_by_day() {
        assert!(build_stats_sql().contains("toStartOfDay(timestamp, 'UTC')"));
    }

    #[test]
    fn stats_sql_uses_uniq() {
        assert!(build_stats_sql().contains("uniq(contexts[1].2)"));
    }

    // ── Argument validation (don't need real ClickHouse) ─────────────────────

    fn unreachable_ch_client() -> Arc<clickhouse::Client> {
        Arc::new(clickhouse::Client::default().with_url("http://127.0.0.1:1"))
    }

    #[tokio::test]
    async fn firings_rejects_invalid_env_id() {
        let req = Request::new(GetEventFiringsRequest {
            env_id: "not-a-uuid".into(),
            event_key: "click".into(),
            limit: 10,
        });
        let err = handle_get_event_firings(&unreachable_ch_client(), req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn firings_rejects_empty_event_key() {
        let req = Request::new(GetEventFiringsRequest {
            env_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            event_key: String::new(),
            limit: 10,
        });
        let err = handle_get_event_firings(&unreachable_ch_client(), req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn stats_rejects_invalid_env_id() {
        let req = Request::new(GetEventStatsRequest {
            env_id: "not-a-uuid".into(),
            event_key: "click".into(),
            days: 7,
        });
        let err = handle_get_event_stats(&unreachable_ch_client(), req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn stats_rejects_empty_event_key() {
        let req = Request::new(GetEventStatsRequest {
            env_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            event_key: String::new(),
            days: 7,
        });
        let err = handle_get_event_stats(&unreachable_ch_client(), req)
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // ── End-to-end with real ClickHouse ──────────────────────────────────────
    //
    // Per `conductor/patterns.md`, integration tests use a shared `stitchd`
    // ClickHouse database; each test scopes its writes to a fresh `env_id`
    // to avoid cross-test interference.

    use uuid::Uuid;

    fn make_ch_client() -> clickhouse::Client {
        clickhouse::Client::default()
            .with_url("http://localhost:8123")
            .with_user("stitchd")
            .with_password("stitchd")
            .with_database("stitchd")
    }

    async fn ensure_migrations() {
        let client = make_ch_client();
        stitchd_event_writer::migrations::run(&client)
            .await
            .expect("CH migrations must apply");
    }

    /// Insert a single firing row directly into `events` for testing.
    /// `timestamp_ms` is both the broker-side `timestamp` and the wall-clock
    /// `occurred_at` (sufficient for grouping / ordering assertions).
    async fn insert_event(
        client: &clickhouse::Client,
        env_id: Uuid,
        event_key: &str,
        context_key: &str,
        timestamp_ms: i64,
        value_int: Option<i64>,
    ) {
        use clickhouse::Row;
        use serde::Serialize;

        #[derive(Serialize, Row)]
        struct InsertRow<'a> {
            #[serde(with = "clickhouse::serde::uuid")]
            env_id: Uuid,
            contexts: Vec<(String, String)>,
            metric_key: &'a str,
            value_bool: Option<bool>,
            value_int: Option<i64>,
            value_double: Option<f64>,
            timestamp: i64,
            properties: Vec<(String, String)>,
            occurred_at: i64,
        }

        let mut insert = client
            .insert::<InsertRow>("events")
            .await
            .expect("insert init");
        insert
            .write(&InsertRow {
                env_id,
                contexts: vec![("user".to_string(), context_key.to_string())],
                metric_key: event_key,
                value_bool: None,
                value_int,
                value_double: None,
                timestamp: timestamp_ms,
                properties: vec![],
                occurred_at: timestamp_ms,
            })
            .await
            .expect("row write");
        insert.end().await.expect("insert end");
    }

    /// Trigger a synchronous merge so freshly-inserted rows are queryable
    /// from the same connection. Mirrors the pattern in
    /// `stitchd-event-writer::tests::clickhouse_views`.
    async fn wait_for_merge(client: &clickhouse::Client) {
        let _ = client.query("OPTIMIZE TABLE events FINAL").execute().await;
    }

    #[tokio::test]
    async fn test_get_event_firings_returns_recent_rows() {
        ensure_migrations().await;
        let client = make_ch_client();
        let env_id = Uuid::new_v4();
        let now = Utc::now().timestamp_millis();

        // Insert 5 events at increasing timestamps. `events` ORDER BY
        // includes timestamp; DESC scan must surface the newest first.
        for i in 0..5 {
            insert_event(
                &client,
                env_id,
                "click",
                &format!("u{i}"),
                now + i64::from(i) * 1000,
                Some(i64::from(i)),
            )
            .await;
        }
        wait_for_merge(&client).await;

        let ch_arc = Arc::new(client);
        let req = Request::new(GetEventFiringsRequest {
            env_id: env_id.to_string(),
            event_key: "click".into(),
            limit: 0, // exercises default
        });
        let resp = handle_get_event_firings(&ch_arc, req)
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.firings.len(), 5);
        // Newest first: user "u4" then "u3" ... "u0".
        assert_eq!(
            resp.firings[0].contexts.get("user").map(String::as_str),
            Some("u4")
        );
        assert_eq!(
            resp.firings[4].contexts.get("user").map(String::as_str),
            Some("u0")
        );
        // Spot-check schema mapping.
        assert_eq!(resp.firings[0].value_json, "4");
        assert_eq!(resp.firings[0].properties_json, "{}");
        assert!(resp.firings[0].occurred_at.ends_with('Z'));
        assert!(resp.firings[0].ingested_at.ends_with('Z'));
    }

    #[tokio::test]
    async fn test_get_event_firings_respects_limit() {
        ensure_migrations().await;
        let client = make_ch_client();
        let env_id = Uuid::new_v4();
        let now = Utc::now().timestamp_millis();

        for i in 0..10 {
            insert_event(
                &client,
                env_id,
                "view",
                &format!("u{i}"),
                now + i64::from(i) * 1000,
                None,
            )
            .await;
        }
        wait_for_merge(&client).await;

        let ch_arc = Arc::new(client);
        let req = Request::new(GetEventFiringsRequest {
            env_id: env_id.to_string(),
            event_key: "view".into(),
            limit: 3,
        });
        let resp = handle_get_event_firings(&ch_arc, req)
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.firings.len(), 3);
        // Value column was `None` so `value_json` must be the empty marker.
        assert_eq!(resp.firings[0].value_json, "");
    }

    #[tokio::test]
    async fn test_get_event_firings_empty_returns_empty_response() {
        ensure_migrations().await;
        let env_id = Uuid::new_v4(); // no rows for this env
        let ch_arc = Arc::new(make_ch_client());
        let req = Request::new(GetEventFiringsRequest {
            env_id: env_id.to_string(),
            event_key: "never_fired".into(),
            limit: 50,
        });
        let resp = handle_get_event_firings(&ch_arc, req)
            .await
            .unwrap()
            .into_inner();
        assert!(resp.firings.is_empty());
    }

    #[tokio::test]
    async fn test_get_event_stats_buckets_by_day() {
        ensure_migrations().await;
        let client = make_ch_client();
        let env_id = Uuid::new_v4();
        let now = Utc::now();

        // Three distinct UTC-day buckets within the last 90 days.
        // Use noon-UTC of {now, now-1d, now-2d} so DST quirks don't matter.
        let day_0 = now
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let day_1 = day_0 - 24 * 60 * 60 * 1000;
        let day_2 = day_0 - 2 * 24 * 60 * 60 * 1000;

        // Two events on day_0, one on day_1, one on day_2 — distinct context_keys.
        insert_event(&client, env_id, "checkout", "alice", day_0, None).await;
        insert_event(&client, env_id, "checkout", "bob", day_0, None).await;
        insert_event(&client, env_id, "checkout", "alice", day_1, None).await;
        insert_event(&client, env_id, "checkout", "carol", day_2, None).await;
        wait_for_merge(&client).await;

        let ch_arc = Arc::new(client);
        let req = Request::new(GetEventStatsRequest {
            env_id: env_id.to_string(),
            event_key: "checkout".into(),
            days: 30,
        });
        let resp = handle_get_event_stats(&ch_arc, req)
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.buckets.len(), 3, "expected 3 day-buckets");
        // ASC order — day_2 (oldest) first, day_0 (newest) last.
        let total: u64 = resp.buckets.iter().map(|b| b.count).sum();
        assert_eq!(total, 4);

        // Day_0 must have count=2 (alice + bob), unique=2.
        let last = resp.buckets.last().unwrap();
        assert_eq!(last.count, 2);
        assert_eq!(last.unique_context_keys, 2);
    }

    #[tokio::test]
    async fn test_get_event_stats_unique_context_keys_dedupes() {
        ensure_migrations().await;
        let client = make_ch_client();
        let env_id = Uuid::new_v4();
        let now = Utc::now();
        let day_0 = now
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();

        // Same context_key fires 3× within the same UTC day.
        insert_event(&client, env_id, "ping", "u1", day_0, None).await;
        insert_event(&client, env_id, "ping", "u1", day_0 + 1000, None).await;
        insert_event(&client, env_id, "ping", "u1", day_0 + 2000, None).await;
        // One other context_key, same day.
        insert_event(&client, env_id, "ping", "u2", day_0 + 3000, None).await;
        wait_for_merge(&client).await;

        let ch_arc = Arc::new(client);
        let req = Request::new(GetEventStatsRequest {
            env_id: env_id.to_string(),
            event_key: "ping".into(),
            days: 7,
        });
        let resp = handle_get_event_stats(&ch_arc, req)
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.buckets.len(), 1);
        assert_eq!(resp.buckets[0].count, 4);
        assert_eq!(resp.buckets[0].unique_context_keys, 2, "u1 dedupes");
    }
}
