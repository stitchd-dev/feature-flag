//! Live-ClickHouse integration test for **operator-bounded order-4** cross-
//! experiment interaction analysis (Phase 10, FR6).
//!
//! Seeds FOUR experiments on distinct flags, every shared context assigned a
//! variant in all four, sharing one conversion metric, then runs the full
//! `compute_and_persist_interactions` orchestration at `max_order = 4` through
//! the production ClickHouse reader/writer and asserts that the order-4 tuple's
//! full hierarchical decomposition — 4 main + 6 two-way + 4 three-way + 1
//! four-way = 15 terms — is produced, persisted, and BH-FDR-corrected, with a
//! planted four-way interaction surfaced as significant.
//!
//! Requires a running ClickHouse at `STITCHD_CLICKHOUSE_URL` (default
//! `http://localhost:8123`) with migrations applied. Tagged `#[ignore]`; run
//! with `cargo test -p stitchd-stats-service --test bandit_interaction_order4 --
//! --ignored`.

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

/// One persisted interaction term row (Array(UUID) stringified to Array(String)).
#[derive(Debug, Clone, Deserialize, clickhouse::Row)]
struct InteractionResultRow {
    experiment_ids_str: Vec<String>,
    interaction_order: u8,
    term: String,
    metric_key: String,
    significant: bool,
    insufficient_data: bool,
}

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
        insert.write(row).await.expect("write assignment row");
    }
    insert.end().await.expect("finalize assignments insert");
}

async fn insert_events(ch: &Client, rows: &[EventRow]) {
    let mut insert = ch
        .insert::<EventRow>("events")
        .await
        .expect("prepare events insert");
    for row in rows {
        insert.write(row).await.expect("write event row");
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

/// Seed FOUR experiments (distinct flags) over a shared 2×2×2×2 context grid:
/// every context is assigned a variant in all four experiments. Returns
/// `(env, [exp_a, exp_b, exp_c, exp_d], metric)`.
///
/// `rate(a,b,c,d)` is the per-corner conversion rate; the caller plants the
/// interaction structure (e.g. a genuine four-way) via this closure.
async fn seed_quad(
    ch: &Client,
    key_prefix: &str,
    n_per_cell: usize,
    rate: impl Fn(usize, usize, usize, usize) -> f64,
) -> (Uuid, [Uuid; 4], Uuid) {
    let env_id = Uuid::new_v4();
    let exps = [
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    ];
    let iters = [
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    ];
    let flags = [
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    ];
    let metric = Uuid::new_v4();
    let at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let evt_at = at + Duration::hours(1);
    let variants = ["control", "treatment"];

    let mut assigns = Vec::new();
    let mut events = Vec::new();
    for (ai, av) in variants.iter().enumerate() {
        for (bi, bv) in variants.iter().enumerate() {
            for (ci, cv) in variants.iter().enumerate() {
                for (di, dv) in variants.iter().enumerate() {
                    let r = rate(ai, bi, ci, di);
                    for k in 0..n_per_cell {
                        let ckey = format!("{key_prefix}_{ai}{bi}{ci}{di}_{k}");
                        assigns.push(assignment(
                            exps[0], iters[0], env_id, flags[0], &ckey, av, at,
                        ));
                        assigns.push(assignment(
                            exps[1], iters[1], env_id, flags[1], &ckey, bv, at,
                        ));
                        assigns.push(assignment(
                            exps[2], iters[2], env_id, flags[2], &ckey, cv, at,
                        ));
                        assigns.push(assignment(
                            exps[3], iters[3], env_id, flags[3], &ckey, dv, at,
                        ));
                        if (k as f64) < (n_per_cell as f64 * r) {
                            events.push(conversion_event(env_id, &ckey, evt_at));
                        }
                    }
                }
            }
        }
    }
    insert_assignments(ch, &assigns).await;
    insert_events(ch, &events).await;
    (env_id, exps, metric)
}

async fn fetch_rows(ch: &Client, exp: Uuid) -> Vec<InteractionResultRow> {
    let sql = format!(
        "SELECT arrayMap(x -> toString(x), experiment_ids) AS experiment_ids_str, \
         interaction_order, term, metric_key, significant, insufficient_data
         FROM experiment_interactions FINAL
         WHERE has(experiment_ids, toUUID('{exp}'))
         ORDER BY interaction_order, term"
    );
    ch.query(&sql)
        .fetch_all::<InteractionResultRow>()
        .await
        .expect("read experiment_interactions")
}

/// Four overlapping experiments at `max_order = 4` produce the order-4 tuple's
/// full hierarchical decomposition (15 terms incl. the top four-way), and a
/// planted four-way interaction is surfaced as significant after BH-FDR.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn order4_sweep_produces_decomposed_fdr_corrected_terms() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    // Plant a genuine FOUR-way interaction: the (a,b,c) three-way bump exists
    // only in the d = treatment slice (and moves elsewhere in d = control), so
    // only the top four-way term carries the signal. Base rate 10%, lifted 60%.
    let rate = |a: usize, b: usize, c: usize, d: usize| {
        let lifted = if d == 1 {
            (a, b, c) == (1, 1, 1)
        } else {
            (a, b, c) == (1, 1, 0)
        };
        if lifted { 0.60 } else { 0.10 }
    };
    let (env, exps, metric) = seed_quad(&ch, "ord4", 80, rate).await;

    let reader = ClickHouseInteractionCells::new(ch.clone());
    let writer = ClickHouseInteractionWriter::new(ch.clone());
    let mut metrics = HashMap::new();
    metrics.insert(metric, conversion_metric(metric));
    let started = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let mut metas: Vec<ExperimentMeta> = exps
        .iter()
        .map(|&e| meta(e, Uuid::new_v4(), metric, started))
        .collect();
    // Distinct flags so all pairs/triples/quad are candidates.
    for m in &mut metas {
        m.flag_id = Uuid::new_v4();
    }

    let written = compute_and_persist_interactions(
        &reader,
        &writer,
        env,
        &metas,
        &metrics,
        &["user".to_string()],
        Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap(),
        4, // operator-bounded max interaction order
    )
    .await
    .expect("compute_and_persist should succeed");

    // C(4,2)=6 pairs ×3 + C(4,3)=4 triples ×7 + C(4,4)=1 quad ×15 = 61.
    assert_eq!(
        written,
        6 * 3 + 4 * 7 + 15,
        "pairs + triples + the order-4 quad's full decomposition"
    );

    // Rows touching exp[0] include the single order-4 tuple over all four.
    let rows = fetch_rows(&ch, exps[0]).await;
    let order4: Vec<_> = rows.iter().filter(|r| r.interaction_order == 4).collect();
    assert_eq!(
        order4.len(),
        15,
        "the quad's full hierarchical set (4 main + 6 two-way + 4 three-way + 1 four-way), got {order4:?}"
    );
    for r in &order4 {
        assert_eq!(r.metric_key, "checkout");
        assert_eq!(r.experiment_ids_str.len(), 4, "four participating ids");
    }
    let fourway: Vec<_> = order4
        .iter()
        .filter(|r| r.term.starts_with("4way:"))
        .collect();
    assert_eq!(fourway.len(), 1, "exactly one top four-way term");
    let threeway = order4
        .iter()
        .filter(|r| r.term.starts_with("3way:"))
        .count();
    assert_eq!(threeway, 4, "all four three-way subsets within the quad");
    let mains = order4
        .iter()
        .filter(|r| r.term.starts_with("main:"))
        .count();
    assert_eq!(mains, 4, "one main effect per participating experiment");

    // The planted four-way must be testable and BH-FDR-significant.
    let four = fourway[0];
    assert!(
        !four.insufficient_data,
        "the well-powered quad's four-way must be testable"
    );
    assert!(
        four.significant,
        "planted four-way interaction must survive BH-FDR as significant"
    );
}

/// The default cap (`max_order = 3`) over the SAME four experiments persists NO
/// order-4 rows — the operator-bounded cap is enforced end-to-end.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse"]
async fn default_cap_persists_no_order4_rows() {
    let ch = make_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let rate = |a: usize, b: usize, c: usize, d: usize| {
        if (a, b, c, d) == (1, 1, 1, 1) {
            0.50
        } else {
            0.10
        }
    };
    let (env, exps, metric) = seed_quad(&ch, "cap3", 60, rate).await;

    let reader = ClickHouseInteractionCells::new(ch.clone());
    let writer = ClickHouseInteractionWriter::new(ch.clone());
    let mut metrics = HashMap::new();
    metrics.insert(metric, conversion_metric(metric));
    let started = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let mut metas: Vec<ExperimentMeta> = exps
        .iter()
        .map(|&e| meta(e, Uuid::new_v4(), metric, started))
        .collect();
    for m in &mut metas {
        m.flag_id = Uuid::new_v4();
    }

    let written = compute_and_persist_interactions(
        &reader,
        &writer,
        env,
        &metas,
        &metrics,
        &["user".to_string()],
        Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap(),
        3, // default cap — no order-4
    )
    .await
    .expect("compute_and_persist should succeed");

    // C(4,2)=6 pairs ×3 + C(4,3)=4 triples ×7 = 46; no order-4.
    assert_eq!(written, 6 * 3 + 4 * 7);
    let rows = fetch_rows(&ch, exps[0]).await;
    assert!(
        rows.iter().all(|r| r.interaction_order <= 3),
        "the order-3 cap must persist no order-4 row"
    );
}
