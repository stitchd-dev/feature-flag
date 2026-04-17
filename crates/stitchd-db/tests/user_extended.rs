/// Extended integration tests for user.rs covering edge cases.
use std::sync::Arc;
use stitchd_core::{
    id::{OrganisationId, ProjectId, UserId},
    tenant::{Organisation, Project},
    user::User,
};
use stitchd_db::{
    OrganisationRepository, ProjectRepository, RepositoryError, UserRepository,
    repository::pg::{
        PgAuditLogger, PgOrganisationRepository, PgProjectRepository, PgUserRepository,
    },
};

async fn setup(pool: &sqlx::PgPool) -> (Arc<PgAuditLogger>, OrganisationId, ProjectId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "UserExtOrg".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "UserExtProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    (audit, org.id, project.id)
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_find_by_id_not_found(pool: sqlx::PgPool) {
    let (audit, _, _) = setup(&pool).await;
    let repo = PgUserRepository::new(pool, audit);
    let err = repo.find_by_id(UserId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_find_by_email_not_found(pool: sqlx::PgPool) {
    let (audit, org_id, _) = setup(&pool).await;
    let repo = PgUserRepository::new(pool, audit);
    let err = repo
        .find_by_email("nobody@example.com", org_id)
        .await
        .unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_list_by_organisation(pool: sqlx::PgPool) {
    let (audit, org_id, _) = setup(&pool).await;
    let repo = PgUserRepository::new(pool, audit);

    let user = User {
        id: UserId::new(),
        email: "list@example.com".into(),
        password_hash: "hash".into(),
        organisation_id: org_id,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&user).await.unwrap();

    let users = repo.list_by_organisation(org_id).await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].email, "list@example.com");
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_update_version_conflict(pool: sqlx::PgPool) {
    let (audit, org_id, _) = setup(&pool).await;
    let repo = PgUserRepository::new(pool, audit);

    let user = User {
        id: UserId::new(),
        email: "conflict@example.com".into(),
        password_hash: "hash".into(),
        organisation_id: org_id,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&user).await.unwrap();

    let mut updated = user.clone();
    updated.email = "changed@example.com".into();
    repo.update(&updated).await.unwrap();

    let err = repo.update(&user).await.unwrap_err();
    assert!(matches!(err, RepositoryError::VersionConflict { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_update_not_found(pool: sqlx::PgPool) {
    let (audit, org_id, _) = setup(&pool).await;
    let repo = PgUserRepository::new(pool, audit);

    let ghost = User {
        id: UserId::new(),
        email: "ghost@example.com".into(),
        password_hash: "hash".into(),
        organisation_id: org_id,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    let err = repo.update(&ghost).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_update_soft_deleted(pool: sqlx::PgPool) {
    let (audit, org_id, _) = setup(&pool).await;
    // Use raw query to soft-delete since UserRepository has no soft_delete method
    let repo = PgUserRepository::new(pool.clone(), audit);

    let user = User {
        id: UserId::new(),
        email: "softdel@example.com".into(),
        password_hash: "hash".into(),
        organisation_id: org_id,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&user).await.unwrap();

    // Manually soft-delete the user
    sqlx::query(
        "UPDATE users SET deleted_at = NOW() WHERE id = $1",
    )
    .bind(user.id as UserId)
    .execute(&pool)
    .await
    .unwrap();

    let err = repo.update(&user).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_find_permissions_empty(pool: sqlx::PgPool) {
    let (audit, org_id, project_id) = setup(&pool).await;
    let repo = PgUserRepository::new(pool.clone(), audit);

    let user = User {
        id: UserId::new(),
        email: "noperm@example.com".into(),
        password_hash: "hash".into(),
        organisation_id: org_id,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&user).await.unwrap();

    let perms = repo
        .find_permissions_for_user(user.id, project_id)
        .await
        .unwrap();
    assert!(perms.is_empty());
}
