/// Extended integration tests for PgUserRepository covering edge cases.
use std::sync::Arc;
use stitchd_core::{
    auth::{User, UserStatus},
    id::{OrganisationId, ProjectId, UserId},
    tenant::{Organisation, Project},
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
        is_system: false,
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

fn make_user(email: &str) -> User {
    User {
        id: UserId::new(),
        email: email.to_string(),
        display_name: "Test User".to_string(),
        avatar_url: None,
        password_hash: Some("hash".to_string()),
        token_secret: uuid::Uuid::new_v4(),
        totp_secret: None,
        totp_enabled: false,
        status: UserStatus::Active,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
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
    let (audit, _, _) = setup(&pool).await;
    let repo = PgUserRepository::new(pool, audit);
    let err = repo.find_by_email("nobody@example.com").await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_list_by_organisation(pool: sqlx::PgPool) {
    let (audit, org_id, _) = setup(&pool).await;
    let repo = PgUserRepository::new(pool.clone(), audit);

    let user = make_user("list@example.com");
    repo.create(&user).await.unwrap();

    // Wire up membership so the join in list_by_organisation returns the user
    sqlx::query!(
        "INSERT INTO org_memberships (user_id, org_id, role) VALUES ($1, $2, 'org_member')",
        user.id.as_uuid(),
        org_id.as_uuid(),
    )
    .execute(&pool)
    .await
    .unwrap();

    let users = repo.list_by_organisation(org_id).await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].email, "list@example.com");
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_update_not_found(pool: sqlx::PgPool) {
    let (audit, _, _) = setup(&pool).await;
    let repo = PgUserRepository::new(pool, audit);

    let ghost = make_user("ghost@example.com");
    let err = repo.update(&ghost).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_update_changes_fields(pool: sqlx::PgPool) {
    let (audit, _, _) = setup(&pool).await;
    let repo = PgUserRepository::new(pool, audit);

    let user = make_user("updateme@example.com");
    repo.create(&user).await.unwrap();

    let mut updated = user.clone();
    updated.display_name = "Updated Name".to_string();
    let result = repo.update(&updated).await.unwrap();
    assert_eq!(result.display_name, "Updated Name");
}

#[sqlx::test(migrations = "./migrations")]
async fn test_user_find_permissions_empty(pool: sqlx::PgPool) {
    let (audit, _, project_id) = setup(&pool).await;
    let repo = PgUserRepository::new(pool.clone(), audit);

    let user = make_user("noperm@example.com");
    repo.create(&user).await.unwrap();

    let perms = repo
        .find_permissions_for_user(user.id, project_id)
        .await
        .unwrap();
    assert!(perms.is_empty());
}
