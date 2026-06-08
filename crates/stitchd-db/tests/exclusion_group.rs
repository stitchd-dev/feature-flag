//! Integration tests for the exclusion-group PG repository: CRUD, the
//! basis-point range allocator (allocation, capacity rejection, free-and-reuse),
//! optimistic-concurrency version conflicts, and the `set_rule_exclusion_gate`
//! round-trip on the flag repo.

use std::sync::Arc;

use chrono::Utc;
use stitchd_core::{
    experimentation::{Experiment, ExperimentStatus},
    flag::{FlagRecord, FlagRule, FlagValueType},
    id::{
        EnvironmentId, ExperimentId, FlagId, FlagKey, MetricId, OrganisationId, ProjectId, RuleId,
        VariantId,
    },
    rule_engine::types::{
        ConditionExpr, ExclusionGate, PercentageTarget, Rule, RuleOutput, TargetField,
    },
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, ExperimentRepository, FlagRepository, OrganisationRepository,
    ProjectRepository, RepositoryError,
    repository::pg::{
        ExclusionGroupRepository, PgAuditLogger, PgEnvironmentRepository,
        PgExclusionGroupRepository, PgExperimentRepository, PgFlagRepository,
        PgOrganisationRepository, PgProjectRepository,
    },
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

struct Deps {
    env_id: EnvironmentId,
    flag_id: FlagId,
    flag_rule_id: RuleId,
}

/// Insert org → project → environment → flag → percentage rule, returning the
/// IDs needed to exercise exclusion groups. The flag rule carries a Percentage
/// output so the exclusion gate can be set on it.
async fn setup_deps(pool: sqlx::PgPool) -> Deps {
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
        is_system: false,
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

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id: project.id,
        key: FlagKey::new("excl-flag").unwrap(),
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
    flag_repo.create(&flag).await.unwrap();

    // A Percentage rule so the exclusion gate has somewhere to live.
    let rules = vec![FlagRule {
        flag_id: flag.id,
        rule_index: 0,
        rule: Rule {
            id: RuleId::new(),
            name: None,
            condition: ConditionExpr::And(vec![]),
            output: RuleOutput::Percentage {
                targets: vec![PercentageTarget {
                    context_type: "user".to_string(),
                    field: TargetField::Key,
                }],
                weights: vec![(VariantId::new(), 10_000)],
                exclusion_gate: None,
                realtime_bandit: None,
            },
        },
    }];
    flag_repo.upsert_rules(flag.id, &rules).await.unwrap();

    let rule_uuid: uuid::Uuid = sqlx::query_scalar!(
        "SELECT id FROM feature_flag_rules WHERE flag_id = $1 LIMIT 1",
        flag.id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    Deps {
        env_id: env.id,
        flag_id: flag.id,
        flag_rule_id: RuleId::from_uuid(rule_uuid),
    }
}

/// Create a draft experiment with the given traffic allocation (%).
async fn make_experiment(
    repo: &PgExperimentRepository,
    deps: &Deps,
    traffic_allocation: f64,
) -> ExperimentId {
    let id = ExperimentId::new();
    let exp = Experiment {
        id,
        environment_id: deps.env_id,
        flag_id: deps.flag_id,
        flag_key: None,
        variant_keys: vec![],
        flag_rule_id: Some(deps.flag_rule_id),
        targets_default_rule: false,
        name: format!("exp-{traffic_allocation}"),
        description: None,
        hypothesis: None,
        metric_ids: vec![MetricId::new()],
        guardrail_metric_ids: vec![],
        traffic_allocation,
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
        exclusion_group_id: None,
        group_bucket_lo: None,
        group_bucket_hi: None,
        sequential_testing_enabled: false,
        sequential_alpha: 0.05,
        sequential_tau_squared: None,
        sequential_min_sample_size: 100,
        experiment_mode: stitchd_core::experimentation::bandit::ExperimentMode::Fixed,
        bandit_config: None,
    };
    repo.create(&exp).await.unwrap();
    id
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn test_create_and_find(pool: sqlx::PgPool) {
    let deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExclusionGroupRepository::new(pool.clone(), audit);

    let group = repo
        .create(deps.env_id, "Checkout group", Some("desc"), "user")
        .await
        .expect("create should succeed");

    assert_eq!(group.name, "Checkout group");
    assert_eq!(group.description.as_deref(), Some("desc"));
    assert_eq!(group.version, 1);
    assert_eq!(group.allocated_bp, 0);
    assert_eq!(group.free_bp, 10_000);
    assert!(!group.salt.is_empty(), "salt should be generated");

    let found = repo.find_by_id(group.id).await.unwrap();
    assert_eq!(found.id, group.id);
    assert_eq!(found.salt, group.salt, "salt is stable across reads");
}

#[sqlx::test(migrations = "./migrations")]
async fn test_list_by_environment(pool: sqlx::PgPool) {
    let deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExclusionGroupRepository::new(pool.clone(), audit);

    repo.create(deps.env_id, "g1", None, "user").await.unwrap();
    repo.create(deps.env_id, "g2", None, "user").await.unwrap();

    let listed = repo.list_by_environment(deps.env_id).await.unwrap();
    assert_eq!(listed.len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_soft_delete(pool: sqlx::PgPool) {
    let deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExclusionGroupRepository::new(pool.clone(), audit);

    let group = repo
        .create(deps.env_id, "to-delete", None, "user")
        .await
        .unwrap();
    repo.soft_delete(group.id).await.unwrap();

    let err = repo.find_by_id(group.id).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));

    // Soft-deleting a missing group is NotFound.
    let err = repo.soft_delete(group.id).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

// ---------------------------------------------------------------------------
// Version conflict
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn test_update_version_conflict(pool: sqlx::PgPool) {
    let deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgExclusionGroupRepository::new(pool.clone(), audit);

    let group = repo.create(deps.env_id, "g", None, "user").await.unwrap();

    // First update succeeds, bumps version to 2.
    let updated = repo
        .update(group.id, "g-renamed", Some("now described"), group.version)
        .await
        .unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.name, "g-renamed");

    // Second update with the stale version (1) must conflict.
    let err = repo
        .update(group.id, "stale", None, group.version)
        .await
        .unwrap_err();
    match err {
        RepositoryError::VersionConflict { expected, actual } => {
            assert_eq!(expected, 1);
            assert_eq!(actual, 2);
        }
        other => panic!("expected VersionConflict, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Allocation
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn test_allocate_disjoint_ranges(pool: sqlx::PgPool) {
    let deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let group_repo = PgExclusionGroupRepository::new(pool.clone(), audit.clone());
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);

    let group = group_repo
        .create(deps.env_id, "g", None, "user")
        .await
        .unwrap();
    let e1 = make_experiment(&exp_repo, &deps, 25.0).await;
    let e2 = make_experiment(&exp_repo, &deps, 25.0).await;

    let r1 = group_repo.allocate_range(group.id, e1, 2500).await.unwrap();
    assert_eq!((r1.lo, r1.hi), (0, 2500));

    let r2 = group_repo.allocate_range(group.id, e2, 2500).await.unwrap();
    assert_eq!((r2.lo, r2.hi), (2500, 5000));

    let (allocated, free) = group_repo.allocated_free_bp(group.id).await.unwrap();
    assert_eq!(allocated, 5000);
    assert_eq!(free, 5000);

    let g = group_repo.find_by_id(group.id).await.unwrap();
    assert_eq!(g.allocated_bp, 5000);
    assert_eq!(g.free_bp, 5000);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_allocate_capacity_rejection(pool: sqlx::PgPool) {
    let deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let group_repo = PgExclusionGroupRepository::new(pool.clone(), audit.clone());
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);

    let group = group_repo
        .create(deps.env_id, "g", None, "user")
        .await
        .unwrap();
    let e1 = make_experiment(&exp_repo, &deps, 60.0).await;
    let e2 = make_experiment(&exp_repo, &deps, 60.0).await;

    group_repo.allocate_range(group.id, e1, 6000).await.unwrap();

    // Combined would be 120% > 100% — no contiguous 6000 bp window remains.
    let err = group_repo
        .allocate_range(group.id, e2, 6000)
        .await
        .unwrap_err();
    assert!(
        matches!(err, RepositoryError::InvalidState { .. }),
        "expected InvalidState (capacity), got {err:?}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_allocate_zero_and_oversize_rejected(pool: sqlx::PgPool) {
    let deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let group_repo = PgExclusionGroupRepository::new(pool.clone(), audit.clone());
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);

    let group = group_repo
        .create(deps.env_id, "g", None, "user")
        .await
        .unwrap();
    let e1 = make_experiment(&exp_repo, &deps, 0.0).await;

    assert!(matches!(
        group_repo
            .allocate_range(group.id, e1, 0)
            .await
            .unwrap_err(),
        RepositoryError::InvalidState { .. }
    ));
    assert!(matches!(
        group_repo
            .allocate_range(group.id, e1, 10_001)
            .await
            .unwrap_err(),
        RepositoryError::InvalidState { .. }
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_free_and_reuse(pool: sqlx::PgPool) {
    let deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let group_repo = PgExclusionGroupRepository::new(pool.clone(), audit.clone());
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);

    let group = group_repo
        .create(deps.env_id, "g", None, "user")
        .await
        .unwrap();
    let e1 = make_experiment(&exp_repo, &deps, 100.0).await;
    let e2 = make_experiment(&exp_repo, &deps, 100.0).await;

    // e1 takes the whole space.
    group_repo
        .allocate_range(group.id, e1, 10_000)
        .await
        .unwrap();
    assert_eq!(
        group_repo.allocated_free_bp(group.id).await.unwrap(),
        (10_000, 0)
    );

    // No room for e2.
    assert!(
        group_repo
            .allocate_range(group.id, e2, 10_000)
            .await
            .is_err()
    );

    // Free e1; space is reusable.
    group_repo.free_range(e1).await.unwrap();
    assert_eq!(
        group_repo.allocated_free_bp(group.id).await.unwrap(),
        (0, 10_000)
    );

    let r2 = group_repo
        .allocate_range(group.id, e2, 10_000)
        .await
        .unwrap();
    assert_eq!((r2.lo, r2.hi), (0, 10_000));

    // free_range is idempotent.
    group_repo.free_range(e1).await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn test_allocate_fills_internal_gap(pool: sqlx::PgPool) {
    let deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let group_repo = PgExclusionGroupRepository::new(pool.clone(), audit.clone());
    let exp_repo = PgExperimentRepository::new(pool.clone(), audit);

    let group = group_repo
        .create(deps.env_id, "g", None, "user")
        .await
        .unwrap();
    let e1 = make_experiment(&exp_repo, &deps, 25.0).await;
    let e2 = make_experiment(&exp_repo, &deps, 25.0).await;
    let e3 = make_experiment(&exp_repo, &deps, 25.0).await;

    let r1 = group_repo.allocate_range(group.id, e1, 2500).await.unwrap();
    let r2 = group_repo.allocate_range(group.id, e2, 2500).await.unwrap();
    assert_eq!((r1.lo, r1.hi), (0, 2500));
    assert_eq!((r2.lo, r2.hi), (2500, 5000));

    // Free the first member, leaving an internal gap at [0,2500).
    group_repo.free_range(e1).await.unwrap();

    // e3 should be placed into the lowest free window: [0,2500).
    let r3 = group_repo.allocate_range(group.id, e3, 2500).await.unwrap();
    assert_eq!((r3.lo, r3.hi), (0, 2500));
}

// ---------------------------------------------------------------------------
// set_rule_exclusion_gate round-trip (on the flag repo)
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn test_set_and_clear_rule_exclusion_gate(pool: sqlx::PgPool) {
    let deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let flag_repo = PgFlagRepository::new(pool.clone(), audit);

    // Initially no gate.
    let rules = flag_repo.find_rules(deps.flag_id).await.unwrap();
    assert_eq!(rules.len(), 1);
    match &rules[0].rule.output {
        RuleOutput::Percentage { exclusion_gate, .. } => assert!(exclusion_gate.is_none()),
        other => panic!("expected Percentage, got {other:?}"),
    }

    // Set the gate.
    let gate = ExclusionGate {
        group_salt: "salt-abc".to_string(),
        context_type: "user".to_string(),
        bucket_lo: 0,
        bucket_hi: 2500,
    };
    flag_repo
        .set_rule_exclusion_gate(deps.flag_rule_id, Some(gate.clone()))
        .await
        .unwrap();

    let rules = flag_repo.find_rules(deps.flag_id).await.unwrap();
    match &rules[0].rule.output {
        RuleOutput::Percentage { exclusion_gate, .. } => {
            assert_eq!(exclusion_gate.as_ref(), Some(&gate));
        }
        other => panic!("expected Percentage, got {other:?}"),
    }

    // Clear the gate.
    flag_repo
        .set_rule_exclusion_gate(deps.flag_rule_id, None)
        .await
        .unwrap();

    let rules = flag_repo.find_rules(deps.flag_id).await.unwrap();
    match &rules[0].rule.output {
        RuleOutput::Percentage { exclusion_gate, .. } => assert!(exclusion_gate.is_none()),
        other => panic!("expected Percentage, got {other:?}"),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn test_set_rule_exclusion_gate_missing_rule(pool: sqlx::PgPool) {
    let _deps = setup_deps(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let flag_repo = PgFlagRepository::new(pool.clone(), audit);

    let err = flag_repo
        .set_rule_exclusion_gate(RuleId::new(), None)
        .await
        .unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}
