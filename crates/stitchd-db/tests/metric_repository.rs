//! Integration tests for `PgMetricRepository`.
//!
//! Uses `#[sqlx::test(migrations = "./migrations")]` for isolated DB
//! per test — each gets a fresh temp database with all migrations
//! applied. The seed_env helper provisions the parent org → project →
//! environment chain that every metric needs.

use std::sync::Arc;

use stitchd_core::{
    id::{EnvironmentId, MetricId, OrganisationId, ProjectId},
    metric::{
        AggregationConfig, AggregationOperator, FunnelConfig, FunnelStep, GoalDirection,
        MetricDefinition, MetricKind, RatioConfig,
    },
    tenant::{Environment, Organisation, Project},
};

use stitchd_db::{
    EnvironmentRepository, MetricRepository, OrganisationRepository, ProjectRepository,
    RepositoryError,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgMetricRepository, PgOrganisationRepository,
        PgProjectRepository,
    },
};

// ── Setup ────────────────────────────────────────────────────────────────────

async fn seed_env(pool: &sqlx::PgPool) -> (PgMetricRepository, EnvironmentId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let metric_repo = PgMetricRepository::new(pool.clone(), audit);

    let org = Organisation {
        id: OrganisationId::new(),
        name: "Org".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
        is_system: false,
    };
    org_repo.create(&org).await.unwrap();
    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "Proj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();
    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "Env".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();
    (metric_repo, env.id)
}

fn make_aggregation(env_id: EnvironmentId, key: &str) -> MetricDefinition {
    MetricDefinition {
        id: MetricId::new(),
        environment_id: env_id,
        key: key.into(),
        name: format!("Metric {key}"),
        description: Some(format!("Test metric `{key}`")),
        kind: MetricKind::Aggregation(AggregationConfig {
            event_key: "checkout_completed".into(),
            aggregator: AggregationOperator::Count,
            on_field: None,
            where_clause: None,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "./migrations")]
async fn create_and_find_by_id(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m = make_aggregation(env, "checkout_completion_count");
    repo.create(&m).await.unwrap();
    let fetched = repo.find_by_id(m.id).await.unwrap();
    assert_eq!(fetched, m);
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_id_returns_not_found_for_missing(pool: sqlx::PgPool) {
    let (repo, _env) = seed_env(&pool).await;
    let err = repo.find_by_id(MetricId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_key_returns_live_metric(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m = make_aggregation(env, "checkout_rate");
    repo.create(&m).await.unwrap();
    let fetched = repo.find_by_key("checkout_rate", env).await.unwrap();
    assert_eq!(fetched.id, m.id);
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_key_returns_not_found_for_missing(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let err = repo.find_by_key("nope", env).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn duplicate_key_in_env_violates_unique(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m1 = make_aggregation(env, "dup");
    let mut m2 = make_aggregation(env, "dup");
    m2.id = MetricId::new();
    repo.create(&m1).await.unwrap();
    let err = repo.create(&m2).await.unwrap_err();
    assert!(matches!(err, RepositoryError::UniqueViolation { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn soft_delete_allows_recreating_same_key(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m1 = make_aggregation(env, "reusable_key");
    repo.create(&m1).await.unwrap();
    repo.soft_delete(m1.id).await.unwrap();

    // After soft-delete, the partial UNIQUE index should allow a new
    // metric with the same key.
    let mut m2 = make_aggregation(env, "reusable_key");
    m2.id = MetricId::new();
    repo.create(&m2).await.unwrap();

    // find_by_key returns the live one, not the deleted one.
    let fetched = repo.find_by_key("reusable_key", env).await.unwrap();
    assert_eq!(fetched.id, m2.id);
}

#[sqlx::test(migrations = "./migrations")]
async fn soft_delete_missing_returns_not_found(pool: sqlx::PgPool) {
    let (repo, _env) = seed_env(&pool).await;
    let err = repo.soft_delete(MetricId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_returns_only_live(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m1 = make_aggregation(env, "alpha");
    let m2 = make_aggregation(env, "beta");
    let m3 = make_aggregation(env, "gamma");
    repo.create(&m1).await.unwrap();
    repo.create(&m2).await.unwrap();
    repo.create(&m3).await.unwrap();
    repo.soft_delete(m2.id).await.unwrap();

    let listed = repo.list_by_environment(env).await.unwrap();
    let keys: Vec<_> = listed.iter().map(|m| m.key.as_str()).collect();
    assert!(keys.contains(&"alpha"));
    assert!(keys.contains(&"gamma"));
    assert!(!keys.contains(&"beta"));
    assert_eq!(listed.len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_paginated_returns_total(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    for i in 0..5 {
        repo.create(&make_aggregation(env, &format!("m{i}"))).await.unwrap();
    }
    let (page, total) = repo.list_by_environment_paginated(env, 0, 3).await.unwrap();
    assert_eq!(page.len(), 3);
    assert_eq!(total, 5);

    let (page2, total2) = repo.list_by_environment_paginated(env, 3, 3).await.unwrap();
    assert_eq!(page2.len(), 2);
    assert_eq!(total2, 5);
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_paginated_empty(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let (page, total) = repo.list_by_environment_paginated(env, 0, 50).await.unwrap();
    assert!(page.is_empty());
    assert_eq!(total, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn update_increments_version_and_persists(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let mut m = make_aggregation(env, "mutable");
    repo.create(&m).await.unwrap();

    m.name = "Updated Name".into();
    m.description = Some("Updated".into());
    m.goal_direction = GoalDirection::Decrease;
    let updated = repo.update(&m).await.unwrap();
    assert_eq!(updated.name, "Updated Name");
    assert_eq!(updated.description.as_deref(), Some("Updated"));
    assert_eq!(updated.goal_direction, GoalDirection::Decrease);
    assert_eq!(updated.version, m.version + 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn update_stale_version_returns_conflict(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let mut m = make_aggregation(env, "raced");
    repo.create(&m).await.unwrap();
    // First update bumps version to 2.
    let _ = repo.update(&m).await.unwrap();
    // Caller still holds version 1 → conflict.
    m.name = "Stale".into();
    let err = repo.update(&m).await.unwrap_err();
    match err {
        RepositoryError::VersionConflict { expected, actual } => {
            assert_eq!(expected, 1);
            assert_eq!(actual, 2);
        }
        other => panic!("expected VersionConflict, got {other:?}"),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn update_missing_returns_not_found(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m = make_aggregation(env, "absent");
    let err = repo.update(&m).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn ratio_metric_round_trips(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let num = make_aggregation(env, "num");
    let den = make_aggregation(env, "den");
    repo.create(&num).await.unwrap();
    repo.create(&den).await.unwrap();

    let ratio = MetricDefinition {
        id: MetricId::new(),
        environment_id: env,
        key: "ratio_metric".into(),
        name: "Conversion Rate".into(),
        description: None,
        kind: MetricKind::Ratio(RatioConfig {
            numerator_metric_id: num.id,
            denominator_metric_id: den.id,
            min_denominator: 100,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
    };
    repo.create(&ratio).await.unwrap();
    let fetched = repo.find_by_id(ratio.id).await.unwrap();
    assert_eq!(fetched, ratio);
}

#[sqlx::test(migrations = "./migrations")]
async fn funnel_metric_round_trips_with_steps(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let funnel = MetricDefinition {
        id: MetricId::new(),
        environment_id: env,
        key: "checkout_funnel".into(),
        name: "Checkout Funnel".into(),
        description: Some("3-step purchase funnel".into()),
        kind: MetricKind::Funnel(FunnelConfig {
            steps: vec![
                FunnelStep { event_key: "view_product".into(), where_clause: None },
                FunnelStep { event_key: "add_to_cart".into(), where_clause: None },
                FunnelStep { event_key: "checkout_completed".into(), where_clause: None },
            ],
            window_seconds: 86_400,
            count_repeats: false,
        }),
        goal_direction: GoalDirection::Increase,
        version: 1,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
    };
    repo.create(&funnel).await.unwrap();
    let fetched = repo.find_by_id(funnel.id).await.unwrap();
    assert_eq!(fetched, funnel);
    if let MetricKind::Funnel(c) = &fetched.kind {
        assert_eq!(c.steps.len(), 3);
        assert_eq!(c.window_seconds, 86_400);
    } else {
        panic!("expected Funnel variant");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn find_batch_by_ids_returns_only_live(pool: sqlx::PgPool) {
    let (repo, env) = seed_env(&pool).await;
    let m1 = make_aggregation(env, "b1");
    let m2 = make_aggregation(env, "b2");
    let m3 = make_aggregation(env, "b3");
    repo.create(&m1).await.unwrap();
    repo.create(&m2).await.unwrap();
    repo.create(&m3).await.unwrap();
    repo.soft_delete(m2.id).await.unwrap();

    let fetched = repo
        .find_batch_by_ids(&[m1.id, m2.id, m3.id, MetricId::new()])
        .await
        .unwrap();
    let ids: Vec<_> = fetched.iter().map(|m| m.id).collect();
    assert!(ids.contains(&m1.id));
    assert!(ids.contains(&m3.id));
    assert!(!ids.contains(&m2.id));
    assert_eq!(fetched.len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn find_batch_by_ids_empty_input_returns_empty(pool: sqlx::PgPool) {
    let (repo, _env) = seed_env(&pool).await;
    let fetched = repo.find_batch_by_ids(&[]).await.unwrap();
    assert!(fetched.is_empty());
}
