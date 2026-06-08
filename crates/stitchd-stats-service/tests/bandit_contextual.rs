//! Live-ClickHouse + live-Postgres integration test for the CONTEXTUAL bandit
//! real-time refresh.
//!
//! Self-seeds the minimal PG FK chain (organisation, project, environment,
//! flag, experiment, iteration) so a `bandit_allocation_runs` row can be
//! inserted under its foreign keys, plus `experiment_assignments` + `events` in
//! ClickHouse for a 2-variant numeric-reward experiment where the per-unit
//! reward DEPENDS ON a context feature carried in the event `properties`:
//!
//! * variant `high_lover` earns reward = `score`  (prefers HIGH score), and
//! * variant `low_lover`  earns reward = `10 - score` (prefers LOW score).
//!
//! It runs `run_bandit_reallocation` against the real `ClickHouseCellReader`
//! (realtime propagation + Contextual algorithm) with a fake applier that
//! captures the refreshed `RealtimeBanditModel`, then maps the captured proto
//! contextual model back to the domain and asserts that sampling from it assigns
//! `high_lover` for a HIGH feature value and `low_lover` for a LOW one — i.e. the
//! fitted model picks the better variant for the appropriate feature value.
//!
//! Tagged `#[ignore]` so the default `cargo test` run needs no infrastructure.
//! Run explicitly with:
//!
//! ```sh
//! DATABASE_URL=… cargo test -p stitchd-stats-service --test bandit_contextual -- --ignored
//! ```

#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use clickhouse::Client;
use uuid::Uuid;

use stitchd_core::experimentation::bandit::contextual::{
    ContextualModel, FeatureEncoding, FeatureSpec, VariantCoefficients, sample_contextual_variant,
};
use stitchd_core::experimentation::bandit::{
    BanditAlgorithm, BanditConfig, ContextualConfig, ExperimentMode, LifecyclePolicy,
    PropagationMode, RewardObjective,
};
use stitchd_core::id::{EnvironmentId, MetricId};
use stitchd_core::metric::{
    AggregationConfig, AggregationOperator, GoalDirection, MetricDefinition, MetricKind,
};
use stitchd_core::rollout::RolloutDistribution;
use stitchd_core::rule_engine::types::BanditGoal;
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

/// A fake applier that always returns APPLIED, capturing the realtime model it
/// was handed so the test can assert the FITTED contextual model.
#[derive(Default)]
struct CapturingApplier {
    last_model: Mutex<Option<stitchd_proto::flags::v1::RealtimeBanditModel>>,
}

#[async_trait::async_trait]
impl AllocationApplier for CapturingApplier {
    async fn apply(
        &self,
        _experiment_id: Uuid,
        _allocation: &RolloutDistribution,
    ) -> Result<ApplyResult, anyhow::Error> {
        Ok(ApplyResult::Applied {
            resolved_target: "default_rule".to_string(),
            new_version: 2,
        })
    }

    async fn apply_realtime(
        &self,
        _experiment_id: Uuid,
        _allocation: &RolloutDistribution,
        model: stitchd_proto::flags::v1::RealtimeBanditModel,
    ) -> Result<ApplyResult, anyhow::Error> {
        *self.last_model.lock().unwrap() = Some(model);
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
        .bind("ctx-bandit-test-org")
        .execute(pool)
        .await
        .expect("seed organisation");
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(org_id)
        .bind("ctx-bandit-test-project")
        .execute(pool)
        .await
        .expect("seed project");
    sqlx::query("INSERT INTO environments (id, project_id, name) VALUES ($1, $2, $3)")
        .bind(env_id)
        .bind(project_id)
        .bind("ctx-bandit-test-env")
        .execute(pool)
        .await
        .expect("seed environment");
    sqlx::query(
        "INSERT INTO feature_flags (id, project_id, key, name, value_type, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(flag_id)
    .bind(project_id)
    .bind(format!("ctx_bandit_flag_{}", &exp_id.to_string()[..8]))
    .bind("ctx bandit flag")
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
    .bind("ctx bandit experiment")
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

/// Seed assignments + reward events whose value depends on a `score` feature in
/// the event properties: `high_lover` earns `score`, `low_lover` earns
/// `10 - score`. `n_per_variant` units per variant, scores cycling 0..10.
async fn seed_contextual_assignments_events(
    ch: &Client,
    env_id: Uuid,
    exp_id: Uuid,
    iter_id: Uuid,
    metric_key: &str,
    n_per_variant: usize,
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
        assignments.push(mk_assignment("high_lover", format!("h_{i}")));
        assignments.push(mk_assignment("low_lover", format!("l_{i}")));
    }
    insert_assignments(ch, &assignments).await;

    let mk_event = |key: String, score: f64, reward: f64| EventRow {
        env_id,
        contexts: vec![("user".into(), key)],
        metric_key: metric_key.to_string(),
        value_bool: None,
        value_int: None,
        value_double: Some(reward),
        timestamp: event_at,
        ingested_at: event_at,
        // The contextual feature rides in the event properties map.
        properties: vec![
            ("score".into(), score.to_string()),
            ("reward".into(), reward.to_string()),
        ],
        occurred_at: event_at,
    };
    let mut events = Vec::with_capacity(2 * n_per_variant);
    for i in 0..n_per_variant {
        let score = (i % 11) as f64; // 0..10
        events.push(mk_event(format!("h_{i}"), score, score)); // high_lover: reward = score
        events.push(mk_event(format!("l_{i}"), score, 10.0 - score)); // low_lover: 10 - score
    }
    insert_events(ch, &events).await;

    iter_end
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running clickhouse + postgres"]
async fn contextual_bandit_fit_assigns_better_variant_per_feature_value() {
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

    // 2) Seed CH: 200 units/variant, reward depends on the `score` feature.
    let metric_id = Uuid::new_v4();
    let metric_key = format!("ctx_reward_{}", &exp_id.to_string()[..8]);
    let iter_end =
        seed_contextual_assignments_events(&ch, env_id, exp_id, iter_id, &metric_key, 200).await;

    // 3) Build the running CONTEXTUAL bandit experiment (realtime propagation).
    let now = Utc::now();
    let metric = MetricDefinition {
        id: MetricId::from_uuid(metric_id),
        environment_id: EnvironmentId::from_uuid(env_id),
        key: metric_key.clone(),
        name: "ctx reward".into(),
        description: None,
        // Sum the per-event `reward` value over the post-period (one event/unit).
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: metric_key.clone(),
            aggregator: AggregationOperator::Sum,
            on_field: Some("reward".into()),
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
        algorithm: BanditAlgorithm::Contextual(ContextualConfig {
            features: vec!["score".to_string()],
        }),
        propagation_mode: PropagationMode::Realtime,
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
        variant_keys: vec!["high_lover".into(), "low_lover".into()],
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
            .expect("contextual bandit refresh should not error against live infra");

    assert!(
        matches!(outcome, BanditRunOutcome::Applied { .. }),
        "expected Applied, got {outcome:?}"
    );

    // 5) Map the captured proto contextual model back to the domain and sample.
    let proto_model = applier
        .last_model
        .lock()
        .unwrap()
        .clone()
        .expect("apply_realtime called with a refreshed model");
    let proto_ctx = proto_model
        .contextual
        .expect("the fitted model must carry a contextual representation");
    let domain = ContextualModel {
        features: proto_ctx
            .features
            .iter()
            .map(|f| FeatureSpec {
                context_type: f.context_type.clone(),
                parameter: f.parameter.clone(),
                encoding: FeatureEncoding::Numeric,
            })
            .collect(),
        variants: proto_ctx
            .variants
            .iter()
            .map(|v| VariantCoefficients {
                variant_key: v.variant_key.clone(),
                coeffs: v.coeffs.clone(),
                // Exploit the fitted means for a stable assertion.
                a_inv: None,
            })
            .collect(),
    };

    // For a HIGH score (10) the high_lover arm should dominate; for a LOW score
    // (0) the low_lover arm should dominate — the fit recovered the dependence.
    let mut high_wins = 0;
    let mut low_wins = 0;
    let n = 500u64;
    for seed in 0..n {
        if sample_contextual_variant(&domain, &[1.0, 10.0], BanditGoal::Increase, seed).as_deref()
            == Some("high_lover")
        {
            high_wins += 1;
        }
        if sample_contextual_variant(&domain, &[1.0, 0.0], BanditGoal::Increase, seed).as_deref()
            == Some("low_lover")
        {
            low_wins += 1;
        }
    }
    assert!(
        high_wins as f64 / n as f64 > 0.85,
        "high_lover should win for high score: {high_wins}/{n}"
    );
    assert!(
        low_wins as f64 / n as f64 > 0.85,
        "low_lover should win for low score: {low_wins}/{n}"
    );

    // 6) A bandit_allocation_runs row was recorded.
    let row: (String, String) = sqlx::query_as(
        "SELECT action, outcome FROM bandit_allocation_runs \
         WHERE experiment_id = $1 AND iteration_id = $2 ORDER BY fired_at DESC LIMIT 1",
    )
    .bind(exp_id)
    .bind(iter_id)
    .fetch_one(&pool)
    .await
    .expect("a bandit_allocation_runs row should exist");
    assert_eq!(row.0, "reallocate");
    assert_eq!(row.1, "applied");

    println!(
        "contextual fit: high_lover wins {high_wins}/{n} (high), low_lover wins {low_wins}/{n} (low)"
    );

    // Cleanup the seeded PG rows so repeated local runs stay clean.
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
