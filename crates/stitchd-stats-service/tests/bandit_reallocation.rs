//! Live-ClickHouse + live-Postgres integration test for the bandit static
//! reallocation pass.
//!
//! Self-seeds the minimal PG FK chain (organisation, project, environment,
//! flag, experiment, iteration) so a `bandit_allocation_runs` row can be
//! inserted under its foreign keys, plus `experiment_assignments` + `events` in
//! ClickHouse for a 2-variant count experiment with a CLEAR reward effect
//! (treatment converts ~3x control). It then runs `run_bandit_reallocation`
//! with the real `ClickHouseCellReader` and the real `PgRunRecorder` (a fake
//! applier stands in for the experimentation-service / flag-service write path),
//! and asserts that the pass produced an APPLIED outcome with a NON-uniform
//! allocation shifting traffic toward the better arm (treatment > control,
//! every arm at or above the floor, summing to 10000 bp), and that a
//! `bandit_allocation_runs` row was recorded (`action = reallocate`,
//! `outcome = applied`, `new_allocation` carrying the computed weights).
//!
//! Tagged `#[ignore]` so the default `cargo test` run needs no infrastructure.
//! Run explicitly with:
//!
//! ```sh
//! DATABASE_URL=… cargo test -p stitchd-stats-service --test bandit_reallocation -- --ignored
//! ```

#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use clickhouse::Client;
use uuid::Uuid;

use stitchd_core::experimentation::bandit::{
    BanditAlgorithm, BanditConfig, ExperimentMode, LifecyclePolicy, PropagationMode,
    RewardObjective,
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

/// A fake applier that always returns APPLIED, capturing the distribution it was
/// handed (so the test can assert the EXACT weights the pass computed).
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
        .bind("bandit-test-org")
        .execute(pool)
        .await
        .expect("seed organisation");
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(org_id)
        .bind("bandit-test-project")
        .execute(pool)
        .await
        .expect("seed project");
    sqlx::query("INSERT INTO environments (id, project_id, name) VALUES ($1, $2, $3)")
        .bind(env_id)
        .bind(project_id)
        .bind("bandit-test-env")
        .execute(pool)
        .await
        .expect("seed environment");
    sqlx::query(
        "INSERT INTO feature_flags (id, project_id, key, name, value_type, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(flag_id)
    .bind(project_id)
    .bind(format!("bandit_flag_{}", &exp_id.to_string()[..8]))
    .bind("bandit flag")
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
    .bind("bandit experiment")
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

/// Seed N units per variant in ClickHouse with a per-variant conversion count.
#[allow(clippy::too_many_arguments)]
async fn seed_count_assignments_events(
    ch: &Client,
    env_id: Uuid,
    exp_id: Uuid,
    iter_id: Uuid,
    metric_key: &str,
    n_per_variant: usize,
    control_conversions: usize,
    treatment_conversions: usize,
) -> DateTime<Utc> {
    let flag_id = Uuid::new_v4();
    let assigned_at = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let iter_end = Utc.with_ymd_and_hms(2026, 5, 31, 0, 0, 0).unwrap();
    let event_at = assigned_at + Duration::hours(1);

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
        assignments.push(mk_assignment("control", format!("c_{i}")));
        assignments.push(mk_assignment("treatment", format!("t_{i}")));
    }
    insert_assignments(ch, &assignments).await;

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
    for i in 0..control_conversions {
        events.push(mk_event(format!("c_{i}")));
    }
    for i in 0..treatment_conversions {
        events.push(mk_event(format!("t_{i}")));
    }
    insert_events(ch, &events).await;

    iter_end
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse + postgres"]
async fn bandit_reallocation_shifts_toward_better_arm_and_records_run() {
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

    // 1) Seed the PG FK chain so the history row can be inserted.
    let (env_id, exp_id, iter_id) = seed_pg_chain(&pool).await;

    // 2) Seed CH: 1000/variant, control 100 (10 %) vs treatment 300 (30 %).
    let metric_id = Uuid::new_v4();
    let metric_key = format!("conv_{}", &exp_id.to_string()[..8]);
    let iter_end =
        seed_count_assignments_events(&ch, env_id, exp_id, iter_id, &metric_key, 1000, 100, 300)
            .await;

    // 3) Build the running bandit experiment (Thompson, static, floor 500 bp).
    let now = Utc::now();
    let metric = MetricDefinition {
        id: MetricId::from_uuid(metric_id),
        environment_id: EnvironmentId::from_uuid(env_id),
        key: metric_key.clone(),
        name: "conversion".into(),
        description: None,
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: metric_key.clone(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };
    let metrics: HashMap<Uuid, MetricDefinition> =
        [(metric_id, metric.clone())].into_iter().collect();

    let config = BanditConfig {
        algorithm: BanditAlgorithm::ThompsonSampling,
        propagation_mode: PropagationMode::Static,
        min_exploration_bp: 500,
        objective: RewardObjective::Scalar { metric_id },
        lifecycle_policy: LifecyclePolicy::Advisory,
        convergence_prob_threshold: 0.95,
    };
    let exp = RunningExperiment {
        experiment_id: exp_id,
        env_id,
        iteration_id: iter_id,
        metric_ids: vec![metric_id],
        variant_keys: vec!["control".into(), "treatment".into()],
        started_at: now,
        unit_context_types: vec!["user".into()],
        pre_period_days: 0,
        sequential: SequentialSettings::default(),
        variant_expected_bp: HashMap::new(),
        experiment_mode: ExperimentMode::Bandit,
        bandit_config: Some(config),
    };

    // 4) Run the pass against the real CH reader + real PG recorder.
    let reader = ClickHouseCellReader::new(Arc::new(ch.clone()));
    let applier = CapturingApplier::default();
    let recorder = PgRunRecorder::new(pool.clone());
    let tick = iter_end + Duration::days(1);

    let outcome =
        run_bandit_reallocation(&reader, &applier, &recorder, &exp, &metrics, iter_end, tick)
            .await
            .expect("bandit reallocation should not error against live infra");

    // 5) Assert APPLIED + non-uniform, better-arm-favouring distribution.
    let allocation = match outcome {
        BanditRunOutcome::Applied { allocation, .. } => allocation,
        other => panic!("expected Applied, got {other:?}"),
    };
    let weight = |k: &str| {
        allocation
            .allocations
            .iter()
            .find(|a| a.variant_key == k)
            .map(|a| a.percentage_bp)
            .unwrap_or(0)
    };
    let sum: u32 = allocation.allocations.iter().map(|a| a.percentage_bp).sum();
    assert_eq!(sum, 10_000, "allocation must sum to 10000 bp");
    assert!(
        weight("control") >= 500,
        "control below floor: {allocation:?}"
    );
    assert!(
        weight("treatment") >= 500,
        "treatment below floor: {allocation:?}"
    );
    assert!(
        weight("treatment") > weight("control"),
        "treatment (30%) should get more traffic than control (10%): {allocation:?}"
    );
    // The fake applier should have been handed the same distribution.
    let captured = applier
        .last
        .lock()
        .unwrap()
        .clone()
        .expect("applier called");
    assert_eq!(
        captured, allocation,
        "applier received the computed weights"
    );

    // 6) Assert a bandit_allocation_runs row was recorded.
    let row: (String, String, Option<serde_json::Value>) = sqlx::query_as(
        "SELECT action, outcome, new_allocation FROM bandit_allocation_runs \
         WHERE experiment_id = $1 AND iteration_id = $2 ORDER BY fired_at DESC LIMIT 1",
    )
    .bind(exp_id)
    .bind(iter_id)
    .fetch_one(&pool)
    .await
    .expect("a bandit_allocation_runs row should exist");
    assert_eq!(row.0, "reallocate");
    assert_eq!(row.1, "applied");
    let new_alloc = row.2.expect("new_allocation JSON recorded");
    assert!(
        new_alloc["treatment"].as_u64().unwrap() > new_alloc["control"].as_u64().unwrap(),
        "recorded new_allocation should favour treatment: {new_alloc}"
    );

    println!(
        "bandit reallocation: control={} bp, treatment={} bp (sum {sum})",
        weight("control"),
        weight("treatment")
    );

    // Cleanup the seeded PG rows so repeated local runs stay clean (CH is
    // append-only / deduped by unique keys per run).
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
