//! Live-ClickHouse integration tests for the cross-experiment interaction
//! compute-and-persist pipeline ([`compute_and_persist_interactions`]).
//!
//! These seed `experiment_assignments` (two experiments on distinct flags
//! sharing a conversion metric) and `events`, then run the full orchestration
//! through the production ClickHouse reader/writer and assert on the rows
//! written to `experiment_interactions`. They require:
//!
//!   * A running ClickHouse reachable at `STITCHD_CLICKHOUSE_URL`
//!     (default: `http://localhost:8123`).
//!   * The migrations applied (so `experiment_assignments`, `events`, and
//!     `experiment_interactions` exist).
//!
//! Tagged `#[ignore]`; run with
//! `cargo test -p stitchd-stats-service --test interaction_compute -- --ignored`.

#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use clickhouse::Client;
use serde::Deserialize;
use stitchd_core::id::{EnvironmentId, MetricId};
use stitchd_core::metric::{
    AggregationConfig, AggregationOperator, GoalDirection, MetricDefinition, MetricKind,
};
use stitchd_db::clickhouse::SeedAssignmentRow;
use stitchd_event_writer::SeedEventRow;
use stitchd_stats_service::interaction_compute::{
    ClickHouseInteractionCells, ClickHouseInteractionWriter, compute_and_persist_interactions,
};
use stitchd_stats_service::interaction_pairs::ExperimentMeta;
use uuid::Uuid;

type AssignmentRow = SeedAssignmentRow;
type EventRow = SeedEventRow;

#[derive(Debug, Clone, Deserialize, clickhouse::Row)]
struct InteractionResultRow {
    #[serde(with = "clickhouse::serde::uuid")]
    experiment_id_a: Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    experiment_id_b: Uuid,
    context_type: String,
    metric_key: String,
    shared_count: u64,
    cell_stats: String,
    significant: bool,
    insufficient_data: bool,
}

/// Decoded `cell_stats` JSON row (mirrors `CellAggregate`).
#[derive(Debug, Clone, Deserialize)]
struct CellStat {
    a_variant_key: String,
    b_variant_key: String,
    n: u64,
    successes: u64,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn make_client() -> Arc<Client> {
    let url = std::env::var("STITCHD_CLICKHOUSE_URL")
        .unwrap_or_else(|_| "http://localhost:8123".to_string());
    let db = std::env::var("STITCHD_CLICKHOUSE_DB").unwrap_or_else(|_| "stitchd".to_string());
    let user = std::env::var("STITCHD_CLICKHOUSE_USER").unwrap_or_else(|_| "stitchd".to_string());
    let password =
        std::env::var("STITCHD_CLICKHOUSE_PASSWORD").unwrap_or_else(|_| "stitchd".to_string());
    Arc::new(
        Client::default()
            .with_url(url)
            .with_database(db)
            .with_user(user)
            .with_password(password),
    )
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

#[allow(clippy::too_many_arguments)]
fn assignment(
    exp_id: Uuid,
    iter_id: Uuid,
    env_id: Uuid,
    flag_id: Uuid,
    context_key: &str,
    variant_key: &str,
    assigned_at: DateTime<Utc>,
) -> AssignmentRow {
    AssignmentRow {
        experiment_id: exp_id,
        iteration_id: iter_id,
        env_id,
        flag_id,
        matched_rule_id: None,
        context_type: "user".into(),
        context_key: context_key.into(),
        variant_key: variant_key.into(),
        assigned_at,
        version: -assigned_at.timestamp_millis(),
    }
}

fn conversion_event(env_id: Uuid, context_key: &str, at: DateTime<Utc>) -> EventRow {
    EventRow {
        env_id,
        contexts: vec![("user".into(), context_key.into())],
        metric_key: "checkout_completed".into(),
        value_bool: None,
        value_int: None,
        value_double: None,
        timestamp: at,
        ingested_at: at,
        properties: vec![],
        occurred_at: at,
    }
}

fn conversion_metric(id: Uuid) -> MetricDefinition {
    let now = Utc::now();
    MetricDefinition {
        id: MetricId::from_uuid(id),
        environment_id: EnvironmentId::new(),
        key: "checkout".into(),
        name: "Checkout conversion".into(),
        description: None,
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    }
}

fn meta(id: Uuid, flag: Uuid, metric: Uuid, started: DateTime<Utc>) -> ExperimentMeta {
    ExperimentMeta {
        id,
        flag_id: flag,
        started_at: started,
        ended_at: None,
        metric_ids: vec![metric],
        exclusion_group_id: None,
    }
}

/// Seed two experiments (distinct flags) over `n_per_cell` shared contexts per
/// `(a_variant, b_variant)` cell, with per-cell conversion rates given by
/// `rates[a][b]`. Returns `(env, exp_a, exp_b, metric_id)`.
async fn seed_pair(
    ch: &Client,
    key_prefix: &str,
    n_per_cell: usize,
    rates: [[f64; 2]; 2],
) -> (Uuid, Uuid, Uuid, Uuid) {
    let env_id = Uuid::new_v4();
    let exp_a = Uuid::new_v4();
    let exp_b = Uuid::new_v4();
    let iter_a = Uuid::new_v4();
    let iter_b = Uuid::new_v4();
    let flag_a = Uuid::new_v4();
    let flag_b = Uuid::new_v4();
    let metric = Uuid::new_v4();
    let at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let evt_at = at + Duration::hours(1);

    let variants = ["control", "treatment"];
    let mut assigns = Vec::new();
    let mut events = Vec::new();

    for (ai, av) in variants.iter().enumerate() {
        for (bi, bv) in variants.iter().enumerate() {
            for k in 0..n_per_cell {
                let ckey = format!("{key_prefix}_{ai}_{bi}_{k}");
                assigns.push(assignment(exp_a, iter_a, env_id, flag_a, &ckey, av, at));
                assigns.push(assignment(exp_b, iter_b, env_id, flag_b, &ckey, bv, at));
                // Convert this context iff k < n_per_cell * rate.
                let converts = (k as f64) < (n_per_cell as f64 * rates[ai][bi]);
                if converts {
                    events.push(conversion_event(env_id, &ckey, evt_at));
                }
            }
        }
    }
    insert_assignments(ch, &assigns).await;
    insert_events(ch, &events).await;
    (env_id, exp_a, exp_b, metric)
}

async fn fetch_rows(ch: &Client, exp: Uuid) -> Vec<InteractionResultRow> {
    let sql = format!(
        "SELECT experiment_id_a, experiment_id_b, context_type, metric_key, shared_count, \
         cell_stats, significant, insufficient_data
         FROM experiment_interactions
         WHERE experiment_id_a = toUUID('{exp}') OR experiment_id_b = toUUID('{exp}')"
    );
    ch.query(&sql)
        .fetch_all::<InteractionResultRow>()
        .await
        .expect("read experiment_interactions")
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// A planted super-additive interaction (treatment×treatment converts far above
/// the additive prediction) writes a `significant = true` row.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn significant_interaction_is_written() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    // control/control 10%, single-treatment ~12-14%, double-treatment 60% —
    // strongly non-additive.
    let rates = [[0.10, 0.12], [0.14, 0.60]];
    let (env, exp_a, exp_b, metric) = seed_pair(&ch, "sig", 100, rates).await;

    let reader = ClickHouseInteractionCells::new(ch.clone());
    let writer = ClickHouseInteractionWriter::new(ch.clone());
    let mut metrics = HashMap::new();
    metrics.insert(metric, conversion_metric(metric));
    let exps = vec![
        meta(
            exp_a,
            Uuid::new_v4(),
            metric,
            Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        ),
        meta(
            exp_b,
            Uuid::new_v4(),
            metric,
            Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        ),
    ];
    // distinct flags
    let mut exps = exps;
    exps[0].flag_id = Uuid::new_v4();
    exps[1].flag_id = Uuid::new_v4();

    let written = compute_and_persist_interactions(
        &reader,
        &writer,
        env,
        &exps,
        &metrics,
        &["user".to_string()],
        Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap(),
    )
    .await
    .expect("compute_and_persist should succeed");
    assert_eq!(written, 1, "one (pair, metric, context_type) row");

    let rows = fetch_rows(&ch, exp_a).await;
    assert_eq!(rows.len(), 1, "exactly one interaction row, got {rows:?}");
    assert_eq!(rows[0].metric_key, "checkout");
    assert_eq!(rows[0].context_type, "user");
    assert_eq!(rows[0].shared_count, 400);
    assert!(
        rows[0].significant,
        "planted super-additive interaction must be significant"
    );
    // MED-2: a well-powered grid is not insufficient.
    assert!(
        !rows[0].insufficient_data,
        "ample-data grid must persist insufficient_data = false"
    );
    // The persisted pair is the sorted (min, max) ordering candidate_pairs emits.
    let (lo, hi) = (exp_a.min(exp_b), exp_a.max(exp_b));
    assert_eq!(rows[0].experiment_id_a, lo);
    assert_eq!(rows[0].experiment_id_b, hi);
}

/// Independent (additive) effects produce a non-significant row.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn independent_effects_are_not_significant() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    // Perfectly additive: base 20%, +10pp for A-treatment, +10pp for
    // B-treatment, double-treatment = 40% (= no interaction).
    let rates = [[0.20, 0.30], [0.30, 0.40]];
    let (env, exp_a, exp_b, metric) = seed_pair(&ch, "ind", 100, rates).await;

    let reader = ClickHouseInteractionCells::new(ch.clone());
    let writer = ClickHouseInteractionWriter::new(ch.clone());
    let mut metrics = HashMap::new();
    metrics.insert(metric, conversion_metric(metric));
    let mut exps = vec![
        meta(
            exp_a,
            Uuid::new_v4(),
            metric,
            Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        ),
        meta(
            exp_b,
            Uuid::new_v4(),
            metric,
            Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        ),
    ];
    exps[0].flag_id = Uuid::new_v4();
    exps[1].flag_id = Uuid::new_v4();

    let written = compute_and_persist_interactions(
        &reader,
        &writer,
        env,
        &exps,
        &metrics,
        &["user".to_string()],
        Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap(),
    )
    .await
    .expect("compute_and_persist should succeed");
    assert_eq!(written, 1);

    let rows = fetch_rows(&ch, exp_a).await;
    assert_eq!(rows.len(), 1);
    assert!(
        !rows[0].significant,
        "additive effects must NOT be flagged as a significant interaction"
    );
}

/// A same-exclusion-group pair is excluded by `candidate_pairs` and writes
/// nothing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn same_group_pair_writes_nothing() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let rates = [[0.10, 0.12], [0.14, 0.60]];
    let (env, exp_a, exp_b, metric) = seed_pair(&ch, "grp", 100, rates).await;

    let reader = ClickHouseInteractionCells::new(ch.clone());
    let writer = ClickHouseInteractionWriter::new(ch.clone());
    let mut metrics = HashMap::new();
    metrics.insert(metric, conversion_metric(metric));
    let group = Uuid::new_v4();
    let mut exps = vec![
        meta(
            exp_a,
            Uuid::new_v4(),
            metric,
            Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        ),
        meta(
            exp_b,
            Uuid::new_v4(),
            metric,
            Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        ),
    ];
    exps[0].flag_id = Uuid::new_v4();
    exps[1].flag_id = Uuid::new_v4();
    exps[0].exclusion_group_id = Some(group);
    exps[1].exclusion_group_id = Some(group);

    let written = compute_and_persist_interactions(
        &reader,
        &writer,
        env,
        &exps,
        &metrics,
        &["user".to_string()],
        Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap(),
    )
    .await
    .expect("compute_and_persist should succeed");
    assert_eq!(written, 0, "same-group pair must be excluded");

    let rows = fetch_rows(&ch, exp_a).await;
    assert!(
        rows.is_empty(),
        "no interaction row for same-group pair, got {rows:?}"
    );
}

/// MED-2: a too-sparse grid yields `insufficient_data = true` (and is never
/// significant), with the sentinel 0.0 estimate / p-value persisted.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn insufficient_data_is_persisted() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    // Only one context per cell → far too sparse for a valid interaction test.
    let rates = [[0.0, 0.0], [0.0, 1.0]];
    let (env, exp_a, exp_b, metric) = seed_pair(&ch, "insuf", 1, rates).await;

    let reader = ClickHouseInteractionCells::new(ch.clone());
    let writer = ClickHouseInteractionWriter::new(ch.clone());
    let mut metrics = HashMap::new();
    metrics.insert(metric, conversion_metric(metric));
    let mut exps = vec![
        meta(
            exp_a,
            Uuid::new_v4(),
            metric,
            Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        ),
        meta(
            exp_b,
            Uuid::new_v4(),
            metric,
            Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap(),
        ),
    ];
    exps[0].flag_id = Uuid::new_v4();
    exps[1].flag_id = Uuid::new_v4();

    let written = compute_and_persist_interactions(
        &reader,
        &writer,
        env,
        &exps,
        &metrics,
        &["user".to_string()],
        Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap(),
    )
    .await
    .expect("compute_and_persist should succeed");
    assert_eq!(written, 1);

    let rows = fetch_rows(&ch, exp_a).await;
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].insufficient_data,
        "sparse grid must persist insufficient_data = true"
    );
    assert!(
        !rows[0].significant,
        "insufficient-data rows are never significant"
    );
}

/// HIGH-2 regression: events that occur BEFORE a context's joint exposure to
/// both experiments must NOT be counted (strict first-exposure / ITT). A
/// context whose only event fires before assignment must show `successes = 0`
/// in its cell, while a context with a post-exposure event counts.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn pre_exposure_events_are_excluded_itt() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let env_id = Uuid::new_v4();
    let exp_a = Uuid::new_v4();
    let exp_b = Uuid::new_v4();
    let iter_a = Uuid::new_v4();
    let iter_b = Uuid::new_v4();
    let flag_a = Uuid::new_v4();
    let flag_b = Uuid::new_v4();
    let metric = Uuid::new_v4();

    // Joint exposure for every context is `assigned` (both sides assigned then).
    let assigned = Utc.with_ymd_and_hms(2026, 5, 10, 0, 0, 0).unwrap();
    let pre = assigned - Duration::hours(1); // BEFORE exposure — must be ignored
    let post = assigned + Duration::hours(1); // AFTER exposure — must count
    let end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();

    // Build a 2x2 grid. In every cell, half the contexts get a POST event
    // (a genuine post-exposure conversion) and half get a PRE event ONLY
    // (pre-exposure noise that must be excluded → contributes 0 successes).
    let variants = ["control", "treatment"];
    let n_per_cell = 20usize;
    let mut assigns = Vec::new();
    let mut events = Vec::new();
    // Track expected post-exposure successes per (a,b) cell.
    let mut expected_succ: std::collections::HashMap<(String, String), u64> =
        std::collections::HashMap::new();

    for (ai, av) in variants.iter().enumerate() {
        for (bi, bv) in variants.iter().enumerate() {
            for k in 0..n_per_cell {
                let ckey = format!("itt_{ai}_{bi}_{k}");
                assigns.push(assignment(
                    exp_a, iter_a, env_id, flag_a, &ckey, av, assigned,
                ));
                assigns.push(assignment(
                    exp_b, iter_b, env_id, flag_b, &ckey, bv, assigned,
                ));
                if k % 2 == 0 {
                    // Genuine post-exposure conversion.
                    events.push(conversion_event(env_id, &ckey, post));
                    *expected_succ
                        .entry(((*av).to_string(), (*bv).to_string()))
                        .or_default() += 1;
                } else {
                    // Pre-exposure event ONLY — must be excluded, so this
                    // context must contribute 0 successes.
                    events.push(conversion_event(env_id, &ckey, pre));
                }
            }
        }
    }
    insert_assignments(&ch, &assigns).await;
    insert_events(&ch, &events).await;

    let reader = ClickHouseInteractionCells::new(ch.clone());
    let writer = ClickHouseInteractionWriter::new(ch.clone());
    let mut metrics = HashMap::new();
    metrics.insert(metric, conversion_metric(metric));
    let exps = vec![
        meta(exp_a, flag_a, metric, assigned),
        meta(exp_b, flag_b, metric, assigned),
    ];

    let written = compute_and_persist_interactions(
        &reader,
        &writer,
        env_id,
        &exps,
        &metrics,
        &["user".to_string()],
        end,
    )
    .await
    .expect("compute_and_persist should succeed");
    assert_eq!(written, 1);

    let rows = fetch_rows(&ch, exp_a).await;
    assert_eq!(rows.len(), 1, "exactly one interaction row, got {rows:?}");
    assert_eq!(rows[0].shared_count, (n_per_cell * 4) as u64);

    let cells: Vec<CellStat> =
        serde_json::from_str(&rows[0].cell_stats).expect("cell_stats is valid JSON");
    assert_eq!(cells.len(), 4, "2x2 grid of cells, got {cells:?}");

    for cell in &cells {
        let key = (cell.a_variant_key.clone(), cell.b_variant_key.clone());
        let expected = expected_succ.get(&key).copied().unwrap_or(0);
        assert_eq!(
            cell.n, n_per_cell as u64,
            "every cell still counts all contexts in n"
        );
        assert_eq!(
            cell.successes, expected,
            "cell {key:?}: only post-exposure events count as successes \
             (pre-exposure events excluded); got {cell:?}"
        );
        // Half the contexts had only a pre-exposure event → strictly fewer than n.
        assert!(
            cell.successes < cell.n,
            "pre-exposure-only contexts must not be successes ({cell:?})"
        );
    }
}
