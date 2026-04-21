/// Extended integration tests for project.rs covering edge cases.
use std::sync::Arc;
use stitchd_core::{
    id::{OrganisationId, ProjectId},
    tenant::{Organisation, Project},
};
use stitchd_db::{
    OrganisationRepository, ProjectRepository, RepositoryError,
    repository::pg::{PgAuditLogger, PgOrganisationRepository, PgProjectRepository},
};

async fn setup_org(pool: &sqlx::PgPool, audit: &Arc<PgAuditLogger>) -> OrganisationId {
    let repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let org = Organisation {
        id: OrganisationId::new(),
        name: "ProjExtOrg".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
        is_system: false,
};
    repo.create(&org).await.unwrap();
    org.id
}

#[sqlx::test(migrations = "./migrations")]
async fn test_project_find_by_id_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgProjectRepository::new(pool, audit);
    let err = repo.find_by_id(ProjectId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_project_update_version_conflict(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_id = setup_org(&pool, &audit).await;
    let repo = PgProjectRepository::new(pool, audit);

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org_id,
        name: "ProjConflict".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&project).await.unwrap();

    let mut updated = project.clone();
    updated.name = "NewName".into();
    repo.update(&updated).await.unwrap();

    let err = repo.update(&project).await.unwrap_err(); // stale version
    assert!(matches!(err, RepositoryError::VersionConflict { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_project_update_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_id = setup_org(&pool, &audit).await;
    let repo = PgProjectRepository::new(pool, audit);

    let ghost = Project {
        id: ProjectId::new(),
        organisation_id: org_id,
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
async fn test_project_soft_delete_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgProjectRepository::new(pool, audit);
    let err = repo.soft_delete(ProjectId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_project_update_soft_deleted_returns_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_id = setup_org(&pool, &audit).await;
    let repo = PgProjectRepository::new(pool, audit);

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org_id,
        name: "SoftDelProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&project).await.unwrap();
    repo.soft_delete(project.id).await.unwrap();

    let err = repo.update(&project).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}
