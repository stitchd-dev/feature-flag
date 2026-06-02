//! Integration tests for the interaction-detection query builders against a
//! live ClickHouse instance.
//!
//! These exercise the FULL contract of [`build_interaction_cells_query`] and
//! [`build_shared_count_query`] — the `experiment_assignments FINAL` self-join
//! on shared `(env_id, context_type, context_key)`, the per-variant-combination
//! contingency table, and the scalar shared-context count. They require:
//!
//!   * A running ClickHouse reachable at `STITCHD_CLICKHOUSE_URL`
//!     (default: `http://localhost:8123`).
//!   * The migrations applied (so `experiment_assignments` exists).
//!
//! Tagged `#[ignore]` so the default `cargo test` run needs no infrastructure;
//! invoke with `cargo test -p stitchd-stats-service --test interaction_query --
//! --ignored`.

#![allow(clippy::expect_used)]

use chrono::{Duration, TimeZone, Utc};
use clickhouse::Client;
use serde::Deserialize;
use stitchd_db::clickhouse::SeedAssignmentRow;
use stitchd_stats_service::{
    dispatch::rewrite_placeholders_to_clickhouse,
    queries::{
        QueryBind,
        interaction::{build_interaction_cells_query, build_shared_count_query},
    },
};
use uuid::Uuid;

type AssignmentRow = SeedAssignmentRow;

#[derive(Debug, Clone, Deserialize, clickhouse::Row)]
struct CellRow {
    a_variant_key: String,
    b_variant_key: String,
    cell_count: u64,
}

#[derive(Debug, Clone, Deserialize, clickhouse::Row)]
struct SharedCountRow {
    shared_count: u64,
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

fn bind_all(mut q: clickhouse::query::Query, binds: Vec<QueryBind>) -> clickhouse::query::Query {
    for b in binds {
        q = match b {
            QueryBind::Str(s) => q.bind(s),
            QueryBind::I64(n) => q.bind(n),
            QueryBind::F64(f) => q.bind(f),
        };
    }
    q
}

async fn fetch_cells(ch: &Client, sql: String, binds: Vec<QueryBind>) -> Vec<CellRow> {
    let sql = rewrite_placeholders_to_clickhouse(sql);
    bind_all(ch.query(&sql), binds)
        .fetch_all::<CellRow>()
        .await
        .expect("execute interaction cells query against CH")
}

async fn fetch_shared_count(ch: &Client, sql: String, binds: Vec<QueryBind>) -> u64 {
    let sql = rewrite_placeholders_to_clickhouse(sql);
    bind_all(ch.query(&sql), binds)
        .fetch_one::<SharedCountRow>()
        .await
        .expect("execute shared-count query against CH")
        .shared_count
}

fn assignment(
    exp_id: Uuid,
    env_id: Uuid,
    context_key: &str,
    variant_key: &str,
    assigned_at: chrono::DateTime<Utc>,
) -> AssignmentRow {
    AssignmentRow {
        experiment_id: exp_id,
        iteration_id: Uuid::new_v4(),
        env_id,
        flag_id: Uuid::new_v4(),
        matched_rule_id: None,
        context_type: "user".into(),
        context_key: context_key.into(),
        variant_key: variant_key.into(),
        assigned_at,
        version: -assigned_at.timestamp_millis(),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// Cross-tabulation of shared contexts matches the seeded fixture, and the
/// shared-count equals the grand total of the cell grid.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn interaction_cells_and_shared_count_match_fixture() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let env_id = Uuid::new_v4();
    let exp_a = Uuid::new_v4();
    let exp_b = Uuid::new_v4();
    let at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();

    // Shared contexts: u1, u2, u3.
    //   u1: A=treatment, B=treatment
    //   u2: A=treatment, B=control
    //   u3: A=control,   B=treatment
    // Not shared: u4 (only A), u5 (only B). These must NOT appear.
    let mut rows = vec![
        assignment(exp_a, env_id, "u1", "treatment", at),
        assignment(exp_a, env_id, "u2", "treatment", at),
        assignment(exp_a, env_id, "u3", "control", at),
        assignment(exp_a, env_id, "u4", "treatment", at),
        assignment(exp_b, env_id, "u1", "treatment", at),
        assignment(exp_b, env_id, "u2", "control", at),
        assignment(exp_b, env_id, "u3", "treatment", at),
        assignment(exp_b, env_id, "u5", "control", at),
    ];
    insert_assignments(&ch, &rows).await;
    rows.clear();

    let cells = build_interaction_cells_query(
        &env_id.to_string(),
        &exp_a.to_string(),
        &exp_b.to_string(),
        "user",
        end,
    )
    .expect("build cells query");
    let cell_rows = fetch_cells(&ch, cells.sql, cells.binds).await;

    let find = |a: &str, b: &str| -> u64 {
        cell_rows
            .iter()
            .find(|r| r.a_variant_key == a && r.b_variant_key == b)
            .map_or(0, |r| r.cell_count)
    };
    assert_eq!(find("treatment", "treatment"), 1, "u1");
    assert_eq!(find("treatment", "control"), 1, "u2");
    assert_eq!(find("control", "treatment"), 1, "u3");
    // No row should exist for the unshared u4 / u5 contexts.
    let total: u64 = cell_rows.iter().map(|r| r.cell_count).sum();
    assert_eq!(total, 3, "only the 3 shared contexts count, got {cell_rows:?}");

    let shared = build_shared_count_query(
        &env_id.to_string(),
        &exp_a.to_string(),
        &exp_b.to_string(),
        "user",
        end,
    )
    .expect("build shared-count query");
    let shared_count = fetch_shared_count(&ch, shared.sql, shared.binds).await;
    assert_eq!(shared_count, 3, "shared_count must equal cell-grid total");
}

/// Contexts assigned AFTER the interaction window upper bound are excluded.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn interaction_excludes_out_of_window_assignments() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let env_id = Uuid::new_v4();
    let exp_a = Uuid::new_v4();
    let exp_b = Uuid::new_v4();
    let in_window = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 5, 10, 0, 0, 0).unwrap();
    let post = end + Duration::days(1);

    insert_assignments(
        &ch,
        &[
            // in-window shared context
            assignment(exp_a, env_id, "u1", "treatment", in_window),
            assignment(exp_b, env_id, "u1", "control", in_window),
            // out-of-window shared context (B side after end) — excluded
            assignment(exp_a, env_id, "u2", "treatment", in_window),
            assignment(exp_b, env_id, "u2", "control", post),
        ],
    )
    .await;

    let shared = build_shared_count_query(
        &env_id.to_string(),
        &exp_a.to_string(),
        &exp_b.to_string(),
        "user",
        end,
    )
    .expect("build shared-count query");
    let shared_count = fetch_shared_count(&ch, shared.sql, shared.binds).await;
    assert_eq!(shared_count, 1, "post-window B assignment must be excluded");
}

/// ReplacingMergeTree: a re-exposed context (two physical rows, same
/// `(experiment, iteration, context_type, context_key)`) collapses to ONE
/// logical row under `FINAL`, so it is counted once — not duplicated.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn interaction_final_dedupes_replacing_merge_tree_rows() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let env_id = Uuid::new_v4();
    let exp_a = Uuid::new_v4();
    let exp_b = Uuid::new_v4();
    let iter_a = Uuid::new_v4();
    let iter_b = Uuid::new_v4();
    let first = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let later = first + Duration::hours(2);
    let end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();

    // Same logical key written twice for A (re-exposure). FINAL keeps the
    // earliest (most-negative _version). ORDER BY includes iteration_id, so
    // both physical rows must share the same iteration_id to actually collapse.
    let mk = |exp: Uuid, iter: Uuid, vk: &str, at: chrono::DateTime<Utc>| AssignmentRow {
        experiment_id: exp,
        iteration_id: iter,
        env_id,
        flag_id: Uuid::new_v4(),
        matched_rule_id: None,
        context_type: "user".into(),
        context_key: "u1".into(),
        variant_key: vk.into(),
        assigned_at: at,
        version: -at.timestamp_millis(),
    };
    insert_assignments(
        &ch,
        &[
            mk(exp_a, iter_a, "treatment", first),
            mk(exp_a, iter_a, "treatment", later),
            mk(exp_b, iter_b, "control", first),
        ],
    )
    .await;

    let shared = build_shared_count_query(
        &env_id.to_string(),
        &exp_a.to_string(),
        &exp_b.to_string(),
        "user",
        end,
    )
    .expect("build shared-count query");
    let shared_count = fetch_shared_count(&ch, shared.sql, shared.binds).await;
    assert_eq!(
        shared_count, 1,
        "FINAL must collapse the re-exposed A row so u1 is counted once"
    );
}
