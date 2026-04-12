use std::sync::Arc;
use stitchd_core::{
    id::{EnvironmentId, OrganisationId, ProjectId, RuleId, SegmentId, VariantId},
    rule_engine::types::{ConditionExpr, Rule, RuleOutput},
    segment::{Segment, SegmentType},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, OrganisationRepository, ProjectRepository, SegmentRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgOrganisationRepository, PgProjectRepository,
        PgSegmentRepository,
    },
};

async fn setup(pool: &sqlx::PgPool) -> (PgSegmentRepository, EnvironmentId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let repo = PgSegmentRepository::new(pool.clone(), audit);

    let org = Organisation {
        id: OrganisationId::new(),
        name: "Test Org".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "Test Project".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "Test Env".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    (repo, env.id)
}

#[sqlx::test(migrations = "./migrations")]
async fn test_rule_based_segment_repository(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;

    let segment_id = SegmentId::new();
    let segment = Segment {
        id: segment_id,
        environment_id: env_id,
        key: "rule-segment".to_string(),
        segment_type: SegmentType::Rule,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&segment).await.unwrap();

    let rules = vec![Rule {
        id: RuleId::new(),
        condition: ConditionExpr::And(vec![]),
        output: RuleOutput::Variant(VariantId::new()),
    }];

    // 1. Upsert rules
    repo.upsert_rules(segment_id, &rules).await.unwrap();

    // 2. Find with rules
    let found = repo.find_with_rules(segment_id).await.unwrap();
    assert_eq!(found.rules.len(), 1);
    assert_eq!(found.rules[0].id, rules[0].id);

    // 3. Second upsert replaces
    let new_rules = vec![Rule {
        id: RuleId::new(),
        condition: ConditionExpr::Or(vec![]),
        output: RuleOutput::Variant(VariantId::new()),
    }];
    repo.upsert_rules(segment_id, &new_rules).await.unwrap();
    let found2 = repo.find_with_rules(segment_id).await.unwrap();
    assert_eq!(found2.rules.len(), 1);
    assert_eq!(found2.rules[0].id, new_rules[0].id);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_list_based_segment_repository(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;

    let segment_id = SegmentId::new();
    let segment = Segment {
        id: segment_id,
        environment_id: env_id,
        key: "list-segment".to_string(),
        segment_type: SegmentType::List,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&segment).await.unwrap();

    // 1. Set list entries
    repo.set_list_entries(segment_id, "user", &["u1".to_string()], &["u2".to_string()])
        .await
        .unwrap();

    // 2. Find with list
    let found = repo.find_with_list(segment_id).await.unwrap();
    let user_list = found.lists.get("user").unwrap();
    assert!(user_list.include.contains("u1"));
    assert!(user_list.exclude.contains("u2"));

    // 3. Set replaces for context_type
    repo.set_list_entries(segment_id, "user", &["u3".to_string()], &[])
        .await
        .unwrap();
    let found2 = repo.find_with_list(segment_id).await.unwrap();
    let user_list2 = found2.lists.get("user").unwrap();
    assert!(user_list2.include.contains("u3"));
    assert!(!user_list2.include.contains("u1"));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_wrong_type_returns_not_found(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;

    let segment_id = SegmentId::new();
    let segment = Segment {
        id: segment_id,
        environment_id: env_id,
        key: "rule-segment".to_string(),
        segment_type: SegmentType::Rule,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&segment).await.unwrap();

    // Try to find as list segment
    let result = repo.find_with_list(segment_id).await;
    assert!(result.is_err());
    // Spec says NotFound for wrong type
}
