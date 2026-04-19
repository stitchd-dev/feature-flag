use std::sync::Arc;

use chrono::Utc;
use stitchd_core::{
    experimentation::{Experiment, ExperimentStatus},
    flag::{FlagRecord, FlagRule, FlagValueType},
    id::{EnvironmentId, ExperimentId, FlagId, FlagKey, OrganisationId, ProjectId, RuleId, VariantId},
    rule_engine::types::{ConditionExpr, Rule, RuleOutput},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, ExperimentRepository, FlagRepository, OrganisationRepository,
    ProjectRepository, RepositoryError,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgExperimentRepository, PgFlagRepository,
        PgOrganisationRepository, PgProjectRepository,
    },
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Insert org → project → environment and return (env_id, flag_rule_id).
/// The flag rule is needed as a FK for experiments.
async fn setup_experiment_deps(
    pool: sqlx::PgPool,
) -> (EnvironmentId, RuleId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let flag_repo = PgFlagRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "TestOrg".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "TestProj".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "TestEnv".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    // Create a flag, then a rule (so we have a valid flag_rule_id FK).
    let flag = FlagRecord {
        id: FlagId::new(),
        project_id: project.id,
        key: FlagKey::new("exp-flag").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    flag_repo.create(&flag).await.unwrap();

    let rules = vec![FlagRule {
        flag_id: flag.id,
        rule_index: 0,
        rule: Rule {
            id: RuleId::new(),
            condition: ConditionExpr::And(vec![]),
            output: RuleOutput::Variant(VariantId::new()),
        },
    }];
    flag_repo.upsert_rules(flag.id, &rules).await.unwrap();

    // The DB auto-generates `feature_flag_rules.id`; query it back so we can
    // use it as the `flag_rule_id` FK in experiments.
    let rule_uuid: uuid::Uuid = sqlx::query_scalar!(
        "SELECT id FROM feature_flag_rules WHERE flag_id = $1 LIMIT 1",
        flag.id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let flag_rule_id = RuleId::from_uuid(rule_uuid);

    (env.id, flag_rule_id)
}

fn make_experiment(env_id: EnvironmentId, flag_rule_id: RuleId) -> Experiment {
    Experiment {
        id: ExperimentId::new(),
        environment_id: env_id,
        flag_rule_id,
        name: "Test Experiment".into(),
        description: Some("A test".into()),
        hypothesis: None,
        metric_keys: vec!["checkout_completed".into()],
        traffic_allocation: 100.0,
        min_sample_size: None,
        scheduled_start_at: None,
        scheduled_end_at: None,
        status: ExperimentStatus::Draft,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn test_create_and_find(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.expect("create should succeed");

    let found = repo
        .find_by_id(exp.id)
        .await
        .expect("should find by id");

    assert_eq!(found.id, exp.id);
    assert_eq!(found.name, "Test Experiment");
    assert_eq!(found.status, ExperimentStatus::Draft);
    assert_eq!(found.metric_keys, vec!["checkout_completed"]);
    assert_eq!(found.version, 1);
    assert!(found.deleted_at.is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn test_list_by_environment(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    // Create two draft experiments
    let exp1 = make_experiment(env_id, rule_id);
    let mut exp2 = make_experiment(env_id, rule_id);
    exp2.id = ExperimentId::new();
    exp2.name = "Second Experiment".into();
    // We need a different flag_rule_id or different status to avoid the unique partial index
    // on (flag_rule_id) WHERE status IN ('running', 'paused'). Both are draft so it's fine.
    // However the experiments table itself has no unique constraint on flag_rule_id for drafts.
    // Actually, looking at the migration: unique index only applies WHERE status IN ('running','paused')
    // So two drafts with the same rule_id is allowed.
    repo.create(&exp1).await.unwrap();
    repo.create(&exp2).await.unwrap();

    let all = repo
        .list_by_environment(env_id, None)
        .await
        .expect("list should succeed");
    assert_eq!(all.len(), 2);

    // Filter by status
    let drafts = repo
        .list_by_environment(env_id, Some(ExperimentStatus::Draft))
        .await
        .expect("filtered list should succeed");
    assert_eq!(drafts.len(), 2);

    let running = repo
        .list_by_environment(env_id, Some(ExperimentStatus::Running))
        .await
        .expect("filtered list should succeed");
    assert!(running.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn test_update_optimistic_locking(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // First update: version 1 → 2
    let mut to_update = exp.clone();
    to_update.name = "Updated Name".into();
    let updated = repo
        .update(&to_update)
        .await
        .expect("first update should succeed");
    assert_eq!(updated.version, 2);
    assert_eq!(updated.name, "Updated Name");

    // Second update with stale version=1 must fail
    let result = repo.update(&to_update).await;
    assert!(
        matches!(result, Err(RepositoryError::VersionConflict { .. })),
        "stale version update must return VersionConflict, got: {:?}",
        result
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_soft_delete(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    repo.soft_delete(exp.id)
        .await
        .expect("soft delete should succeed");

    // Should not be findable after soft delete
    let result = repo.find_by_id(exp.id).await;
    assert!(
        matches!(result, Err(RepositoryError::NotFound { .. })),
        "deleted experiment should not be found"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_soft_delete_running_rejected(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let mut exp = make_experiment(env_id, rule_id);
    exp.status = ExperimentStatus::Running;
    repo.create(&exp).await.unwrap();

    let result = repo.soft_delete(exp.id).await;
    assert!(
        matches!(result, Err(RepositoryError::InvalidState { .. })),
        "deleting a running experiment must return InvalidState, got: {:?}",
        result
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_list_iterations_empty(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    let iterations = repo
        .list_iterations(exp.id)
        .await
        .expect("list_iterations should succeed");
    assert!(
        iterations.is_empty(),
        "new experiment should have no iterations"
    );
}
