use std::sync::Arc;

use chrono::Utc;
use stitchd_core::{
    experimentation::{Experiment, ExperimentStatus},
    flag::{FlagRecord, FlagRule, FlagValueType},
    id::{
        EnvironmentId, ExperimentId, FlagId, FlagKey, OrganisationId, ProjectId, RuleId, VariantId,
    },
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
async fn setup_experiment_deps(pool: sqlx::PgPool) -> (EnvironmentId, RuleId) {
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

    let found = repo.find_by_id(exp.id).await.expect("should find by id");

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

// ---------------------------------------------------------------------------
// apply_transition tests
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn test_transition_draft_to_running_creates_iteration(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // Transition draft → running
    let updated = repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("draft→running should succeed");

    assert_eq!(updated.status, ExperimentStatus::Running);
    assert_eq!(updated.version, 2);

    // An iteration must have been created
    let iterations = repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(iterations.len(), 1);
    assert_eq!(iterations[0].iteration_number, 1);
    assert!(
        iterations[0].ended_at.is_none(),
        "iteration should still be active"
    );
    assert_eq!(iterations[0].metric_keys, exp.metric_keys);

    // Flag rule must be frozen
    let frozen: bool = sqlx::query_scalar!(
        "SELECT frozen FROM feature_flag_rules WHERE id = $1",
        rule_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        frozen,
        "flag rule must be frozen when experiment is running"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_transition_running_to_paused_ends_iteration(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // draft → running
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();

    // running → paused
    let updated = repo
        .apply_transition(exp.id, ExperimentStatus::Paused, None)
        .await
        .expect("running→paused should succeed");

    assert_eq!(updated.status, ExperimentStatus::Paused);

    // Iteration must have ended_at set
    let iterations = repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(iterations.len(), 1);
    assert!(
        iterations[0].ended_at.is_some(),
        "iteration must be ended when paused"
    );

    // Flag rule must be unfrozen
    let frozen: bool = sqlx::query_scalar!(
        "SELECT frozen FROM feature_flag_rules WHERE id = $1",
        rule_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !frozen,
        "flag rule must be unfrozen when experiment is paused"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_transition_paused_to_running_creates_next_iteration(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // draft → running → paused
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();
    repo.apply_transition(exp.id, ExperimentStatus::Paused, None)
        .await
        .unwrap();

    // paused → running (second run)
    let updated = repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("paused→running should succeed");

    assert_eq!(updated.status, ExperimentStatus::Running);

    // Now there should be 2 iterations, second one active
    let iterations = repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(iterations.len(), 2, "should have 2 iterations total");
    assert_eq!(iterations[0].iteration_number, 1);
    assert!(
        iterations[0].ended_at.is_some(),
        "first iteration must be ended"
    );
    assert_eq!(iterations[1].iteration_number, 2);
    assert!(
        iterations[1].ended_at.is_none(),
        "second iteration must be active"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_transition_running_to_stopped_unfreezes_rule(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // draft → running → stopped
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();
    let updated = repo
        .apply_transition(exp.id, ExperimentStatus::Stopped, None)
        .await
        .expect("running→stopped should succeed");

    assert_eq!(updated.status, ExperimentStatus::Stopped);

    // Iteration must be ended
    let iterations = repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(iterations.len(), 1);
    assert!(
        iterations[0].ended_at.is_some(),
        "iteration must be ended when stopped"
    );

    // Flag rule must be unfrozen
    let frozen: bool = sqlx::query_scalar!(
        "SELECT frozen FROM feature_flag_rules WHERE id = $1",
        rule_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !frozen,
        "flag rule must be unfrozen when experiment is stopped"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_uniqueness_guard_rejects_second_active_experiment(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    // Create first experiment and start it
    let exp1 = make_experiment(env_id, rule_id);
    repo.create(&exp1).await.unwrap();
    repo.apply_transition(exp1.id, ExperimentStatus::Running, None)
        .await
        .expect("first experiment should start");

    // Create a second experiment on the same flag_rule_id
    let exp2 = make_experiment(env_id, rule_id);
    repo.create(&exp2).await.unwrap();

    // Attempting to run the second experiment on the same rule must fail with UniqueViolation
    let result = repo
        .apply_transition(exp2.id, ExperimentStatus::Running, None)
        .await;

    assert!(
        matches!(result, Err(RepositoryError::UniqueViolation { ref field }) if field == "flag_rule_id"),
        "second active experiment on same rule must return UniqueViolation, got: {result:?}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_invalid_transition_rejected(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // draft → paused is invalid
    let result = repo
        .apply_transition(exp.id, ExperimentStatus::Paused, None)
        .await;

    assert!(
        matches!(result, Err(RepositoryError::InvalidState { .. })),
        "invalid transition must return InvalidState, got: {result:?}"
    );

    // draft → stopped is also invalid
    let result = repo
        .apply_transition(exp.id, ExperimentStatus::Stopped, None)
        .await;

    assert!(
        matches!(result, Err(RepositoryError::InvalidState { .. })),
        "invalid transition must return InvalidState, got: {result:?}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_stopped_to_running_restart_increments_iteration(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // draft → running → stopped → running (restart)
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();
    repo.apply_transition(exp.id, ExperimentStatus::Stopped, None)
        .await
        .unwrap();
    let updated = repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("stopped→running restart should succeed");

    assert_eq!(updated.status, ExperimentStatus::Running);

    // Should now have 2 iterations, second one active with iteration_number=2
    let iterations = repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(
        iterations.len(),
        2,
        "should have 2 iterations after restart"
    );
    assert_eq!(
        iterations[1].iteration_number, 2,
        "restart iteration must be iteration #2"
    );
    assert!(
        iterations[1].ended_at.is_none(),
        "new iteration must be active"
    );

    // Flag rule must be frozen again
    let frozen: bool = sqlx::query_scalar!(
        "SELECT frozen FROM feature_flag_rules WHERE id = $1",
        rule_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(frozen, "flag rule must be frozen on restart");
}

// ---------------------------------------------------------------------------
// Full lifecycle integration tests
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn test_full_lifecycle_draft_running_paused_running_stopped(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // draft → running (creates iteration 1, freezes rule)
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("draft→running should succeed");

    let iterations = repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(iterations.len(), 1);
    assert_eq!(iterations[0].iteration_number, 1);
    assert!(
        iterations[0].ended_at.is_none(),
        "iteration 1 should be active"
    );

    let frozen: bool = sqlx::query_scalar!(
        "SELECT frozen FROM feature_flag_rules WHERE id = $1",
        rule_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(frozen, "rule must be frozen after →running");

    // running → paused (ends iteration 1, unfreezes rule)
    repo.apply_transition(exp.id, ExperimentStatus::Paused, None)
        .await
        .expect("running→paused should succeed");

    let iterations = repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(iterations.len(), 1);
    assert!(
        iterations[0].ended_at.is_some(),
        "iteration 1 must be ended after pause"
    );

    let frozen: bool = sqlx::query_scalar!(
        "SELECT frozen FROM feature_flag_rules WHERE id = $1",
        rule_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!frozen, "rule must be unfrozen after →paused");

    // paused → running (creates iteration 2, refreezes rule)
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("paused→running should succeed");

    let iterations = repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(iterations.len(), 2);
    assert_eq!(iterations[1].iteration_number, 2);
    assert!(
        iterations[1].ended_at.is_none(),
        "iteration 2 should be active"
    );

    let frozen: bool = sqlx::query_scalar!(
        "SELECT frozen FROM feature_flag_rules WHERE id = $1",
        rule_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(frozen, "rule must be refrozen after paused→running");

    // running → stopped (ends iteration 2, unfreezes rule)
    let final_exp = repo
        .apply_transition(exp.id, ExperimentStatus::Stopped, None)
        .await
        .expect("running→stopped should succeed");

    assert_eq!(final_exp.status, ExperimentStatus::Stopped);

    let iterations = repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(iterations.len(), 2, "must have exactly 2 iterations");
    assert!(
        iterations[0].ended_at.is_some(),
        "iteration 1 must have ended_at set"
    );
    assert!(
        iterations[1].ended_at.is_some(),
        "iteration 2 must have ended_at set"
    );

    let frozen: bool = sqlx::query_scalar!(
        "SELECT frozen FROM feature_flag_rules WHERE id = $1",
        rule_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!frozen, "rule must be unfrozen after →stopped");
}

#[sqlx::test(migrations = "./migrations")]
async fn test_stopped_to_running_second_restart_increments_to_iteration_3(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // draft → running (iter 1) → stopped → running (iter 2) → stopped → running (iter 3)
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();
    repo.apply_transition(exp.id, ExperimentStatus::Stopped, None)
        .await
        .unwrap();
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .unwrap();
    repo.apply_transition(exp.id, ExperimentStatus::Stopped, None)
        .await
        .unwrap();
    let updated = repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("second restart (stopped→running) should succeed");

    assert_eq!(updated.status, ExperimentStatus::Running);

    let iterations = repo.list_iterations(exp.id).await.unwrap();
    assert_eq!(
        iterations.len(),
        3,
        "should have 3 iterations after second restart"
    );
    assert_eq!(
        iterations[2].iteration_number, 3,
        "second restart must be iteration #3"
    );
    assert!(
        iterations[2].ended_at.is_none(),
        "iteration 3 must be active"
    );

    // All previous iterations must be ended
    assert!(
        iterations[0].ended_at.is_some(),
        "iteration 1 must have ended_at set"
    );
    assert!(
        iterations[1].ended_at.is_some(),
        "iteration 2 must have ended_at set"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_mutation_guard_patch_while_running_rejected(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // Transition to running
    let running = repo
        .apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("draft→running should succeed");

    // Attempt to update a running experiment
    let mut to_update = running.clone();
    to_update.name = "Mutated Name".into();
    let result = repo.update(&to_update).await;

    assert!(
        matches!(result, Err(RepositoryError::InvalidState { .. })),
        "update while running must return InvalidState, got: {:?}",
        result
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_flag_rule_frozen_while_running(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // Transition to running
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("draft→running should succeed");

    // Verify rule is frozen
    let frozen: bool = sqlx::query_scalar!(
        "SELECT frozen FROM feature_flag_rules WHERE id = $1",
        rule_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        frozen,
        "flag rule must be frozen while experiment is running"
    );

    // Transition to paused
    repo.apply_transition(exp.id, ExperimentStatus::Paused, None)
        .await
        .expect("running→paused should succeed");

    // Verify rule is unfrozen
    let frozen: bool = sqlx::query_scalar!(
        "SELECT frozen FROM feature_flag_rules WHERE id = $1",
        rule_id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !frozen,
        "flag rule must be unfrozen when experiment is paused"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_two_experiments_same_rule_second_active_rejected(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    // Create two experiments bound to the same flag_rule_id
    let exp1 = make_experiment(env_id, rule_id);
    let exp2 = make_experiment(env_id, rule_id);
    repo.create(&exp1).await.unwrap();
    repo.create(&exp2).await.unwrap();

    // Transition first to running (succeeds)
    repo.apply_transition(exp1.id, ExperimentStatus::Running, None)
        .await
        .expect("first experiment should start");

    // Transition second to running (must fail with UniqueViolation)
    let result = repo
        .apply_transition(exp2.id, ExperimentStatus::Running, None)
        .await;

    assert!(
        matches!(result, Err(RepositoryError::UniqueViolation { ref field }) if field == "flag_rule_id"),
        "second active experiment on same rule must return UniqueViolation(flag_rule_id), got: {:?}",
        result
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_soft_delete_while_running_rejected(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // Transition to running
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("draft→running should succeed");

    // Attempt soft delete while running
    let result = repo.soft_delete(exp.id).await;

    assert!(
        matches!(result, Err(RepositoryError::InvalidState { .. })),
        "soft_delete while running must return InvalidState, got: {:?}",
        result
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_soft_delete_while_stopped_succeeds(pool: sqlx::PgPool) {
    let (env_id, rule_id) = setup_experiment_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExperimentRepository::new(pool.clone(), audit);

    let exp = make_experiment(env_id, rule_id);
    repo.create(&exp).await.unwrap();

    // Transition to running → stopped
    repo.apply_transition(exp.id, ExperimentStatus::Running, None)
        .await
        .expect("draft→running should succeed");
    repo.apply_transition(exp.id, ExperimentStatus::Stopped, None)
        .await
        .expect("running→stopped should succeed");

    // Soft delete should succeed
    repo.soft_delete(exp.id)
        .await
        .expect("soft_delete of stopped experiment must succeed");

    // Experiment should no longer be findable
    let result = repo.find_by_id(exp.id).await;
    assert!(
        matches!(result, Err(RepositoryError::NotFound { .. })),
        "deleted experiment should not be found, got: {:?}",
        result
    );

    // Verify deleted_at is set in the database
    let deleted_at: Option<chrono::DateTime<Utc>> = sqlx::query_scalar!(
        "SELECT deleted_at FROM experiments WHERE id = $1",
        exp.id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        deleted_at.is_some(),
        "deleted_at must be set after soft delete"
    );
}
