//! Integration tests for the ratio query builder against a live
//! ClickHouse instance.
//!
//! These tests exercise `build_ratio_query` end-to-end: write
//! `experiment_assignments` + `events_v2` fixtures, run the generated
//! SQL, and assert per-(context_type, variant) ratios + the
//! `min_denominator` insufficient-data semantics.
//!
//! Tagged `#[ignore]` — opt-in via
//! `cargo test -p stitchd-stats-service --test ratio_query -- --ignored`.

#![allow(clippy::expect_used)]

use chrono::{DateTime, Duration, TimeZone, Utc};
use clickhouse::Client;
use serde::{Deserialize, Serialize};
use stitchd_core::id::MetricId;
use stitchd_core::metric::{AggregationConfig, AggregationOperator, RatioConfig};
use stitchd_stats_service::{
    dispatch::rewrite_placeholders_to_clickhouse,
    queries::{QueryBind, ratio::build_ratio_query},
};
use uuid::Uuid;

// ── Fixture row shapes (mirror the CH tables) ────────────────────────────

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
#[allow(dead_code)] // num_value/den_value/den_count are deserialised for the wire shape
struct RatioRow {
    context_type: String,
    variant_key: String,
    num_value: f64,
    den_value: f64,
    den_count: f64,
    metric_value: Option<f64>,
}

// ── Helpers ──────────────────────────────────────────────────────────────

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

async fn execute(ch: &Client, sql: String, binds: Vec<QueryBind>) -> Vec<RatioRow> {
    let sql = rewrite_placeholders_to_clickhouse(sql);
    let mut q = ch.query(&sql);
    for b in binds {
        q = match b {
            QueryBind::Str(s) => q.bind(s),
            QueryBind::I64(n) => q.bind(n),
            QueryBind::F64(f) => q.bind(f),
        };
    }
    q.fetch_all::<RatioRow>()
        .await
        .expect("execute ratio query against CH")
}

fn count_cfg(event_key: &str) -> AggregationConfig {
    AggregationConfig {
        event_key: event_key.into(),
        aggregator: AggregationOperator::Count,
        on_field: None,
        where_clause: None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

/// Numerator and denominator computed under the same assignment scope,
/// per (context_type, variant_key) row. Ratio = num / den.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn ratio_per_context_type_and_variant() {
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

    // Two assigned users — alice → treatment, bob → control.
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

    let t = assigned_at + Duration::hours(1);
    // alice: 1 start + 1 complete (ratio 1.0); bob: 2 starts + 1 complete (0.5).
    insert_events(
        &ch,
        &[
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "checkout_started".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t,
                ingested_at: t,
                properties: vec![],
                occurred_at: t,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "checkout_completed".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t,
                ingested_at: t,
                properties: vec![],
                occurred_at: t,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_bob".into())],
                metric_key: "checkout_started".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t,
                ingested_at: t,
                properties: vec![],
                occurred_at: t,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_bob".into())],
                metric_key: "checkout_started".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t + Duration::minutes(10),
                ingested_at: t + Duration::minutes(10),
                properties: vec![],
                occurred_at: t + Duration::minutes(10),
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_bob".into())],
                metric_key: "checkout_completed".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t,
                ingested_at: t,
                properties: vec![],
                occurred_at: t,
            },
        ],
    )
    .await;

    let ratio_cfg = RatioConfig {
        numerator_metric_id: MetricId::new(),
        denominator_metric_id: MetricId::new(),
        min_denominator: 0,
    };
    let built = build_ratio_query(
        &ratio_cfg,
        &count_cfg("checkout_completed"),
        &count_cfg("checkout_started"),
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        &["control", "treatment"],
        iter_end,
    )
    .expect("build ratio query");
    let rows = execute(&ch, built.sql, built.binds).await;

    let treatment = rows
        .iter()
        .find(|r| r.context_type == "user" && r.variant_key == "treatment")
        .expect("user/treatment row");
    let control = rows
        .iter()
        .find(|r| r.context_type == "user" && r.variant_key == "control")
        .expect("user/control row");
    assert!(
        (treatment.metric_value.expect("treatment ratio") - 1.0).abs() < f64::EPSILON,
        "treatment ratio should be 1.0; got {:?}",
        treatment
    );
    assert!(
        (control.metric_value.expect("control ratio") - 0.5).abs() < f64::EPSILON,
        "control ratio should be 0.5; got {:?}",
        control
    );
}

/// `min_denominator` semantics: rows below threshold are filtered out
/// (the WHERE clause uses `den_count >= ?`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn ratio_filters_by_min_denominator() {
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

    // Only 1 denominator event — below `min_denominator=100`.
    let t = assigned_at + Duration::hours(1);
    insert_events(
        &ch,
        &[
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "checkout_started".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t,
                ingested_at: t,
                properties: vec![],
                occurred_at: t,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "checkout_completed".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t,
                ingested_at: t,
                properties: vec![],
                occurred_at: t,
            },
        ],
    )
    .await;

    let ratio_cfg = RatioConfig {
        numerator_metric_id: MetricId::new(),
        denominator_metric_id: MetricId::new(),
        min_denominator: 100,
    };
    let built = build_ratio_query(
        &ratio_cfg,
        &count_cfg("checkout_completed"),
        &count_cfg("checkout_started"),
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        &["treatment"],
        iter_end,
    )
    .expect("build ratio query");
    let rows = execute(&ch, built.sql, built.binds).await;
    assert!(
        rows.is_empty(),
        "min_denominator must filter out under-threshold rows, got {rows:?}"
    );
}
