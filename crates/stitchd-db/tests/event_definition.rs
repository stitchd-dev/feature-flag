use std::sync::Arc;

use stitchd_core::{
    event::{EventDefinition, EventValueType, MetricType},
    id::{EnvironmentId, EventDefinitionId, OrganisationId, ProjectId},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, EventDefinitionRepository, OrganisationRepository, ProjectRepository,
    RepositoryError,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgEventDefinitionRepository,
        PgOrganisationRepository, PgProjectRepository,
    },
};

/// Build a minimal environment hierarchy for tests.
async fn setup_env(pool: sqlx::PgPool) -> (Organisation, Project, Environment) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "TestOrg".into(),
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
        name: "TestProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "TestEnv".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    (org, project, env)
}

#[sqlx::test(migrations = "./migrations")]
async fn test_create_and_find_event_definition(pool: sqlx::PgPool) {
    let (_org, _project, env) = setup_env(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool.clone(), audit);

    let def = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env.id,
        key: "page_view".to_string(),
        name: "page_view".to_string(),
        description: None,
        metric_type: MetricType::Count,
        schema: None,
        value_type: EventValueType::Int,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };

    repo.create(&def).await.expect("create should succeed");

    let found = repo
        .find_by_key("page_view", env.id)
        .await
        .expect("should find by key");

    assert_eq!(found.id, def.id);
    assert_eq!(found.key, "page_view");
    assert_eq!(found.value_type, EventValueType::Int);
    assert_eq!(found.version, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_list_event_definitions(pool: sqlx::PgPool) {
    let (_org, _project, env) = setup_env(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool.clone(), audit);

    for (key, vt) in [
        ("clicks", EventValueType::Int),
        ("revenue", EventValueType::Double),
        ("converted", EventValueType::Bool),
    ] {
        let def = EventDefinition {
            id: EventDefinitionId::new(),
            environment_id: env.id,
            key: key.to_string(),
            name: key.to_string(),
            description: None,
            metric_type: MetricType::Count,
            schema: None,
            value_type: vt,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        };
        repo.create(&def).await.unwrap();
    }

    let list = repo
        .list_by_environment(env.id)
        .await
        .expect("list should succeed");

    assert_eq!(list.len(), 3);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_soft_delete_event_definition(pool: sqlx::PgPool) {
    let (_org, _project, env) = setup_env(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool.clone(), audit);

    let def = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env.id,
        key: "revenue".to_string(),
        name: "revenue".to_string(),
        description: None,
        metric_type: MetricType::Count,
        schema: None,
        value_type: EventValueType::Double,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&def).await.unwrap();

    repo.soft_delete(def.id)
        .await
        .expect("soft delete should succeed");

    let result = repo.find_by_key("revenue", env.id).await;
    assert!(
        matches!(result, Err(RepositoryError::NotFound { .. })),
        "deleted definition should not be found"
    );

    let list = repo.list_by_environment(env.id).await.unwrap();
    assert!(list.is_empty(), "deleted def should not appear in list");
}

#[sqlx::test(migrations = "./migrations")]
async fn test_optimistic_locking_conflict(pool: sqlx::PgPool) {
    let (_org, _project, env) = setup_env(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool.clone(), audit);

    let def = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env.id,
        key: "purchases".to_string(),
        name: "purchases".to_string(),
        description: None,
        metric_type: MetricType::Count,
        schema: None,
        value_type: EventValueType::Int,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&def).await.unwrap();

    // First update succeeds (version 1 → 2).
    let updated = repo
        .update(&def)
        .await
        .expect("first update should succeed");
    assert_eq!(updated.version, 2);

    // Second update with stale version=1 must fail.
    let result = repo.update(&def).await;
    assert!(
        matches!(result, Err(RepositoryError::VersionConflict { .. })),
        "stale version update must return VersionConflict"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn test_unique_key_per_environment(pool: sqlx::PgPool) {
    let (_org, _project, env) = setup_env(pool.clone()).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool.clone(), audit);

    let def1 = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env.id,
        key: "signup".to_string(),
        name: "signup".to_string(),
        description: None,
        metric_type: MetricType::Count,
        schema: None,
        value_type: EventValueType::Bool,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&def1).await.unwrap();

    let def2 = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env.id,
        key: "signup".to_string(),
        name: "signup".to_string(),
        description: None,
        metric_type: MetricType::Count,
        schema: None,
        value_type: EventValueType::Int,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    let result = repo.create(&def2).await;
    assert!(
        matches!(result, Err(RepositoryError::UniqueViolation { .. })),
        "duplicate key in same env must be rejected"
    );
}
