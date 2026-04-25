use std::sync::Arc;

use stitchd_core::{
    event::{EventDefinition, EventValueType},
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

async fn setup(pool: &sqlx::PgPool) -> EnvironmentId {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "EdExtOrg".into(),
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
        name: "EdExtProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "EdExtEnv".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();
    env.id
}

fn make_def(env_id: EnvironmentId, key: &str) -> EventDefinition {
    EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env_id,
        key: key.to_string(),
        value_type: EventValueType::Int,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_id_returns_definition(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);

    let def = make_def(env_id, "impressions");
    repo.create(&def).await.unwrap();

    let found = repo.find_by_id(def.id).await.unwrap();
    assert_eq!(found.id, def.id);
    assert_eq!(found.key, "impressions");
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_id_not_found_returns_error(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);
    let err = repo.find_by_id(EventDefinitionId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_key_not_found_returns_error(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);
    let err = repo.find_by_key("no_such_key", env_id).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn soft_delete_not_found_returns_error(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);
    let err = repo
        .soft_delete(EventDefinitionId::new())
        .await
        .unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn update_not_found_returns_error(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);
    let ghost = make_def(env_id, "ghost");
    let err = repo.update(&ghost).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn update_stores_new_key_and_type(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);

    let def = make_def(env_id, "original_key");
    repo.create(&def).await.unwrap();

    let mut updated = def.clone();
    updated.key = "updated_key".to_string();
    updated.value_type = EventValueType::Double;
    let result = repo.update(&updated).await.unwrap();
    assert_eq!(result.key, "updated_key");
    assert_eq!(result.value_type, EventValueType::Double);
    assert_eq!(result.version, 2);
}
