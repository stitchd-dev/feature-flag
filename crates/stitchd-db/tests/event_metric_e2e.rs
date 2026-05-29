//! End-to-end integration test for the events + metrics pipeline.
//!
//! Wires together every layer the `events_metrics_20260519` track delivered:
//!
//! 1. **PG setup** — provisions `Organisation → Project → Environment`,
//!    registers two `event_definitions` (`checkout_started`,
//!    `checkout_completed`), creates three `metric_definitions` (two
//!    count aggregations + one ratio metric `conversion_rate =
//!    count_completed / count_started`), and creates a `Draft` experiment
//!    targeting the ratio metric.
//!
//! 2. **State transition** — flips the experiment into `Running`, which
//!    seeds an `experiment_iterations` row carrying the snapshot of
//!    `metric_ids` at compute time.
//!
//! 3. **Event ingestion** — writes events directly to ClickHouse
//!    `events` using the canonical [`EventRow`] from
//!    `stitchd-event-writer`. This is the pragmatic
//!    deviation from the original plan: skipping the SDK → gateway →
//!    analytics-service hops keeps the test focused on the
//!    "experiment reads events through the metric dispatcher" half of the
//!    pipeline, which is the actually-new piece.
//!
//! 4. **Compute** — invokes `dispatch_metric_query` for the ratio metric,
//!    rewrites placeholders to `?`, binds the values to the live
//!    ClickHouse client, executes, and parses the result rows.
//!
//! 5. **Assert** — verifies the per-variant `metric_value` (the conversion
//!    rate) is in the [0.6, 0.8] tolerance band of the true 70% rate.
//!
//! The test is marked `#[ignore = "needs running postgres + clickhouse"]`
//! per the `conductor/patterns.md` convention for infrastructure-dependent
//! tests — it does not execute under `cargo test` but does compile cleanly
//! and runs locally via `cargo test ... -- --ignored`. Live infra is
//! expected on `localhost:5432` (PG) and `localhost:8123` (CH) with the
//! standard dev credentials `stitchd:stitchd` against database `stitchd`.

use std::sync::Arc;

use chrono::Utc;
use stitchd_core::{
    event::{EventDefinition, EventValueType, MetricType},
    experimentation::{Experiment, ExperimentStatus},
    flag::{FlagRecord, FlagRule, FlagValueType},
    id::{
        EnvironmentId, EventDefinitionId, ExperimentId, FlagId, FlagKey, MetricId, OrganisationId,
        ProjectId, RuleId, VariantId,
    },
    metric::{
        AggregationConfig, AggregationOperator, GoalDirection, MetricDefinition, MetricKind,
        RatioConfig,
    },
    rule_engine::types::{ConditionExpr, Rule, RuleOutput},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, EventDefinitionRepository, ExperimentRepository, FlagRepository,
    MetricRepository, OrganisationRepository, ProjectRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgEventDefinitionRepository,
        PgExperimentRepository, PgFlagRepository, PgMetricRepository, PgOrganisationRepository,
        PgProjectRepository,
    },
};
use stitchd_event_writer::writer::EventRow;
use stitchd_stats_service::{
    dispatch::{dispatch_metric_query, rewrite_placeholders_to_clickhouse},
    queries::QueryBind,
};

// ── ClickHouse fixtures ─────────────────────────────────────────────────────

/// Make a ClickHouse client pointed at the local dev container.
///
/// Mirrors `make_ch_client` in `crates/stitchd-analytics-service/src/grpc/
/// ingestion.rs` tests so the two paths converge on a single canonical
/// dev URL/credentials triple.
fn make_ch_client() -> clickhouse::Client {
    clickhouse::Client::default()
        .with_url("http://localhost:8123")
        .with_user("stitchd")
        .with_password("stitchd")
        .with_database("stitchd")
}

/// A single result row returned by the ratio query — mirrors the SELECT
/// list emitted by `ratio::build_ratio_query`. Column order and types match
/// the emitted SQL exactly:
/// - `context_type`: `LowCardinality(String)` → deserialised as `String`
/// - `variant_key`: `String`
/// - `num_value` / `den_value` / `den_count`: `Float64` (the aggregation
///   builder wraps every aggregator in `toFloat64(...)`, including the
///   `count()` denominator)
/// - `metric_value`: `Nullable(Float64)` (the `nullIf` div-by-zero guard
///   in `ratio.rs` makes the result type nullable)
#[derive(Debug, serde::Deserialize, clickhouse::Row)]
struct RatioResultRow {
    context_type: String,
    variant_key: String,
    num_value: f64,
    den_value: f64,
    den_count: f64,
    metric_value: Option<f64>,
}

// ── PG setup helpers ────────────────────────────────────────────────────────

/// Build a [`PgPool`] from `DATABASE_URL`. Panics if unset — by design,
/// the test is `#[ignore]`d so callers must opt in.
async fn pg_pool() -> sqlx::PgPool {
    let url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL must be set (e.g. postgres://stitchd:stitchd@localhost:5432/stitchd)",
    );
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("PG connect should succeed")
}

/// Insert org → project → environment and return their IDs plus a flag
/// rule ID we can pin an experiment to.
async fn seed_tenant(pool: &sqlx::PgPool) -> (EnvironmentId, FlagId, RuleId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let flag_repo = PgFlagRepository::new(pool.clone(), audit);

    // Each run gets a fresh org name so reruns don't collide on the global
    // unique constraint on organisations.name.
    let suffix = uuid::Uuid::new_v4();
    let org = Organisation {
        id: OrganisationId::new(),
        name: format!("E2EOrg-{suffix}"),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
        is_system: false,
    };
    org_repo.create(&org).await.expect("org insert");

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: format!("Proj-{suffix}"),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.expect("project insert");

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: format!("Env-{suffix}"),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.expect("env insert");

    // Experiments require a flag_rule_id FK — provision a flag with a
    // single rule whose output is a variant assignment.
    let flag = FlagRecord {
        id: FlagId::new(),
        project_id: project.id,
        key: FlagKey::new(format!("e2e-flag-{}", suffix.simple())).unwrap(),
        name: String::new(),
        description: String::new(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        default_rule_distribution: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    flag_repo.create(&flag).await.expect("flag insert");

    let rules = vec![FlagRule {
        flag_id: flag.id,
        rule_index: 0,
        rule: Rule {
            id: RuleId::new(),
            name: None,
            condition: ConditionExpr::And(vec![]),
            output: RuleOutput::Variant(VariantId::new()),
        },
    }];
    flag_repo
        .upsert_rules(flag.id, &rules)
        .await
        .expect("flag rules upsert");

    let rule_uuid: uuid::Uuid = sqlx::query_scalar!(
        "SELECT id FROM feature_flag_rules WHERE flag_id = $1 LIMIT 1",
        flag.id.as_uuid()
    )
    .fetch_one(pool)
    .await
    .expect("flag rule fetch");

    (env.id, flag.id, RuleId::from_uuid(rule_uuid))
}

/// Insert two event definitions and return their IDs.
async fn seed_event_definitions(pool: &sqlx::PgPool, env_id: EnvironmentId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool.clone(), audit);

    let now = Utc::now();
    let started = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env_id,
        key: "checkout_started".into(),
        name: "checkout_started".into(),
        description: None,
        metric_type: MetricType::Count,
        schema: None,
        value_type: EventValueType::Bool,
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: 1,
    };
    let completed = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env_id,
        key: "checkout_completed".into(),
        name: "checkout_completed".into(),
        description: None,
        metric_type: MetricType::Count,
        schema: None,
        value_type: EventValueType::Bool,
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: 1,
    };
    repo.create(&started).await.expect("started def insert");
    repo.create(&completed).await.expect("completed def insert");
}

/// Insert the three metrics and return `(count_started_id,
/// count_completed_id, ratio_id)`.
async fn seed_metrics(
    pool: &sqlx::PgPool,
    env_id: EnvironmentId,
) -> (MetricId, MetricId, MetricId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgMetricRepository::new(pool.clone(), audit);

    let now = Utc::now();
    let count_started = MetricDefinition {
        id: MetricId::new(),
        environment_id: env_id,
        key: "count_started".into(),
        name: "Count Started".into(),
        description: None,
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: "checkout_started".into(),
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
    let count_completed = MetricDefinition {
        id: MetricId::new(),
        environment_id: env_id,
        key: "count_completed".into(),
        name: "Count Completed".into(),
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
    };
    let conversion_rate = MetricDefinition {
        id: MetricId::new(),
        environment_id: env_id,
        key: "conversion_rate".into(),
        name: "Conversion Rate".into(),
        description: Some("checkout_completed / checkout_started".into()),
        kind: MetricKind::Ratio(RatioConfig {
            numerator_metric_id: count_completed.id,
            denominator_metric_id: count_started.id,
            min_denominator: 10,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };

    repo.create(&count_started).await.expect("num metric");
    repo.create(&count_completed).await.expect("den metric");
    repo.create(&conversion_rate).await.expect("ratio metric");

    (count_started.id, count_completed.id, conversion_rate.id)
}

/// Create a Draft experiment with the ratio metric attached and
/// transition it to Running so an iteration row is materialised.
async fn seed_running_experiment(
    pool: &sqlx::PgPool,
    env_id: EnvironmentId,
    flag_id: FlagId,
    flag_rule_id: RuleId,
    ratio_id: MetricId,
) -> (ExperimentId, uuid::Uuid) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = Experiment {
        id: ExperimentId::new(),
        environment_id: env_id,
        flag_id,
        flag_key: None,
        variant_keys: vec![],
        flag_rule_id: Some(flag_rule_id),
        targets_default_rule: false,
        name: "E2E Conversion Rate".into(),
        description: Some("ratio metric end-to-end".into()),
        hypothesis: None,
        metric_ids: vec![ratio_id],
        guardrail_metric_ids: vec![],
        traffic_allocation: 100.0,
        min_sample_size: None,
        pre_period_days: 0,
        unit_context_types: vec!["user".to_string()],
        scheduled_start_at: None,
        scheduled_end_at: None,
        status: ExperimentStatus::Draft,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&exp).await.expect("experiment insert");
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("transition to running");

    // Fetch the iteration ID — `list_iterations` returns them in order;
    // the just-created one is the last (and only) entry.
    let iterations = repo.list_iterations(exp.id).await.expect("list iterations");
    let iter_id = iterations
        .last()
        .expect("running experiment must have at least one iteration")
        .id;

    (exp.id, iter_id.as_uuid())
}

// ── Event writing ───────────────────────────────────────────────────────────

/// Build a single [`EventRow`] carrying the experiment / iteration /
/// variant context tuples expected by the dispatched query.
fn make_event_row(
    env_id: EnvironmentId,
    exp_id: ExperimentId,
    iter_id: uuid::Uuid,
    variant_key: &str,
    user_key: &str,
    event_key: &str,
    now_millis: i64,
) -> EventRow {
    EventRow {
        env_id: env_id.as_uuid(),
        contexts: vec![
            ("user".into(), user_key.into()),
            ("experiment".into(), exp_id.as_uuid().to_string()),
            ("iteration".into(), iter_id.to_string()),
            ("variant".into(), variant_key.into()),
        ],
        metric_key: event_key.into(),
        // Bool events for both — match the registered value_type. The
        // aggregation query uses `count()`, so the actual payload is
        // irrelevant beyond schema-validity.
        value_bool: Some(true),
        value_int: None,
        value_double: None,
        timestamp: now_millis,
        properties: vec![],
        occurred_at: now_millis,
    }
}

/// Insert a batch of [`EventRow`]s in one CH statement.
async fn insert_events(client: &clickhouse::Client, rows: &[EventRow]) {
    let mut insert = client
        .insert::<EventRow>("events")
        .await
        .expect("insert init");
    for row in rows {
        insert.write(row).await.expect("row write");
    }
    insert.end().await.expect("insert end");
}

/// A row in the `experiment_assignments` ClickHouse table.
///
/// The post-cutover ratio query attributes events via an ITT join against this
/// table on `(env_id, context_type, context_key)`, scoped by experiment +
/// iteration and bounded by `e.occurred_at >= a.assigned_at`. In production the
/// rows are produced by `experiment_assignments_mv` from eval-log inserts; this
/// test seeds them directly — the same pragmatic deviation as the direct event
/// writes (skip the SDK → gateway → eval-log → MV hops, exercise the
/// query-side attribution).
#[derive(Debug, serde::Serialize, clickhouse::Row)]
struct AssignmentRow {
    #[serde(with = "clickhouse::serde::uuid")]
    experiment_id: uuid::Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    iteration_id: uuid::Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    env_id: uuid::Uuid,
    #[serde(with = "clickhouse::serde::uuid")]
    flag_id: uuid::Uuid,
    #[serde(with = "clickhouse::serde::uuid::option")]
    matched_rule_id: Option<uuid::Uuid>,
    context_type: String,
    context_key: String,
    variant_key: String,
    assigned_at: i64,
    #[serde(rename = "_version")]
    version: i64,
}

/// Insert assignment rows, then `OPTIMIZE … FINAL` so the `ReplacingMergeTree`
/// collapses to one row per `(experiment_id, iteration_id, context_type,
/// context_key)` before the ratio query reads it.
async fn insert_assignments(client: &clickhouse::Client, rows: &[AssignmentRow]) {
    let mut insert = client
        .insert::<AssignmentRow>("experiment_assignments")
        .await
        .expect("assignments insert init");
    for row in rows {
        insert.write(row).await.expect("assignment row write");
    }
    insert.end().await.expect("assignments insert end");
    client
        .query("OPTIMIZE TABLE experiment_assignments FINAL")
        .execute()
        .await
        .expect("optimize experiment_assignments");
}

// ── Query execution ─────────────────────────────────────────────────────────

/// Execute a `BuiltQuery` against ClickHouse, binding values in the order
/// they appear in `binds`.
async fn execute_built_query(
    client: &clickhouse::Client,
    sql: String,
    binds: Vec<QueryBind>,
) -> Vec<RatioResultRow> {
    let ch_sql = rewrite_placeholders_to_clickhouse(sql);
    let mut query = client.query(&ch_sql);
    for bind in binds {
        query = match bind {
            QueryBind::Str(s) => query.bind(s),
            QueryBind::I64(n) => query.bind(n),
            QueryBind::F64(f) => query.bind(f),
        };
    }
    query
        .fetch_all::<RatioResultRow>()
        .await
        .expect("ratio query should execute")
}

// ── The test ────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs running postgres + clickhouse"]
async fn sdk_fires_events_experiment_reads_via_ratio_metric() {
    // Sized small (100 contexts) so the test runs in well under 30 s.
    // 70 of 100 contexts complete checkout → ratio = 0.70.
    const N_CONTEXTS: usize = 100;
    const N_COMPLETED: usize = 70;
    const VARIANT_KEY: &str = "treatment";

    // ── 1. Provision PG state ─────────────────────────────────────────────
    let pool = pg_pool().await;
    let ch = make_ch_client();

    // Make sure the ClickHouse `events` table + the `properties` /
    // `occurred_at` columns exist before we try to write into it. Idempotent.
    stitchd_event_writer::migrations::run(&ch)
        .await
        .expect("CH migrations must apply");

    let (env_id, flag_id, flag_rule_id) = seed_tenant(&pool).await;
    seed_event_definitions(&pool, env_id).await;
    let (_count_started_id, _count_completed_id, ratio_id) = seed_metrics(&pool, env_id).await;
    let (exp_id, iter_uuid) =
        seed_running_experiment(&pool, env_id, flag_id, flag_rule_id, ratio_id).await;

    // ── 2. Fire events directly into ClickHouse ───────────────────────────
    //
    // Pragmatic deviation from the original plan: the SDK transport layer
    // is bypassed because spinning up a live gateway + analytics-service
    // pair in a test binary is heavyweight, and the SDK→gateway hops are
    // covered by other integration tests in this track. The schema of the
    // rows we write here mirrors what the analytics-service ingestion
    // handler would have emitted on its happy path — both construct the
    // canonical `stitchd_event_writer::writer::EventRow`.
    let now_millis = Utc::now().timestamp_millis();
    let mut rows: Vec<EventRow> = Vec::with_capacity(N_CONTEXTS + N_COMPLETED);

    for i in 0..N_CONTEXTS {
        let user_key = format!("user-{i}");
        rows.push(make_event_row(
            env_id,
            exp_id,
            iter_uuid,
            VARIANT_KEY,
            &user_key,
            "checkout_started",
            now_millis,
        ));
    }
    for i in 0..N_COMPLETED {
        let user_key = format!("user-{i}");
        rows.push(make_event_row(
            env_id,
            exp_id,
            iter_uuid,
            VARIANT_KEY,
            &user_key,
            "checkout_completed",
            now_millis,
        ));
    }
    insert_events(&ch, &rows).await;

    // ── 2b. Seed experiment_assignments (ITT attribution) ─────────────────
    //
    // The ratio query attributes events through an INNER JOIN against
    // `experiment_assignments` (on env_id + context_type + context_key,
    // scoped by experiment + iteration), keeping only events whose
    // `occurred_at >= assigned_at`. Every context that fired an event must
    // therefore have an assignment row to `treatment`, assigned strictly
    // before the events so the ITT bound keeps them.
    let assigned_at = now_millis - 60_000; // 1 minute before the events
    let assignments: Vec<AssignmentRow> = (0..N_CONTEXTS)
        .map(|i| AssignmentRow {
            experiment_id: exp_id.as_uuid(),
            iteration_id: iter_uuid,
            env_id: env_id.as_uuid(),
            flag_id: flag_id.as_uuid(),
            matched_rule_id: Some(flag_rule_id.as_uuid()),
            context_type: "user".into(),
            context_key: format!("user-{i}"),
            variant_key: VARIANT_KEY.into(),
            assigned_at,
            version: 1,
        })
        .collect();
    insert_assignments(&ch, &assignments).await;

    // ── 3. Build + execute the ratio query via the dispatcher ─────────────
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let metric_repo = PgMetricRepository::new(pool.clone(), audit);
    let ratio_metric = metric_repo
        .find_by_id(ratio_id)
        .await
        .expect("ratio metric must round-trip from PG");

    let variant_keys = [VARIANT_KEY];
    let built = dispatch_metric_query(
        &ratio_metric,
        &metric_repo,
        &exp_id.as_uuid().to_string(),
        &iter_uuid.to_string(),
        &env_id.as_uuid().to_string(),
        &variant_keys,
        chrono::Utc::now(),
    )
    .await
    .expect("dispatch_metric_query for ratio");

    let result_rows = execute_built_query(&ch, built.sql, built.binds).await;

    // ── 4. Assert the expected conversion rate ────────────────────────────
    assert!(
        !result_rows.is_empty(),
        "ratio query returned zero rows — expected at least one variant"
    );

    let treatment = result_rows
        .iter()
        .find(|r| r.variant_key == VARIANT_KEY)
        .unwrap_or_else(|| panic!("missing `{VARIANT_KEY}` row in {result_rows:?}"));

    // The metric's unit_context_types is `["user"]`, so the ratio query
    // groups/returns rows at the `user` context level.
    assert_eq!(
        treatment.context_type, "user",
        "expected user-level attribution (row={treatment:?})",
    );

    // Counts should be exactly what we wrote. The aggregator emits Float64,
    // but the values are whole counts so exact equality holds.
    let expected_num = N_COMPLETED as f64;
    let expected_den = N_CONTEXTS as f64;
    assert_eq!(
        treatment.num_value, expected_num,
        "numerator count mismatch (row={treatment:?})",
    );
    assert_eq!(
        treatment.den_value, expected_den,
        "denominator count mismatch (row={treatment:?})",
    );
    assert_eq!(
        treatment.den_count, expected_den,
        "den_count mismatch (row={treatment:?})",
    );

    let ratio = treatment
        .metric_value
        .expect("metric_value must be Some when denominator > 0");
    let expected_ratio = N_COMPLETED as f64 / N_CONTEXTS as f64;
    let lower = expected_ratio - 0.05;
    let upper = expected_ratio + 0.05;
    assert!(
        (lower..=upper).contains(&ratio),
        "expected conversion rate ≈ {expected_ratio} (within ±0.05); got {ratio} (row={treatment:?})",
    );
}
