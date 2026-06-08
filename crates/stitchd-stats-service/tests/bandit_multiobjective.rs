//! Live-ClickHouse + live-Postgres integration test for the END-TO-END
//! multi-objective bandit (FR9: scalarization + constrained guardrails).
//!
//! Self-seeds the minimal PG FK chain (organisation, project, environment, flag,
//! experiment, iteration) plus `experiment_assignments` + `events` for TWO count
//! metrics in ClickHouse, then runs `run_bandit_reallocation` with the real
//! `ClickHouseCellReader` for two scenarios:
//!
//! (a) **Scalarization weight shift** — variant `a` is strong on metric 1 / weak
//!     on metric 2, variant `b` is the reverse. Weighting metric 1 heavily makes
//!     `a` the winner; re-running with metric 2 weighted heavily flips the winner
//!     to `b`. Proves the scalarized combined reward (multi-metric ClickHouse
//!     reads → `reward_arms` → allocator) drives the winner end-to-end.
//!
//! (b) **Constrained guardrail down-weight** — variant `b` has the strongest
//!     PRIMARY metric but VIOLATES a guardrail-constraint metric's bound, so it is
//!     held at the exploration floor while `a` (compliant) takes the rest. Proves
//!     a guardrail violation down-weights an otherwise-best arm.
//!
//! Also asserts the per-objective posteriors are surfaced on the recorded
//! `bandit_allocation_runs.new_allocation` JSON under `bandit_objectives`
//! (FR9 surfacing for Phase 11).
//!
//! Tagged `#[ignore]`; run explicitly with:
//!
//! ```sh
//! DATABASE_URL=… cargo test -p stitchd-stats-service --test bandit_multiobjective -- --ignored
//! ```

#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use clickhouse::Client;
use uuid::Uuid;

use stitchd_core::experimentation::bandit::{
    BanditAlgorithm, BanditConfig, ConstraintDirection, ExperimentMode, GuardrailConstraint,
    LifecyclePolicy, ObjectiveWeight, PropagationMode, RewardObjective,
};
use stitchd_core::id::{EnvironmentId, MetricId};
use stitchd_core::metric::{
    AggregationConfig, AggregationOperator, GoalDirection, MetricDefinition, MetricKind,
};
use stitchd_core::rollout::RolloutDistribution;
use stitchd_db::clickhouse::SeedAssignmentRow;
use stitchd_event_writer::SeedEventRow;
use stitchd_stats_service::bandit::{
    AllocationApplier, ApplyResult, BanditRunOutcome, PgRunRecorder, run_bandit_reallocation,
};
use stitchd_stats_service::compute::ClickHouseCellReader;
use stitchd_stats_service::scheduler::{RunningExperiment, SequentialSettings};

type AssignmentRow = SeedAssignmentRow;
type EventRow = SeedEventRow;

fn make_ch_client() -> Client {
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

/// A fake applier that always returns APPLIED, capturing the distribution.
#[derive(Default)]
struct CapturingApplier {
    last: Mutex<Option<RolloutDistribution>>,
}

#[async_trait::async_trait]
impl AllocationApplier for CapturingApplier {
    async fn apply(
        &self,
        _experiment_id: Uuid,
        allocation: &RolloutDistribution,
    ) -> Result<ApplyResult, anyhow::Error> {
        *self.last.lock().unwrap() = Some(allocation.clone());
        Ok(ApplyResult::Applied {
            resolved_target: "default_rule".to_string(),
            new_version: 2,
        })
    }

    async fn apply_realtime(
        &self,
        _experiment_id: Uuid,
        allocation: &RolloutDistribution,
        _model: stitchd_proto::flags::v1::RealtimeBanditModel,
    ) -> Result<ApplyResult, anyhow::Error> {
        *self.last.lock().unwrap() = Some(allocation.clone());
        Ok(ApplyResult::Applied {
            resolved_target: "rule".to_string(),
            new_version: 2,
        })
    }
}

/// Seed the minimal PG FK chain and return `(env_id, exp_id, iter_id)`.
async fn seed_pg_chain(pool: &sqlx::PgPool) -> (Uuid, Uuid, Uuid) {
    let org_id = Uuid::new_v4();
    let project_id = Uuid::new_v4();
    let env_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let exp_id = Uuid::new_v4();
    let iter_id = Uuid::new_v4();

    sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind("bandit-mo-org")
        .execute(pool)
        .await
        .expect("seed organisation");
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(org_id)
        .bind("bandit-mo-project")
        .execute(pool)
        .await
        .expect("seed project");
    sqlx::query("INSERT INTO environments (id, project_id, name) VALUES ($1, $2, $3)")
        .bind(env_id)
        .bind(project_id)
        .bind("bandit-mo-env")
        .execute(pool)
        .await
        .expect("seed environment");
    sqlx::query(
        "INSERT INTO feature_flags (id, project_id, key, name, value_type, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(flag_id)
    .bind(project_id)
    .bind(format!("bandit_mo_flag_{}", &exp_id.to_string()[..8]))
    .bind("bandit mo flag")
    .bind("boolean")
    .bind(true)
    .execute(pool)
    .await
    .expect("seed feature flag");
    sqlx::query(
        "INSERT INTO experiments \
         (id, env_id, flag_id, name, status, experiment_mode, targets_default_rule) \
         VALUES ($1, $2, $3, $4, $5, $6, true)",
    )
    .bind(exp_id)
    .bind(env_id)
    .bind(flag_id)
    .bind("bandit mo experiment")
    .bind("running")
    .bind("bandit")
    .execute(pool)
    .await
    .expect("seed experiment");
    sqlx::query(
        "INSERT INTO experiment_iterations \
         (id, experiment_id, flag_id, iteration_number, started_at, traffic_allocation) \
         VALUES ($1, $2, $3, $4, now(), $5)",
    )
    .bind(iter_id)
    .bind(exp_id)
    .bind(flag_id)
    .bind(1_i32)
    .bind(100.0_f64)
    .execute(pool)
    .await
    .expect("seed iteration");

    (env_id, exp_id, iter_id)
}

/// Seed assignments for two variants (a, b) once.
async fn seed_assignments(
    ch: &Client,
    env_id: Uuid,
    exp_id: Uuid,
    iter_id: Uuid,
    n_per_variant: usize,
    assigned_at: DateTime<Utc>,
) {
    let flag_id = Uuid::new_v4();
    let mk_assignment = |variant: &str, key: String| AssignmentRow {
        experiment_id: exp_id,
        iteration_id: iter_id,
        env_id,
        flag_id,
        matched_rule_id: None,
        context_type: "user".into(),
        context_key: key,
        variant_key: variant.into(),
        assigned_at,
        version: -assigned_at.timestamp_millis(),
    };
    let mut assignments = Vec::with_capacity(2 * n_per_variant);
    for i in 0..n_per_variant {
        assignments.push(mk_assignment("a", format!("a_{i}")));
        assignments.push(mk_assignment("b", format!("b_{i}")));
    }
    insert_assignments(ch, &assignments).await;
}

/// Seed conversion events for one metric: `a_conversions` on variant a's units,
/// `b_conversions` on variant b's units.
async fn seed_metric_events(
    ch: &Client,
    env_id: Uuid,
    metric_key: &str,
    a_conversions: usize,
    b_conversions: usize,
    event_at: DateTime<Utc>,
) {
    let mk_event = |key: String| EventRow {
        env_id,
        contexts: vec![("user".into(), key)],
        metric_key: metric_key.to_string(),
        value_bool: None,
        value_int: None,
        value_double: None,
        timestamp: event_at,
        ingested_at: event_at,
        properties: vec![],
        occurred_at: event_at,
    };
    let mut events = Vec::new();
    for i in 0..a_conversions {
        events.push(mk_event(format!("a_{i}")));
    }
    for i in 0..b_conversions {
        events.push(mk_event(format!("b_{i}")));
    }
    insert_events(ch, &events).await;
}

fn count_metric(id: Uuid, env_id: Uuid, key: &str) -> MetricDefinition {
    let now = Utc::now();
    MetricDefinition {
        id: MetricId::from_uuid(id),
        environment_id: EnvironmentId::from_uuid(env_id),
        key: key.to_string(),
        name: key.to_string(),
        description: None,
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: key.to_string(),
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

fn weight_of(dist: &RolloutDistribution, key: &str) -> u32 {
    dist.allocations
        .iter()
        .find(|a| a.variant_key == key)
        .map(|a| a.percentage_bp)
        .unwrap_or(0)
}

async fn run(
    ch: &Client,
    pool: &sqlx::PgPool,
    exp: &RunningExperiment,
    metrics: &HashMap<Uuid, MetricDefinition>,
    iter_end: DateTime<Utc>,
) -> RolloutDistribution {
    let reader = ClickHouseCellReader::new(Arc::new(ch.clone()));
    let applier = CapturingApplier::default();
    let recorder = PgRunRecorder::new(pool.clone());
    let tick = iter_end + Duration::days(1);
    let outcome =
        run_bandit_reallocation(&reader, &applier, &recorder, exp, metrics, iter_end, tick)
            .await
            .expect("bandit reallocation should not error against live infra");
    match outcome {
        BanditRunOutcome::Applied { allocation, .. } => allocation,
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse + postgres"]
async fn multiobjective_scalarization_shifts_winner_and_guardrail_downweights() {
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must point at a migrated verify database");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .expect("connect to postgres");

    let ch = make_ch_client();
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("apply CH migrations");

    let (env_id, exp_id, iter_id) = seed_pg_chain(&pool).await;

    let assigned_at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let iter_end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();
    let event_at = assigned_at + Duration::hours(1);

    // Two metrics, 1000 units/variant.
    let m1_id = Uuid::new_v4();
    let m2_id = Uuid::new_v4();
    let m1_key = format!("m1_{}", &exp_id.to_string()[..8]);
    let m2_key = format!("m2_{}", &exp_id.to_string()[..8]);

    seed_assignments(&ch, env_id, exp_id, iter_id, 1000, assigned_at).await;
    // metric 1: a strong (700), b weak (100).
    seed_metric_events(&ch, env_id, &m1_key, 700, 100, event_at).await;
    // metric 2: a weak (100), b strong (700).
    seed_metric_events(&ch, env_id, &m2_key, 100, 700, event_at).await;

    let m1 = count_metric(m1_id, env_id, &m1_key);
    let m2 = count_metric(m2_id, env_id, &m2_key);
    let metrics: HashMap<Uuid, MetricDefinition> = [(m1_id, m1.clone()), (m2_id, m2.clone())]
        .into_iter()
        .collect();

    let base_exp = RunningExperiment {
        experiment_id: exp_id,
        env_id,
        iteration_id: iter_id,
        metric_ids: vec![m1_id, m2_id],
        variant_keys: vec!["a".into(), "b".into()],
        started_at: assigned_at,
        unit_context_types: vec!["user".into()],
        pre_period_days: 0,
        sequential: SequentialSettings::default(),
        variant_expected_bp: HashMap::new(),
        experiment_mode: ExperimentMode::Bandit,
        bandit_config: None,
        bandit_campaign_id: None,
    };

    // ── (a) Scalarization: weight metric 1 heavily → a wins. ─────────────────
    let mut exp_m1 = base_exp.clone();
    exp_m1.bandit_config = Some(BanditConfig {
        algorithm: BanditAlgorithm::ThompsonSampling,
        propagation_mode: PropagationMode::Static,
        min_exploration_bp: 500,
        objective: RewardObjective::Scalarized {
            weights: vec![
                ObjectiveWeight {
                    metric_id: m1_id,
                    weight: 1.0,
                },
                ObjectiveWeight {
                    metric_id: m2_id,
                    weight: 0.0,
                },
            ],
        },
        lifecycle_policy: LifecyclePolicy::Advisory,
        convergence_prob_threshold: 0.95,
    });
    let dist_m1 = run(&ch, &pool, &exp_m1, &metrics, iter_end).await;
    assert!(
        weight_of(&dist_m1, "a") > weight_of(&dist_m1, "b"),
        "weighting metric 1 should make a win: {dist_m1:?}"
    );

    // ── (a') Flip: weight metric 2 heavily → b wins. ─────────────────────────
    let mut exp_m2 = base_exp.clone();
    exp_m2.bandit_config = Some(BanditConfig {
        algorithm: BanditAlgorithm::ThompsonSampling,
        propagation_mode: PropagationMode::Static,
        min_exploration_bp: 500,
        objective: RewardObjective::Scalarized {
            weights: vec![
                ObjectiveWeight {
                    metric_id: m1_id,
                    weight: 0.0,
                },
                ObjectiveWeight {
                    metric_id: m2_id,
                    weight: 1.0,
                },
            ],
        },
        lifecycle_policy: LifecyclePolicy::Advisory,
        convergence_prob_threshold: 0.95,
    });
    let dist_m2 = run(&ch, &pool, &exp_m2, &metrics, iter_end).await;
    assert!(
        weight_of(&dist_m2, "b") > weight_of(&dist_m2, "a"),
        "weighting metric 2 should flip the winner to b: {dist_m2:?}"
    );

    // ── (b) Constrained guardrail: b has the strongest primary (metric 2) but
    //        VIOLATES a guardrail on metric 1 (at-most 0.20; b's metric-1 rate is
    //        0.10, a's is 0.70 → A is the violator? No: we want b excluded). Make
    //        the guardrail "metric 1 must be AT LEAST 0.50": b (0.10) violates,
    //        a (0.70) complies → b dropped to floor despite strong primary. ──────
    let mut exp_c = base_exp.clone();
    exp_c.bandit_config = Some(BanditConfig {
        algorithm: BanditAlgorithm::Ucb { c: 0.5 },
        propagation_mode: PropagationMode::Static,
        min_exploration_bp: 500,
        objective: RewardObjective::Constrained {
            primary_metric_id: m2_id, // b is best on metric 2
            constraints: vec![GuardrailConstraint {
                metric_id: m1_id,
                bound: 0.50,
                direction: ConstraintDirection::AtLeast, // metric-1 rate must be ≥ 0.50
            }],
        },
        lifecycle_policy: LifecyclePolicy::Advisory,
        convergence_prob_threshold: 0.95,
    });
    let dist_c = run(&ch, &pool, &exp_c, &metrics, iter_end).await;
    assert_eq!(
        weight_of(&dist_c, "b"),
        500,
        "b violates the metric-1 guardrail → held at the exploration floor: {dist_c:?}"
    );
    assert_eq!(
        weight_of(&dist_c, "a"),
        9500,
        "a (compliant) takes the rest: {dist_c:?}"
    );

    // ── Per-objective posteriors surfaced on the recorded constrained row. ────
    let new_alloc: serde_json::Value = sqlx::query_scalar(
        "SELECT new_allocation FROM bandit_allocation_runs \
         WHERE experiment_id = $1 AND iteration_id = $2 AND action = 'reallocate' \
         ORDER BY fired_at DESC LIMIT 1",
    )
    .bind(exp_id)
    .bind(iter_id)
    .fetch_one(&pool)
    .await
    .expect("a reallocate row should exist");
    let objectives = new_alloc["bandit_objectives"]["objectives"]
        .as_array()
        .expect("bandit_objectives surfaced for Phase 11");
    assert_eq!(objectives.len(), 2, "primary + guardrail surfaced");
    let guard = objectives
        .iter()
        .find(|o| o["role"] == "guardrail")
        .expect("guardrail objective surfaced");
    let gb = guard["variants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["variant_key"] == "b")
        .unwrap();
    assert_eq!(
        gb["guardrail_violated"], true,
        "b flagged as guardrail violator in the surfaced posteriors: {guard}"
    );

    println!(
        "multiobjective: scalarize(m1) a={} b={}; scalarize(m2) a={} b={}; constrained a={} b={}",
        weight_of(&dist_m1, "a"),
        weight_of(&dist_m1, "b"),
        weight_of(&dist_m2, "a"),
        weight_of(&dist_m2, "b"),
        weight_of(&dist_c, "a"),
        weight_of(&dist_c, "b"),
    );

    // Cleanup seeded PG rows.
    let _ = sqlx::query("DELETE FROM bandit_allocation_runs WHERE experiment_id = $1")
        .bind(exp_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM experiment_iterations WHERE experiment_id = $1")
        .bind(exp_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM experiments WHERE id = $1")
        .bind(exp_id)
        .execute(&pool)
        .await;
}
