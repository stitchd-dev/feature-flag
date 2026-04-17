/// Extended integration tests for environment.rs covering edge cases.
use std::sync::Arc;
use stitchd_core::{
    id::{EnvironmentId, OrganisationId, ProjectId},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, OrganisationRepository, ProjectRepository, RepositoryError,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgOrganisationRepository, PgProjectRepository,
    },
};

async fn setup(pool: &sqlx::PgPool) -> (Arc<PgAuditLogger>, ProjectId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "EnvExtOrg".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "EnvExtProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    (audit, project.id)
}

#[sqlx::test(migrations = "./migrations")]
async fn test_environment_find_by_id_not_found(pool: sqlx::PgPool) {
    let (audit, _) = setup(&pool).await;
    let repo = PgEnvironmentRepository::new(pool, audit);
    let err = repo.find_by_id(EnvironmentId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_environment_update_version_conflict(pool: sqlx::PgPool) {
    let (audit, project_id) = setup(&pool).await;
    let repo = PgEnvironmentRepository::new(pool, audit);

    let env = Environment {
        id: EnvironmentId::new(),
        project_id,
        name: "ConflictEnv".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&env).await.unwrap();

    let mut updated = env.clone();
    updated.name = "NewName".into();
    repo.update(&updated).await.unwrap();

    let err = repo.update(&env).await.unwrap_err();
    assert!(matches!(err, RepositoryError::VersionConflict { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_environment_update_not_found(pool: sqlx::PgPool) {
    let (audit, project_id) = setup(&pool).await;
    let repo = PgEnvironmentRepository::new(pool, audit);

    let ghost = Environment {
        id: EnvironmentId::new(),
        project_id,
        name: "Ghost".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    let err = repo.update(&ghost).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_environment_soft_delete_not_found(pool: sqlx::PgPool) {
    let (audit, _) = setup(&pool).await;
    let repo = PgEnvironmentRepository::new(pool, audit);
    let err = repo.soft_delete(EnvironmentId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_environment_update_soft_deleted(pool: sqlx::PgPool) {
    let (audit, project_id) = setup(&pool).await;
    let repo = PgEnvironmentRepository::new(pool, audit);

    let env = Environment {
        id: EnvironmentId::new(),
        project_id,
        name: "SoftDelEnv".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&env).await.unwrap();
    repo.soft_delete(env.id).await.unwrap();

    let err = repo.update(&env).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}
