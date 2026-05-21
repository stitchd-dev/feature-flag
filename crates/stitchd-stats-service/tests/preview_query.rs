//! Integration tests for the experiment-scoped preview query builder.
//!
//! Verifies that `build_experiment_preview_aggregation_query` produces
//! per-day, per-(context_type, variant_key) buckets that respect the
//! same assignment-join + ITT semantics as the headline experiment
//! aggregation query.
//!
//! Tagged `#[ignore]` — opt-in via
//! `cargo test -p stitchd-stats-service --test preview_query -- --ignored`.

#![allow(clippy::expect_used)]

use chrono::{DateTime, Duration, TimeZone, Utc};
use clickhouse::Client;
use serde::{Deserialize, Serialize};
use stitchd_core::metric::{AggregationConfig, AggregationOperator};
use stitchd_stats_service::{
    dispatch::rewrite_placeholders_to_clickhouse,
    queries::{QueryBind, preview::build_experiment_preview_aggregation_query},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct AssignmentRow {
    experiment_id: Uuid,
    iteration_id: Uuid,
    env_id: Uuid,
    flag_id: Uuid,
    matched_rule_id: Option<Uuid>,
    context_type: String,
    context_key: String,
    variant_key: String,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    assigned_at: DateTime<Utc>,
    #[serde(rename = "_version")]
    version: i64,
}

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct EventRow {
    env_id: Uuid,
    contexts: Vec<(String, String)>,
    metric_key: String,
    value_bool: Option<bool>,
    value_int: Option<i64>,
    value_double: Option<f64>,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    timestamp: DateTime<Utc>,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    ingested_at: DateTime<Utc>,
    properties: Vec<(String, String)>,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, clickhouse::Row)]
#[allow(dead_code)]
struct PreviewRow {
    day_ts: u32,
    context_type: String,
    variant_key: String,
    value: Option<f64>,
}

fn make_client() -> Client {
    let url = std::env::var("STITCHD_CLICKHOUSE_URL")
        .unwrap_or_else(|_| "http://localhost:8123".to_string());
    let db = std::env::var("STITCHD_CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());
    let user = std::env::var("STITCHD_CLICKHOUSE_USER").unwrap_or_else(|_| "stitchd".to_string());
    let password =
        std::env::var("STITCHD_CLICKHOUSE_PASSWORD").unwrap_or_else(|_| "stitchd".to_string());
    Client::default()
        .with_url(url)
        .with_database(db)
        .with_user(user)
        .with_password(password)
}

async fn insert_assignments(ch: &Client, rows: &[AssignmentRow]) {
    let mut insert = ch
        .insert::<AssignmentRow>("experiment_assignments")
        .await
        .expect("prepare assignments insert");
    for row in rows {
        insert.write(row).await.expect("write assignment row to CH");
    }
    insert.end().await.expect("finalize assignments insert");
}

async fn insert_events(ch: &Client, rows: &[EventRow]) {
    let mut insert = ch
        .insert::<EventRow>("events_v2")
        .await
        .expect("prepare events_v2 insert");
    for row in rows {
        insert.write(row).await.expect("write event row to CH");
    }
    insert.end().await.expect("finalize events_v2 insert");
}

async fn execute(ch: &Client, sql: String, binds: Vec<QueryBind>) -> Vec<PreviewRow> {
    let sql = rewrite_placeholders_to_clickhouse(sql);
    let mut q = ch.query(&sql);
    for b in binds {
        q = match b {
            QueryBind::Str(s) => q.bind(s),
            QueryBind::I64(n) => q.bind(n),
            QueryBind::F64(f) => q.bind(f),
        };
    }
    q.fetch_all::<PreviewRow>()
        .await
        .expect("execute preview query against CH")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn preview_per_day_per_context_type_per_variant() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let env_id = Uuid::new_v4();
    let exp_id = Uuid::new_v4();
    let iter_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let assigned_at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let iter_end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();

    // alice (treatment), bob (control) — both user-level assigned.
    insert_assignments(
        &ch,
        &[
            AssignmentRow {
                experiment_id: exp_id,
                iteration_id: iter_id,
                env_id,
                flag_id,
                matched_rule_id: None,
                context_type: "user".into(),
                context_key: "u_alice".into(),
                variant_key: "treatment".into(),
                assigned_at,
                version: -assigned_at.timestamp_millis(),
            },
            AssignmentRow {
                experiment_id: exp_id,
                iteration_id: iter_id,
                env_id,
                flag_id,
                matched_rule_id: None,
                context_type: "user".into(),
                context_key: "u_bob".into(),
                variant_key: "control".into(),
                assigned_at,
                version: -assigned_at.timestamp_millis(),
            },
        ],
    )
    .await;

    let day1 = assigned_at + Duration::hours(1);
    let day2 = assigned_at + Duration::days(1) + Duration::hours(1);
    // alice: 2 events on day1, 1 on day2.
    // bob:   1 event on day1.
    insert_events(
        &ch,
        &[
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "click".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: day1,
                ingested_at: day1,
                properties: vec![],
                occurred_at: day1,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "click".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: day1 + Duration::minutes(5),
                ingested_at: day1 + Duration::minutes(5),
                properties: vec![],
                occurred_at: day1 + Duration::minutes(5),
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "click".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: day2,
                ingested_at: day2,
                properties: vec![],
                occurred_at: day2,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_bob".into())],
                metric_key: "click".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: day1,
                ingested_at: day1,
                properties: vec![],
                occurred_at: day1,
            },
        ],
    )
    .await;

    let cfg = AggregationConfig {
        event_key: "click".into(),
        aggregator: AggregationOperator::Count,
        on_field: None,
        where_clause: None,
    };
    let built = build_experiment_preview_aggregation_query(
        &cfg,
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        iter_end,
    )
    .expect("build experiment preview aggregation");
    let rows = execute(&ch, built.sql, built.binds).await;

    // Expect 3 rows: (day1, user, treatment) = 2, (day1, user, control) = 1,
    // (day2, user, treatment) = 1.
    assert_eq!(
        rows.len(),
        3,
        "expected 3 (day, ctx, variant) rows: {rows:?}"
    );

    let day1_ts = day1
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc()
        .timestamp() as u32;
    let day2_ts = day2
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight")
        .and_utc()
        .timestamp() as u32;

    let alice_day1 = rows
        .iter()
        .find(|r| r.day_ts == day1_ts && r.variant_key == "treatment")
        .expect("alice day1 row");
    let alice_day2 = rows
        .iter()
        .find(|r| r.day_ts == day2_ts && r.variant_key == "treatment")
        .expect("alice day2 row");
    let bob_day1 = rows
        .iter()
        .find(|r| r.day_ts == day1_ts && r.variant_key == "control")
        .expect("bob day1 row");

    assert!((alice_day1.value.expect("alice d1") - 2.0).abs() < f64::EPSILON);
    assert!((alice_day2.value.expect("alice d2") - 1.0).abs() < f64::EPSILON);
    assert!((bob_day1.value.expect("bob d1") - 1.0).abs() < f64::EPSILON);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn preview_excludes_pre_exposure_per_day() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let env_id = Uuid::new_v4();
    let exp_id = Uuid::new_v4();
    let iter_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let assigned_at = Utc.with_ymd_and_hms(2026, 5, 10, 12, 0, 0).unwrap();
    let iter_end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();

    insert_assignments(
        &ch,
        &[AssignmentRow {
            experiment_id: exp_id,
            iteration_id: iter_id,
            env_id,
            flag_id,
            matched_rule_id: None,
            context_type: "user".into(),
            context_key: "u_alice".into(),
            variant_key: "treatment".into(),
            assigned_at,
            version: -assigned_at.timestamp_millis(),
        }],
    )
    .await;

    let pre = assigned_at - Duration::hours(2); // same day, but pre-exposure
    let post = assigned_at + Duration::hours(2);
    insert_events(
        &ch,
        &[
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "view".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: pre,
                ingested_at: pre,
                properties: vec![],
                occurred_at: pre,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "view".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: post,
                ingested_at: post,
                properties: vec![],
                occurred_at: post,
            },
        ],
    )
    .await;

    let cfg = AggregationConfig {
        event_key: "view".into(),
        aggregator: AggregationOperator::Count,
        on_field: None,
        where_clause: None,
    };
    let built = build_experiment_preview_aggregation_query(
        &cfg,
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        iter_end,
    )
    .expect("build experiment preview aggregation");
    let rows = execute(&ch, built.sql, built.binds).await;
    assert_eq!(rows.len(), 1, "exactly one post-exposure row");
    assert!(
        (rows[0].value.expect("value") - 1.0).abs() < f64::EPSILON,
        "pre-exposure event must be excluded even when same day; got {:?}",
        rows[0]
    );
}
