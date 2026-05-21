//! Integration tests for the funnel query builder against a live
//! ClickHouse instance.
//!
//! Tagged `#[ignore]` — opt-in via
//! `cargo test -p stitchd-stats-service --test funnel_query -- --ignored`.

#![allow(clippy::expect_used)]

use chrono::{DateTime, Duration, TimeZone, Utc};
use clickhouse::Client;
use serde::{Deserialize, Serialize};
use stitchd_core::metric::{FunnelConfig, FunnelStep};
use stitchd_stats_service::{
    dispatch::rewrite_placeholders_to_clickhouse,
    queries::{QueryBind, funnel::build_funnel_query},
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
struct FunnelRow {
    context_type: String,
    variant_key: String,
    step_index: u32,
    event_key: String,
    step_count: u64,
    step_total: u64,
    conversion_rate: Option<f64>,
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

async fn execute(ch: &Client, sql: String, binds: Vec<QueryBind>) -> Vec<FunnelRow> {
    let sql = rewrite_placeholders_to_clickhouse(sql);
    let mut q = ch.query(&sql);
    for b in binds {
        q = match b {
            QueryBind::Str(s) => q.bind(s),
            QueryBind::I64(n) => q.bind(n),
            QueryBind::F64(f) => q.bind(f),
        };
    }
    q.fetch_all::<FunnelRow>()
        .await
        .expect("execute funnel query against CH")
}

fn step(key: &str) -> FunnelStep {
    FunnelStep {
        event_key: key.into(),
        where_clause: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn funnel_three_step_per_assigned_context() {
    // 3-step funnel: view → add_to_cart → checkout_completed
    // alice (treatment) completes all 3 steps within the window.
    // bob (treatment) gets to step 2 only.
    // expected: step_total = 2, step1 count = 2, step2 count = 1.
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
                variant_key: "treatment".into(),
                assigned_at,
                version: -assigned_at.timestamp_millis(),
            },
        ],
    )
    .await;

    let t0 = assigned_at + Duration::minutes(1);
    let t1 = t0 + Duration::minutes(5);
    let t2 = t0 + Duration::minutes(10);
    // alice: view → add_to_cart → checkout_completed (all 3).
    // bob:   view → add_to_cart (just 2).
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
                timestamp: t0,
                ingested_at: t0,
                properties: vec![],
                occurred_at: t0,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "add_to_cart".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t1,
                ingested_at: t1,
                properties: vec![],
                occurred_at: t1,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "checkout_completed".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t2,
                ingested_at: t2,
                properties: vec![],
                occurred_at: t2,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_bob".into())],
                metric_key: "view".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t0,
                ingested_at: t0,
                properties: vec![],
                occurred_at: t0,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_bob".into())],
                metric_key: "add_to_cart".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t1,
                ingested_at: t1,
                properties: vec![],
                occurred_at: t1,
            },
        ],
    )
    .await;

    let cfg = FunnelConfig {
        steps: vec![
            step("view"),
            step("add_to_cart"),
            step("checkout_completed"),
        ],
        window_seconds: 3600,
        count_repeats: false,
    };
    let built = build_funnel_query(
        &cfg,
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        &["treatment"],
        iter_end,
    )
    .expect("build funnel query");

    let rows = execute(&ch, built.sql, built.binds).await;
    // 3 step rows for (user, treatment).
    let by_step: std::collections::HashMap<u32, &FunnelRow> =
        rows.iter().map(|r| (r.step_index, r)).collect();
    assert_eq!(by_step.len(), 3, "expected 3 step rows, got {rows:?}");

    let s0 = by_step.get(&0).expect("step 0 row");
    let s1 = by_step.get(&1).expect("step 1 row");
    let s2 = by_step.get(&2).expect("step 2 row");

    assert_eq!(s0.context_type, "user");
    assert_eq!(s0.variant_key, "treatment");
    assert_eq!(
        s0.step_total, 2,
        "step_total = number of contexts that started the funnel"
    );
    assert_eq!(s0.step_count, 2, "step 0 reaches everyone");
    assert_eq!(
        s1.step_count, 2,
        "alice + bob both reached step 1 (add_to_cart)"
    );
    assert_eq!(
        s2.step_count, 1,
        "only alice reached step 2 (checkout_completed)"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn funnel_excludes_unassigned_contexts() {
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

    let t0 = assigned_at + Duration::minutes(1);
    let t1 = t0 + Duration::minutes(5);
    insert_events(
        &ch,
        &[
            // alice — full funnel.
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "step_a".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t0,
                ingested_at: t0,
                properties: vec![],
                occurred_at: t0,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "step_b".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t1,
                ingested_at: t1,
                properties: vec![],
                occurred_at: t1,
            },
            // eve — NOT assigned. Her events should not contribute.
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_eve".into())],
                metric_key: "step_a".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t0,
                ingested_at: t0,
                properties: vec![],
                occurred_at: t0,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_eve".into())],
                metric_key: "step_b".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: t1,
                ingested_at: t1,
                properties: vec![],
                occurred_at: t1,
            },
        ],
    )
    .await;

    let cfg = FunnelConfig {
        steps: vec![step("step_a"), step("step_b")],
        window_seconds: 3600,
        count_repeats: false,
    };
    let built = build_funnel_query(
        &cfg,
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        &["treatment"],
        iter_end,
    )
    .expect("build funnel query");
    let rows = execute(&ch, built.sql, built.binds).await;

    let by_step: std::collections::HashMap<u32, &FunnelRow> =
        rows.iter().map(|r| (r.step_index, r)).collect();
    let s1 = by_step.get(&1).expect("step 1 row");
    assert_eq!(s1.step_count, 1, "only alice should count");
    assert_eq!(s1.step_total, 1, "only alice entered the funnel");
}
