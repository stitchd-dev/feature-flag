//! Integration tests verifying ClickHouse materialized views populate correctly.
//!
//! Requires a running ClickHouse instance at `http://localhost:8123` (credentials
//! `stitchd:stitchd`, database `stitchd`). Run with:
//!
//! ```sh
//! cargo test --test clickhouse_views -- --test-threads=1
//! ```

use chrono::Utc;
use clickhouse::Client;
use serde::Deserialize;
use stitchd_core::event::{EventContext, EventPayload, EventValue};
use stitchd_events::{migrations, writer::EventWriter};
use uuid::Uuid;

fn make_client() -> Client {
    Client::default()
        .with_url("http://localhost:8123")
        .with_user("stitchd")
        .with_password("stitchd")
        .with_database("stitchd")
}

async fn setup_migrations(client: &Client) {
    migrations::run(client).await.expect("migrations should apply");
}

fn make_writer() -> EventWriter {
    EventWriter::new(make_client())
}

// ── Query result types ────────────────────────────────────────────────────────

#[derive(Debug, clickhouse::Row, Deserialize)]
struct CountRow {
    metric_key: String,
    event_count: u64,
}

#[derive(Debug, clickhouse::Row, Deserialize)]
struct NumericRow {
    metric_key: String,
    value_sum: f64,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn payload(_env_id: Uuid, metric_key: &str, value: EventValue) -> EventPayload {
    EventPayload {
        contexts: vec![EventContext {
            context_type: "user".into(),
            key: "u1".into(),
        }],
        metric_key: metric_key.to_string(),
        value,
        timestamp: Utc::now(),
    }
}

async fn wait_for_merge(client: &Client, table: &str) {
    // OPTIMIZE forces a merge so SummingMergeTree/AggregatingMergeTree consolidates parts.
    let sql = format!("OPTIMIZE TABLE {table} FINAL");
    client.query(&sql).execute().await.ok();
    // Brief sleep in case parts haven't landed yet
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    // Re-run just in case
    client.query(&sql).execute().await.ok();
}

async fn query_count_rows(client: &Client, env_id: Uuid) -> Vec<CountRow> {
    let sql = format!(
        "SELECT CAST(metric_key AS String) AS metric_key, sum(event_count) AS event_count \
         FROM events_count WHERE env_id = '{env_id}' GROUP BY metric_key"
    );
    client.query(&sql).fetch_all().await.unwrap()
}

async fn query_numeric_rows(client: &Client, env_id: Uuid) -> Vec<NumericRow> {
    let sql = format!(
        "SELECT CAST(metric_key AS String) AS metric_key, sumMerge(value_sum) AS value_sum \
         FROM events_numeric WHERE env_id = '{env_id}' GROUP BY metric_key"
    );
    client.query(&sql).fetch_all().await.unwrap()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn events_count_mv_populates_from_bool_events() {
    let client = make_client();
    setup_migrations(&client).await;
    let writer = make_writer();
    let env_id = Uuid::new_v4();

    writer
        .write(env_id, &payload(env_id, "conversion", EventValue::Bool(true)))
        .await
        .unwrap();
    writer
        .write(env_id, &payload(env_id, "conversion", EventValue::Bool(false)))
        .await
        .unwrap();

    wait_for_merge(&client, "events_count").await;

    let rows = query_count_rows(&client, env_id).await;
    let row = rows.iter().find(|r| r.metric_key == "conversion");
    assert!(row.is_some(), "expected 'conversion' row in events_count");
    assert!(
        row.unwrap().event_count >= 2,
        "expected at least 2 events, got {}",
        row.unwrap().event_count
    );
}

#[tokio::test]
async fn events_count_mv_populates_from_int_events() {
    let client = make_client();
    setup_migrations(&client).await;
    let writer = make_writer();
    let env_id = Uuid::new_v4();

    for i in 1..=5i64 {
        writer
            .write(env_id, &payload(env_id, "click_count", EventValue::Int(i)))
            .await
            .unwrap();
    }

    wait_for_merge(&client, "events_count").await;

    let rows = query_count_rows(&client, env_id).await;
    let row = rows.iter().find(|r| r.metric_key == "click_count");
    assert!(row.is_some(), "expected 'click_count' row in events_count");
    assert!(
        row.unwrap().event_count >= 5,
        "expected at least 5 events, got {}",
        row.unwrap().event_count
    );
}

#[tokio::test]
async fn events_numeric_mv_accumulates_sum_for_int_events() {
    let client = make_client();
    setup_migrations(&client).await;
    let writer = make_writer();
    let env_id = Uuid::new_v4();

    // Insert 3 int events with values 10, 20, 30 → sum should be 60
    for v in [10i64, 20, 30] {
        writer
            .write(env_id, &payload(env_id, "revenue_int", EventValue::Int(v)))
            .await
            .unwrap();
    }

    wait_for_merge(&client, "events_numeric").await;

    let rows = query_numeric_rows(&client, env_id).await;
    let row = rows.iter().find(|r| r.metric_key == "revenue_int");
    assert!(row.is_some(), "expected 'revenue_int' row in events_numeric");
    assert!(
        (row.unwrap().value_sum - 60.0).abs() < 0.01,
        "expected sum ~60.0, got {}",
        row.unwrap().value_sum
    );
}

#[tokio::test]
async fn events_numeric_mv_accumulates_sum_for_double_events() {
    let client = make_client();
    setup_migrations(&client).await;
    let writer = make_writer();
    let env_id = Uuid::new_v4();

    // Insert 3 double events → sum should be ~9.99
    for v in [3.33f64, 3.33, 3.33] {
        writer
            .write(env_id, &payload(env_id, "revenue_double", EventValue::Double(v)))
            .await
            .unwrap();
    }

    wait_for_merge(&client, "events_numeric").await;

    let rows = query_numeric_rows(&client, env_id).await;
    let row = rows.iter().find(|r| r.metric_key == "revenue_double");
    assert!(row.is_some(), "expected 'revenue_double' row in events_numeric");
    assert!(
        (row.unwrap().value_sum - 9.99).abs() < 0.01,
        "expected sum ~9.99, got {}",
        row.unwrap().value_sum
    );
}

#[tokio::test]
async fn bool_events_not_included_in_numeric_mv() {
    let client = make_client();
    setup_migrations(&client).await;
    let writer = make_writer();
    let env_id = Uuid::new_v4();

    // Bool events must NOT appear in events_numeric
    writer
        .write(env_id, &payload(env_id, "bool_only_metric", EventValue::Bool(true)))
        .await
        .unwrap();

    wait_for_merge(&client, "events_numeric").await;

    let rows = query_numeric_rows(&client, env_id).await;
    let bool_row = rows.iter().find(|r| r.metric_key == "bool_only_metric");
    assert!(
        bool_row.is_none(),
        "bool events should not appear in events_numeric"
    );
}
