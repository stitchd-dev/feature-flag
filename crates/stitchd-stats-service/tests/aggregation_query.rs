//! Integration tests for the aggregation query builder against a live
//! ClickHouse instance.
//!
//! These tests exercise the FULL contract of `build_aggregation_query` —
//! including the JOIN against `experiment_assignments` and the ITT bound
//! `e.occurred_at >= a.assigned_at`. They require:
//!
//!   * A running ClickHouse reachable at `STITCHD_CLICKHOUSE_URL`
//!     (default: `http://localhost:8123`).
//!   * The Phase 4 migrations applied (so `experiment_assignments` and
//!     `events` exist with the right schema).
//!
//! The tests are tagged `#[ignore]` so the default `cargo test` run does
//! not require infrastructure. CI / local runs invoke them with
//! `cargo test -p stitchd-stats-service --test aggregation_query -- \
//!  --ignored`.

#![allow(clippy::expect_used)]

use chrono::{Duration, TimeZone, Utc};
use clickhouse::Client;
use serde::Deserialize;
use stitchd_core::metric::{AggregationConfig, AggregationOperator};
use stitchd_db::clickhouse::SeedAssignmentRow;
use stitchd_event_writer::SeedEventRow;
use stitchd_stats_service::{
    dispatch::rewrite_placeholders_to_clickhouse,
    queries::{QueryBind, aggregation::build_aggregation_query},
};
use uuid::Uuid;

// ── Fixture row shapes ──────────────────────────────────────────────────────
// `SeedAssignmentRow` is shared from `stitchd-db` (DUP-004).
// `SeedEventRow` is shared from `stitchd-event-writer` (DUP-005).

type AssignmentRow = SeedAssignmentRow;
type EventRow = SeedEventRow;

#[derive(Debug, Clone, Deserialize, clickhouse::Row)]
struct AggregationResultRow {
    context_type: String,
    variant_key: String,
    metric_value: f64,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

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
        .insert::<EventRow>("events")
        .await
        .expect("prepare events insert");
    for row in rows {
        insert.write(row).await.expect("write event row to CH");
    }
    insert.end().await.expect("finalize events insert");
}

async fn execute(ch: &Client, sql: String, binds: Vec<QueryBind>) -> Vec<AggregationResultRow> {
    let sql = rewrite_placeholders_to_clickhouse(sql);
    let mut q = ch.query(&sql);
    for b in binds {
        q = match b {
            QueryBind::Str(s) => q.bind(s),
            QueryBind::I64(n) => q.bind(n),
            QueryBind::F64(f) => q.bind(f),
        };
    }
    q.fetch_all::<AggregationResultRow>()
        .await
        .expect("execute aggregation query against CH")
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// Per-(context_type, variant_key) sums match the seeded fixture.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn aggregation_groups_per_context_type_and_variant() {
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

    let assignments = vec![
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
    ];
    insert_assignments(&ch, &assignments).await;

    let after_assign = assigned_at + Duration::hours(1);
    let events = vec![
        EventRow {
            env_id,
            contexts: vec![("user".into(), "u_alice".into())],
            metric_key: "checkout_completed".into(),
            value_bool: None,
            value_int: None,
            value_double: None,
            timestamp: after_assign,
            ingested_at: after_assign,
            properties: vec![],
            occurred_at: after_assign,
        },
        EventRow {
            env_id,
            contexts: vec![("user".into(), "u_alice".into())],
            metric_key: "checkout_completed".into(),
            value_bool: None,
            value_int: None,
            value_double: None,
            timestamp: after_assign + Duration::minutes(5),
            ingested_at: after_assign + Duration::minutes(5),
            properties: vec![],
            occurred_at: after_assign + Duration::minutes(5),
        },
        EventRow {
            env_id,
            contexts: vec![("user".into(), "u_bob".into())],
            metric_key: "checkout_completed".into(),
            value_bool: None,
            value_int: None,
            value_double: None,
            timestamp: after_assign,
            ingested_at: after_assign,
            properties: vec![],
            occurred_at: after_assign,
        },
    ];
    insert_events(&ch, &events).await;

    let cfg = AggregationConfig {
        event_key: "checkout_completed".into(),
        aggregator: AggregationOperator::Count,
        on_field: None,
        where_clause: None,
    };
    let built = build_aggregation_query(
        &cfg,
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        &["control", "treatment"],
        iter_end,
    )
    .expect("build aggregation query");

    let rows = execute(&ch, built.sql, built.binds).await;

    // 2 events for alice (treatment), 1 event for bob (control).
    let treatment = rows
        .iter()
        .find(|r| r.context_type == "user" && r.variant_key == "treatment")
        .expect("must have a user/treatment row");
    let control = rows
        .iter()
        .find(|r| r.context_type == "user" && r.variant_key == "control")
        .expect("must have a user/control row");
    assert!(
        (treatment.metric_value - 2.0).abs() < f64::EPSILON,
        "treatment count={}",
        treatment.metric_value
    );
    assert!(
        (control.metric_value - 1.0).abs() < f64::EPSILON,
        "control count={}",
        control.metric_value
    );
}

/// Pre-exposure events (`occurred_at < assigned_at`) MUST be excluded.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn aggregation_excludes_pre_exposure_events() {
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

    let pre = assigned_at - Duration::hours(1);
    let post = assigned_at + Duration::hours(1);
    insert_events(
        &ch,
        &[
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "checkout".into(),
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
                metric_key: "checkout".into(),
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
        event_key: "checkout".into(),
        aggregator: AggregationOperator::Count,
        on_field: None,
        where_clause: None,
    };
    let built = build_aggregation_query(
        &cfg,
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        &["treatment"],
        iter_end,
    )
    .expect("build aggregation query");
    let rows = execute(&ch, built.sql, built.binds).await;

    assert_eq!(rows.len(), 1, "exactly one (context_type, variant_key) row");
    assert!(
        (rows[0].metric_value - 1.0).abs() < f64::EPSILON,
        "pre-exposure event must be excluded; got metric_value={}",
        rows[0].metric_value
    );
}

/// Events fired AFTER `iteration_end` MUST be excluded.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn aggregation_excludes_out_of_iteration_events() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let env_id = Uuid::new_v4();
    let exp_id = Uuid::new_v4();
    let iter_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let assigned_at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let iter_end = Utc.with_ymd_and_hms(2026, 5, 10, 0, 0, 0).unwrap();

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

    let in_window = assigned_at + Duration::days(1);
    let post_window = iter_end + Duration::hours(1);
    insert_events(
        &ch,
        &[
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "page_view".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: in_window,
                ingested_at: in_window,
                properties: vec![],
                occurred_at: in_window,
            },
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_alice".into())],
                metric_key: "page_view".into(),
                value_bool: None,
                value_int: None,
                value_double: None,
                timestamp: post_window,
                ingested_at: post_window,
                properties: vec![],
                occurred_at: post_window,
            },
        ],
    )
    .await;

    let cfg = AggregationConfig {
        event_key: "page_view".into(),
        aggregator: AggregationOperator::Count,
        on_field: None,
        where_clause: None,
    };
    let built = build_aggregation_query(
        &cfg,
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        &["treatment"],
        iter_end,
    )
    .expect("build aggregation query");
    let rows = execute(&ch, built.sql, built.binds).await;

    assert_eq!(rows.len(), 1);
    assert!(
        (rows[0].metric_value - 1.0).abs() < f64::EPSILON,
        "out-of-iteration event must be excluded; got metric_value={}",
        rows[0].metric_value
    );
}

/// An event with NO matching assignment row is excluded — no INNER JOIN
/// match means no contribution to any (context_type, variant) bucket.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn aggregation_excludes_unassigned_events() {
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

    let t = assigned_at + Duration::hours(1);
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
                timestamp: t,
                ingested_at: t,
                properties: vec![],
                occurred_at: t,
            },
            // unassigned context — no matching row in experiment_assignments.
            EventRow {
                env_id,
                contexts: vec![("user".into(), "u_eve".into())],
                metric_key: "click".into(),
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

    let cfg = AggregationConfig {
        event_key: "click".into(),
        aggregator: AggregationOperator::Count,
        on_field: None,
        where_clause: None,
    };
    let built = build_aggregation_query(
        &cfg,
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        &["treatment"],
        iter_end,
    )
    .expect("build aggregation query");
    let rows = execute(&ch, built.sql, built.binds).await;

    assert_eq!(rows.len(), 1, "single (user, treatment) row expected");
    assert!(
        (rows[0].metric_value - 1.0).abs() < f64::EPSILON,
        "only the assigned alice event must count; got metric_value={}",
        rows[0].metric_value
    );
}

/// Multi-context-type experiment: events tagged with `(user, ...)` count
/// against the user assignment; events tagged with `(account, ...)` count
/// against the account assignment. Both context types appear in the result.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn aggregation_multi_context_type_groups_independently() {
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
                context_type: "account".into(),
                context_key: "acct_42".into(),
                variant_key: "control".into(),
                assigned_at,
                version: -assigned_at.timestamp_millis(),
            },
        ],
    )
    .await;

    let t = assigned_at + Duration::hours(1);
    insert_events(
        &ch,
        &[EventRow {
            env_id,
            contexts: vec![
                ("user".into(), "u_alice".into()),
                ("account".into(), "acct_42".into()),
            ],
            metric_key: "view".into(),
            value_bool: None,
            value_int: None,
            value_double: None,
            timestamp: t,
            ingested_at: t,
            properties: vec![],
            occurred_at: t,
        }],
    )
    .await;

    let cfg = AggregationConfig {
        event_key: "view".into(),
        aggregator: AggregationOperator::Count,
        on_field: None,
        where_clause: None,
    };
    let built = build_aggregation_query(
        &cfg,
        &exp_id.to_string(),
        &iter_id.to_string(),
        &env_id.to_string(),
        &["control", "treatment"],
        iter_end,
    )
    .expect("build aggregation query");
    let rows = execute(&ch, built.sql, built.binds).await;

    let user_row = rows
        .iter()
        .find(|r| r.context_type == "user")
        .expect("must include a user/treatment row");
    let acct_row = rows
        .iter()
        .find(|r| r.context_type == "account")
        .expect("must include an account/control row");
    assert!((user_row.metric_value - 1.0).abs() < f64::EPSILON);
    assert!((acct_row.metric_value - 1.0).abs() < f64::EPSILON);
    assert_eq!(user_row.variant_key, "treatment");
    assert_eq!(acct_row.variant_key, "control");
}
