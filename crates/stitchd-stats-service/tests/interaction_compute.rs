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

/// One persisted interaction **term** row. The `Array(UUID)` column is read as
/// `Array(String)` under the alias `experiment_ids_str` (the `fetch_rows` SQL
/// stringifies it via `arrayMap`) to sidestep the array-UUID RowBinary serde on
/// the read side. clickhouse-rs matches columns by NAME, so the field name must
/// equal the SQL alias.
#[derive(Debug, Clone, Deserialize, clickhouse::Row)]
struct InteractionResultRow {
    experiment_ids_str: Vec<String>,
    interaction_order: u8,
    term: String,
    context_type: String,
    metric_key: String,
    shared_count: u64,
    cell_stats: String,
    significant: bool,
    insufficient_data: bool,
}

/// Decoded `cell_stats` JSON cell (mirrors `NdCellAggregate`).
#[derive(Debug, Clone, Deserialize)]
struct CellStat {
    variant_keys: Vec<String>,
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
    // Stringify the Array(UUID) column so it reads as Array(String) — avoids the
    // array-UUID RowBinary serde on the read side. `FINAL` collapses the
    // ReplacingMergeTree window so re-inserts within a tick don't duplicate.
    // Alias the stringified projection to a NON-colliding name: reusing
    // `experiment_ids` makes ClickHouse try to unify the String[] alias with the
    // UUID[] column referenced in WHERE/ORDER BY (NO_COMMON_TYPE). Row decoding is
    // positional, so the alias name is irrelevant to deserialization.
    let sql = format!(
        "SELECT arrayMap(x -> toString(x), experiment_ids) AS experiment_ids_str, \
         interaction_order, term, context_type, metric_key, shared_count, \
         cell_stats, significant, insufficient_data
         FROM experiment_interactions FINAL
         WHERE has(experiment_ids, toUUID('{exp}'))
         ORDER BY interaction_order, term"
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
    assert_eq!(
        written, 3,
        "one pair × metric × context → 2 main + 1 two-way"
    );

    let rows = fetch_rows(&ch, exp_a).await;
    assert_eq!(
        rows.len(),
        3,
        "the full pairwise decomposition, got {rows:?}"
    );
    // The persisted tuple is the sorted (min, max) ordering candidate_pairs emits.
    let (lo, hi) = (exp_a.min(exp_b), exp_a.max(exp_b));
    for r in &rows {
        assert_eq!(r.metric_key, "checkout");
        assert_eq!(r.context_type, "user");
        assert_eq!(r.shared_count, 400);
        assert_eq!(r.interaction_order, 2);
        assert_eq!(r.experiment_ids_str, vec![lo.to_string(), hi.to_string()]);
    }
    let two = rows
        .iter()
        .find(|r| r.term.starts_with("2way:"))
        .expect("two-way term present");
    assert_eq!(two.term, format!("2way:{lo}x{hi}"));
    assert!(
        two.significant,
        "planted super-additive interaction must be significant"
    );
    // MED-2: a well-powered grid is not insufficient.
    assert!(
        !two.insufficient_data,
        "ample-data grid must persist insufficient_data = false"
    );
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
    assert_eq!(written, 3);

    let rows = fetch_rows(&ch, exp_a).await;
    assert_eq!(rows.len(), 3);
    let two = rows
        .iter()
        .find(|r| r.term.starts_with("2way:"))
        .expect("two-way term present");
    assert!(
        !two.significant,
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
    assert_eq!(written, 3);

    let rows = fetch_rows(&ch, exp_a).await;
    assert_eq!(rows.len(), 3);
    let two = rows
        .iter()
        .find(|r| r.term.starts_with("2way:"))
        .expect("two-way term present");
    assert!(
        two.insufficient_data,
        "sparse grid must persist insufficient_data = true"
    );
    assert!(
        !two.significant,
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
    assert_eq!(written, 3, "pairwise decomposition: 2 main + 1 two-way");

    let rows = fetch_rows(&ch, exp_a).await;
    assert_eq!(
        rows.len(),
        3,
        "the pairwise decomposition rows, got {rows:?}"
    );
    // The cell grid is shared across all term rows of the tuple.
    let row = &rows[0];
    assert_eq!(row.shared_count, (n_per_cell * 4) as u64);

    let cells: Vec<CellStat> =
        serde_json::from_str(&row.cell_stats).expect("cell_stats is valid JSON");
    assert_eq!(cells.len(), 4, "2x2 grid of cells, got {cells:?}");

    for cell in &cells {
        assert_eq!(cell.variant_keys.len(), 2, "pairwise variant tuple");
        let key = (cell.variant_keys[0].clone(), cell.variant_keys[1].clone());
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

/// Seed THREE experiments (distinct flags) over a shared 2×2×2 context grid —
/// every context is assigned a variant in all three experiments. Returns
/// `(env, exp_a, exp_b, exp_c, metric_id)`.
async fn seed_triple(
    ch: &Client,
    key_prefix: &str,
    n_per_cell: usize,
) -> (Uuid, Uuid, Uuid, Uuid, Uuid) {
    let env_id = Uuid::new_v4();
    let (exp_a, exp_b, exp_c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (iter_a, iter_b, iter_c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let (flag_a, flag_b, flag_c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let metric = Uuid::new_v4();
    let at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let evt_at = at + Duration::hours(1);
    let variants = ["control", "treatment"];

    let mut assigns = Vec::new();
    let mut events = Vec::new();
    for (ai, av) in variants.iter().enumerate() {
        for (bi, bv) in variants.iter().enumerate() {
            for (ci, cv) in variants.iter().enumerate() {
                for k in 0..n_per_cell {
                    let ckey = format!("{key_prefix}_{ai}{bi}{ci}_{k}");
                    assigns.push(assignment(exp_a, iter_a, env_id, flag_a, &ckey, av, at));
                    assigns.push(assignment(exp_b, iter_b, env_id, flag_b, &ckey, bv, at));
                    assigns.push(assignment(exp_c, iter_c, env_id, flag_c, &ckey, cv, at));
                    if k % 2 == 0 {
                        events.push(conversion_event(env_id, &ckey, evt_at));
                    }
                }
            }
        }
    }
    insert_assignments(ch, &assigns).await;
    insert_events(ch, &events).await;
    (env_id, exp_a, exp_b, exp_c, metric)
}

/// Regression for the ReplacingMergeTree dedup-key fix (review #1) + the
/// order-2/order-3 sub-term distinction (review #7): with three overlapping
/// experiments sharing one metric, the sweep emits pairs AND the triple, and a
/// `main:<exp>` term is produced once per tuple containing that experiment. The
/// fix added `experiment_ids` to the table's ORDER BY, so those same-order,
/// same-term rows from DIFFERENT tuples must NOT collapse under FINAL.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn three_way_sweep_keeps_distinct_main_effect_rows() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let (env, exp_a, exp_b, exp_c, metric) = seed_triple(&ch, "nway", 40).await;

    let reader = ClickHouseInteractionCells::new(ch.clone());
    let writer = ClickHouseInteractionWriter::new(ch.clone());
    let mut metrics = HashMap::new();
    metrics.insert(metric, conversion_metric(metric));
    let started = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let mut exps = vec![
        meta(exp_a, Uuid::new_v4(), metric, started),
        meta(exp_b, Uuid::new_v4(), metric, started),
        meta(exp_c, Uuid::new_v4(), metric, started),
    ];
    // distinct flags so all three pairwise + the triple are candidates
    exps[0].flag_id = Uuid::new_v4();
    exps[1].flag_id = Uuid::new_v4();
    exps[2].flag_id = Uuid::new_v4();

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
    // 3 pairs × (2 main + 1 two-way) + 1 triple × (3 main + 3 two-way + 1 three-way)
    assert_eq!(written, 16, "pairs + triple full decomposition");

    // Rows where exp_a participates: pairs {A,B},{A,C} and triple {A,B,C}.
    let rows = fetch_rows(&ch, exp_a).await;
    let main_a = format!("main:{exp_a}");
    let main_a_rows: Vec<_> = rows.iter().filter(|r| r.term == main_a).collect();
    // THE FIX: 3 distinct main:A rows survive FINAL (one per tuple containing A).
    // Pre-fix, the two order-2 rows ({A,B},{A,C}) shared a dedup key and one was
    // silently lost, leaving only 2.
    assert_eq!(
        main_a_rows.len(),
        3,
        "main:A must persist once per tuple containing A (no FINAL collapse): {main_a_rows:?}"
    );
    let distinct_tuples: std::collections::HashSet<_> = main_a_rows
        .iter()
        .map(|r| r.experiment_ids_str.clone())
        .collect();
    assert_eq!(
        distinct_tuples.len(),
        3,
        "the 3 main:A rows name 3 distinct tuples"
    );
    let order2 = main_a_rows
        .iter()
        .filter(|r| r.interaction_order == 2)
        .count();
    let order3 = main_a_rows
        .iter()
        .filter(|r| r.interaction_order == 3)
        .count();
    assert_eq!(
        (order2, order3),
        (2, 1),
        "two pairwise + one three-way main:A"
    );
}
