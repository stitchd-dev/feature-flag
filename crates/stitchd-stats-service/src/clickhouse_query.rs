//! Time-bounded ClickHouse event query for experiment stats computation.
//!
//! Queries the `events` table bounded to an experiment iteration's time window:
//! `[iteration.started_at, iteration.ended_at]` — or `NOW()` when the iteration
//! is still active (i.e. `ended_at` is `None`).

use chrono::{DateTime, Utc};
use clickhouse::Client;
use serde::Deserialize;
use uuid::Uuid;

/// Aggregated event counts per (variant_key, metric_key) for an iteration window.
#[derive(Debug, Clone, Deserialize, clickhouse::Row)]
pub struct EventAggregate {
    pub variant_key: String,
    pub metric_key: String,
    pub event_count: u64,
}

/// Determine the upper time bound for an iteration query.
///
/// Returns `ended_at` when the iteration has concluded, or the current UTC time
/// when the iteration is still active.
pub fn upper_bound(ended_at: Option<DateTime<Utc>>) -> DateTime<Utc> {
    ended_at.unwrap_or_else(Utc::now)
}

/// Fetch aggregated event counts from ClickHouse for the given iteration window.
///
/// Groups by `variant_key` (the value of the `"variant"` context key) and
/// `metric_key`, counting events within the iteration's time bounds.
pub async fn fetch_event_aggregates(
    client: &Client,
    env_id: Uuid,
    metric_keys: &[String],
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
) -> Result<Vec<EventAggregate>, clickhouse::error::Error> {
    if metric_keys.is_empty() {
        return Ok(vec![]);
    }

    let upper = upper_bound(ended_at);

    // Format UUIDs and timestamps for inline substitution.
    // The clickhouse crate's query().bind() API doesn't support Array/IN params,
    // so we build the IN list as a literal and use bind() only for scalar values.
    let metric_in = metric_keys
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");

    let env_id_str = env_id.to_string();
    let lower_ms = started_at.timestamp_millis();
    let upper_ms = upper.timestamp_millis();

    let sql = format!(
        r"
        SELECT
            arrayElement(arrayFilter(x -> x.1 = 'variant', contexts), 1).2 AS variant_key,
            metric_key,
            COUNT(*) AS event_count
        FROM events
        WHERE env_id = '{env_id_str}'
          AND metric_key IN ({metric_in})
          AND timestamp >= fromUnixTimestamp64Milli({lower_ms})
          AND timestamp <  fromUnixTimestamp64Milli({upper_ms})
        GROUP BY variant_key, metric_key
        HAVING variant_key != ''
        "
    );

    client.query(&sql).fetch_all::<EventAggregate>().await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    #[test]
    fn test_upper_bound_uses_ended_at_when_present() {
        let ended = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        assert_eq!(upper_bound(Some(ended)), ended);
    }

    #[test]
    fn test_upper_bound_uses_now_when_none() {
        let before = Utc::now();
        let result = upper_bound(None);
        let after = Utc::now();
        assert!(
            result >= before && result <= after,
            "upper_bound(None) should be approximately now"
        );
    }

    #[test]
    fn test_upper_bound_ended_at_can_be_past() {
        let past = Utc::now() - Duration::hours(2);
        assert_eq!(upper_bound(Some(past)), past);
    }

    #[test]
    fn test_upper_bound_ended_at_can_be_future() {
        let future = Utc::now() + Duration::hours(1);
        assert_eq!(upper_bound(Some(future)), future);
    }

    #[test]
    fn test_lower_bound_is_started_at() {
        // The lower bound is simply iteration.started_at — verify that the
        // caller passes it through unchanged (no rounding or truncation).
        let started_at = Utc.with_ymd_and_hms(2026, 4, 1, 8, 30, 0).unwrap();
        let ended_at = Utc.with_ymd_and_hms(2026, 4, 10, 8, 30, 0).unwrap();

        // upper_bound returns ended_at → the window is [started_at, ended_at]
        assert_eq!(upper_bound(Some(ended_at)), ended_at);
        // started_at is the caller's direct input; no transformation is applied.
        // Verify it round-trips through timestamp_millis() without truncation.
        let ms = started_at.timestamp_millis();
        let recovered = DateTime::from_timestamp_millis(ms).unwrap();
        assert_eq!(recovered, started_at);
    }

    #[tokio::test]
    async fn test_fetch_empty_metric_keys_returns_empty_without_db() {
        // The function short-circuits before touching ClickHouse when metric_keys is empty.
        let client = clickhouse::Client::default();
        let env_id = uuid::Uuid::new_v4();
        let started_at = Utc::now() - Duration::hours(1);
        let result = fetch_event_aggregates(&client, env_id, &[], started_at, None).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_upper_bound_is_strictly_after_lower_when_ended() {
        let started_at = Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap();
        let ended_at = Utc.with_ymd_and_hms(2026, 4, 10, 0, 0, 0).unwrap();
        assert!(
            upper_bound(Some(ended_at)) > started_at,
            "ended_at must be after started_at"
        );
    }
}
