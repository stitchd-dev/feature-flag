/// Extended integration tests for organisation.rs covering edge cases.
use std::sync::Arc;
use stitchd_core::{id::OrganisationId, tenant::Organisation};
use stitchd_db::{
    OrganisationRepository, RepositoryError,
    repository::pg::{PgAuditLogger, PgOrganisationRepository},
};

#[sqlx::test(migrations = "./migrations")]
async fn test_organisation_find_by_id_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgOrganisationRepository::new(pool, audit);
    let err = repo.find_by_id(OrganisationId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_organisation_update_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgOrganisationRepository::new(pool, audit);
    let ghost = Organisation {
        id: OrganisationId::new(),
        name: "Ghost".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
        is_system: false,
    };
    let err = repo.update(&ghost).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_organisation_soft_delete_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgOrganisationRepository::new(pool, audit);
    let err = repo.soft_delete(OrganisationId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_organisation_update_soft_deleted_returns_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgOrganisationRepository::new(pool, audit);

    let org = Organisation {
        id: OrganisationId::new(),
        name: "To Delete".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
        is_system: false,
    };
    repo.create(&org).await.unwrap();
    repo.soft_delete(org.id).await.unwrap();

    let err = repo.update(&org).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_organisation_create_duplicate_returns_unique_violation(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgOrganisationRepository::new(pool, audit);

    let id = OrganisationId::new();
    let org = Organisation {
        id,
        name: "DupOrg".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
        is_system: false,
    };
    repo.create(&org).await.unwrap();

    let dup = Organisation {
        id, // same UUID
        name: "DupOrg2".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
        is_system: false,
    };
    let err = repo.create(&dup).await.unwrap_err();
    assert!(matches!(err, RepositoryError::UniqueViolation { .. }));
}
