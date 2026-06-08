//! End-to-end live-ClickHouse + live-Postgres integration test for the FULL
//! static bandit lifecycle in one flow.
//!
//! This exercises the complete autonomous static-bandit pipeline against real
//! infrastructure, in the order the production scheduler runs it each tick:
//!
//!   1. Create a bandit experiment (Static propagation, `AutoRollout` policy)
//!      and seed the PG FK chain + ClickHouse assignments/events such that one
//!      arm (`treatment`, ~30% conversion) is CLEARLY better than the other
//!      (`control`, ~10%).
//!   2. Run [`run_bandit_reallocation`] repeatedly and assert that the
//!      allocation SHIFTS toward the winner across ticks (recorded as
//!      `bandit_allocation_runs action='reallocate' outcome='applied'` rows).
//!   3. Run [`run_bandit_lifecycle`] (AutoRollout) and assert that:
//!      (a) convergence is detected and persisted
//!          (`experiments.bandit_converged_variant` / `_prob` set to the
//!          winner),
//!      (b) the winner is COMMITTED (100% single-bucket distribution applied via
//!          the privileged applier) and a `rollout` history row is recorded,
//!      (c) the experiment is STOPPED (status → stopped), which by the
//!          whole-flag lock model RELEASES the lock (no running/paused
//!          experiment remains bound to the flag),
//!      (d) the bound rule ends at the winner (the applier was handed the
//!          single-bucket winner distribution).
//!   4. Run the lifecycle once MORE and assert idempotency (already
//!      committed + stopped → `StopOnly`, still rolled out, no duplicate
//!      commit).
//!
//! The [`LifecycleTransitioner`] is implemented HERE with REAL Postgres writes
//! (advisory `UPDATE experiments SET bandit_converged_variant…` exactly like the
//! production `GrpcLifecycleTransitioner`, plus a `status='concluded'` stop write
//! that mirrors what the experimentation-service `TransitionExperiment(Concluded)`
//! RPC does to the row) — so we exercise the real lifecycle ORCHESTRATION and the
//! real DB side effects without needing the experimentation-service process up.
//! The allocation applier is captured (it stands in for the flag-service
//! privileged write path, asserted unit-side elsewhere).
//!
//! Tagged `#[ignore]` so the default `cargo test` run needs no infrastructure.
//! Run explicitly with:
//!
//! ```sh
//! DATABASE_URL=… cargo test -p stitchd-stats-service --test bandit_e2e_lifecycle -- --ignored
//! ```

#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use clickhouse::Client;
use uuid::Uuid;

use stitchd_core::experimentation::bandit::{
    BanditAlgorithm, BanditConfig, ConvergedWinner, ExperimentMode, LifecyclePolicy,
    PropagationMode, RewardObjective,
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
use stitchd_stats_service::lifecycle::{
    LifecycleOutcome, LifecycleTransitioner, run_bandit_lifecycle,
};
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

/// A fake applier that always returns APPLIED, capturing the most recent
/// distribution it was handed so the test can assert the bound rule ends at the
/// winner (single-bucket 100%).
#[derive(Default)]
struct CapturingApplier {
    last: Mutex<Option<RolloutDistribution>>,
    applies: Mutex<usize>,
}

#[async_trait::async_trait]
impl AllocationApplier for CapturingApplier {
    async fn apply(
        &self,
        _experiment_id: Uuid,
        allocation: &RolloutDistribution,
    ) -> Result<ApplyResult, anyhow::Error> {
        *self.last.lock().unwrap() = Some(allocation.clone());
        *self.applies.lock().unwrap() += 1;
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

/// A real-Postgres [`LifecycleTransitioner`]. `record_convergence` issues the
/// exact advisory `UPDATE` the production `GrpcLifecycleTransitioner` does; the
/// "stop" write sets `status='stopped'`, mirroring the row mutation the
/// experimentation-service `TransitionExperiment(Concluded)` RPC performs — which
/// releases the whole-flag lock (no running/paused experiment remains bound).
struct PgLifecycleTransitioner {
    pool: sqlx::PgPool,
    stops: Mutex<usize>,
}

impl PgLifecycleTransitioner {
    fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            stops: Mutex::new(0),
        }
    }
}

#[async_trait::async_trait]
impl LifecycleTransitioner for PgLifecycleTransitioner {
    async fn stop_experiment(
        &self,
        experiment_id: Uuid,
        _environment_id: Uuid,
    ) -> Result<(), anyhow::Error> {
        *self.stops.lock().unwrap() += 1;
        // Proto CONCLUDED maps to the core `Stopped` status persisted on the row
        // (see `GrpcLifecycleTransitioner::stop_experiment`), which is the
        // permanent stop that releases the whole-flag lock.
        sqlx::query("UPDATE experiments SET status = 'stopped' WHERE id = $1")
            .bind(experiment_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn record_convergence(
        &self,
        experiment_id: Uuid,
        winner: &ConvergedWinner,
    ) -> Result<(), anyhow::Error> {
        sqlx::query(
            "UPDATE experiments \
             SET bandit_converged_variant = $2, bandit_converged_prob = $3 \
             WHERE id = $1",
        )
        .bind(experiment_id)
        .bind(&winner.variant_key)
        .bind(winner.prob)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Seed the minimal PG FK chain and return `(env_id, flag_id, exp_id, iter_id)`.
async fn seed_pg_chain(pool: &sqlx::PgPool) -> (Uuid, Uuid, Uuid, Uuid) {
    let org_id = Uuid::new_v4();
    let project_id = Uuid::new_v4();
    let env_id = Uuid::new_v4();
    let flag_id = Uuid::new_v4();
    let exp_id = Uuid::new_v4();
    let iter_id = Uuid::new_v4();

    sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind("bandit-e2e-org")
        .execute(pool)
        .await
        .expect("seed organisation");
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(org_id)
        .bind("bandit-e2e-project")
        .execute(pool)
        .await
        .expect("seed project");
    sqlx::query("INSERT INTO environments (id, project_id, name) VALUES ($1, $2, $3)")
        .bind(env_id)
        .bind(project_id)
        .bind("bandit-e2e-env")
        .execute(pool)
        .await
        .expect("seed environment");
    sqlx::query(
        "INSERT INTO feature_flags (id, project_id, key, name, value_type, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(flag_id)
    .bind(project_id)
    .bind(format!("bandit_e2e_flag_{}", &exp_id.to_string()[..8]))
    .bind("bandit e2e flag")
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
    .bind("bandit e2e experiment")
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

    (env_id, flag_id, exp_id, iter_id)
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
async fn bandit_full_lifecycle_reallocate_converge_rollout_releases_lock() {
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

    // ── 1) Seed PG FK chain + ClickHouse data (treatment 30% >> control 10%). ──
    let (env_id, flag_id, exp_id, iter_id) = seed_pg_chain(&pool).await;
    let metric_id = Uuid::new_v4();
    let metric_key = format!("conv_{}", &exp_id.to_string()[..8]);
    let iter_end =
        seed_count_assignments_events(&ch, env_id, exp_id, iter_id, &metric_key, 1000, 100, 300)
            .await;

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

    // Static propagation + AutoRollout policy (autonomous commit + stop).
    let config = BanditConfig {
        algorithm: BanditAlgorithm::ThompsonSampling,
        propagation_mode: PropagationMode::Static,
        min_exploration_bp: 500,
        objective: RewardObjective::Scalar { metric_id },
        lifecycle_policy: LifecyclePolicy::AutoRollout,
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
        bandit_campaign_id: None,
    };

    let reader = ClickHouseCellReader::new(Arc::new(ch.clone()));
    let applier = CapturingApplier::default();
    let recorder = PgRunRecorder::new(pool.clone());
    let transitioner = PgLifecycleTransitioner::new(pool.clone());

    // ── 2) Run the reallocation pass across several ticks; assert it favours
    // the winner and records `reallocate`/`applied` history rows. ──────────────
    let mut last_alloc: Option<RolloutDistribution> = None;
    for day in 1..=3 {
        let tick = iter_end + Duration::days(day);
        let outcome =
            run_bandit_reallocation(&reader, &applier, &recorder, &exp, &metrics, iter_end, tick)
                .await
                .expect("reallocation must not error against live infra");
        let allocation = match outcome {
            BanditRunOutcome::Applied { allocation, .. } => allocation,
            other => panic!("expected Applied on tick {day}, got {other:?}"),
        };
        let weight = |a: &RolloutDistribution, k: &str| {
            a.allocations
                .iter()
                .find(|x| x.variant_key == k)
                .map(|x| x.percentage_bp)
                .unwrap_or(0)
        };
        let sum: u32 = allocation.allocations.iter().map(|a| a.percentage_bp).sum();
        assert_eq!(sum, 10_000, "tick {day}: allocation must sum to 10000 bp");
        assert!(
            weight(&allocation, "treatment") > weight(&allocation, "control"),
            "tick {day}: treatment (30%) should outweigh control (10%): {allocation:?}"
        );
        last_alloc = Some(allocation);
    }

    // The most recent reallocation favours the winner but is NOT a 100% commit
    // (a floor of 500 bp keeps control exploring) — this is the `current
    // allocation` the lifecycle sees as "not yet committed".
    let current_alloc = last_alloc.expect("at least one reallocation tick");
    assert!(
        current_alloc
            .allocations
            .iter()
            .any(|a| a.variant_key == "control" && a.percentage_bp > 0),
        "pre-commit allocation should still explore control: {current_alloc:?}"
    );

    // Assert at least one applied `reallocate` row landed in PG.
    let reallocate_rows: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM bandit_allocation_runs \
         WHERE experiment_id = $1 AND action = 'reallocate' AND outcome = 'applied'",
    )
    .bind(exp_id)
    .fetch_one(&pool)
    .await
    .expect("count reallocate rows");
    assert!(
        reallocate_rows.0 >= 1,
        "expected at least one applied reallocate row, got {}",
        reallocate_rows.0
    );

    // ── 3) Run the AutoRollout lifecycle pass: converge → commit → stop. ───────
    let tick = iter_end + Duration::days(10);
    let outcome = run_bandit_lifecycle(
        &reader,
        &applier,
        &transitioner,
        &recorder,
        &exp,
        &metrics,
        Some(&current_alloc),
        iter_end,
        tick,
    )
    .await
    .expect("lifecycle must not error against live infra");

    let winner = match outcome {
        LifecycleOutcome::RolledOut(w) => w,
        other => panic!("expected RolledOut, got {other:?}"),
    };
    assert_eq!(
        winner.variant_key, "treatment",
        "treatment should be the converged winner"
    );
    assert!(
        winner.prob >= 0.95,
        "winner prob {} should clear the 0.95 threshold",
        winner.prob
    );

    // (a) Convergence persisted onto the experiment row.
    let conv: (Option<String>, Option<f64>) = sqlx::query_as(
        "SELECT bandit_converged_variant, bandit_converged_prob FROM experiments WHERE id = $1",
    )
    .bind(exp_id)
    .fetch_one(&pool)
    .await
    .expect("read convergence state");
    assert_eq!(
        conv.0.as_deref(),
        Some("treatment"),
        "bandit_converged_variant must be set to the winner"
    );
    assert!(
        conv.1.is_some_and(|p| p >= 0.95),
        "bandit_converged_prob must be set above threshold: {:?}",
        conv.1
    );

    // (b) Rollout committed the winner: the applier was handed the single-bucket
    // 100% winner distribution (the bound rule ends at the winner).
    let captured = applier
        .last
        .lock()
        .unwrap()
        .clone()
        .expect("applier called during rollout commit");
    assert_eq!(
        captured.allocations.len(),
        1,
        "commit must be a single-bucket distribution: {captured:?}"
    );
    assert_eq!(captured.allocations[0].variant_key, "treatment");
    assert_eq!(captured.allocations[0].percentage_bp, 10_000);

    // A `rollout`/`applied` history row was recorded.
    let rollout_rows: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM bandit_allocation_runs \
         WHERE experiment_id = $1 AND action = 'rollout' AND outcome = 'applied'",
    )
    .bind(exp_id)
    .fetch_one(&pool)
    .await
    .expect("count rollout rows");
    assert!(
        rollout_rows.0 >= 1,
        "expected an applied rollout row, got {}",
        rollout_rows.0
    );

    // (c) The experiment is stopped → the lock is released (no running/paused
    // experiment bound to the flag — the exact derivation `is_flag_locked` uses).
    let status: (String,) = sqlx::query_as("SELECT status FROM experiments WHERE id = $1")
        .bind(exp_id)
        .fetch_one(&pool)
        .await
        .expect("read experiment status");
    assert_eq!(status.0, "stopped", "experiment should be stopped");

    let lock_holders: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM experiments \
         WHERE flag_id = $1 AND status IN ('running','paused') AND deleted_at IS NULL",
    )
    .bind(flag_id)
    .fetch_one(&pool)
    .await
    .expect("count lock holders");
    assert_eq!(
        lock_holders.0, 0,
        "after rollout, no running/paused experiment should hold the flag lock"
    );

    // ── 4) Idempotency: a second lifecycle pass with the committed allocation is
    // StopOnly — still rolled out, no second commit-apply. ─────────────────────
    let applies_before = *applier.applies.lock().unwrap();
    let outcome2 = run_bandit_lifecycle(
        &reader,
        &applier,
        &transitioner,
        &recorder,
        &exp,
        &metrics,
        Some(&captured), // committed single-bucket distribution
        iter_end,
        iter_end + Duration::days(11),
    )
    .await
    .expect("idempotent lifecycle pass must not error");
    assert!(
        matches!(outcome2, LifecycleOutcome::RolledOut(_)),
        "idempotent second pass should still report RolledOut, got {outcome2:?}"
    );
    assert_eq!(
        *applier.applies.lock().unwrap(),
        applies_before,
        "StopOnly must not re-apply a commit"
    );

    println!(
        "bandit e2e lifecycle: winner={} (p={:.4}), reallocate_rows={}, rollout_rows={}, status={}",
        winner.variant_key, winner.prob, reallocate_rows.0, rollout_rows.0, status.0
    );

    // ── Cleanup seeded PG rows so repeated local runs stay clean. ──────────────
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
