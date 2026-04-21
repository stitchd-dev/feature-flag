use std::sync::Arc;
use stitchd_core::{
    auth::{User, UserStatus},
    id::{OrganisationId, ProjectId, UserId},
    tenant::{Organisation, Project},
};
use stitchd_db::{
    OrganisationRepository, ProjectRepository, UserRepository,
    repository::pg::{
        PgAuditLogger, PgOrganisationRepository, PgProjectRepository, PgUserRepository,
    },
};

#[sqlx::test(migrations = "./migrations")]
async fn test_user_lifecycle(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let repo = PgUserRepository::new(pool.clone(), audit);

    // 0. Setup
    let org = Organisation {
        id: OrganisationId::new(),
        name: "Org".into(),
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
        name: "Proj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    // 1. Create User
    let user = User {
        id: UserId::new(),
        email: "test@example.com".to_string(),
        display_name: "Test User".to_string(),
        avatar_url: None,
        password_hash: Some("hash".to_string()),
        token_secret: uuid::Uuid::new_v4(),
        totp_secret: None,
        totp_enabled: false,
        status: UserStatus::Active,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    repo.create(&user).await.expect("Failed to create user");

    // Add org membership so list_by_organisation works
    sqlx::query!(
        "INSERT INTO org_memberships (user_id, org_id, role) VALUES ($1, $2, 'org_member')",
        user.id.as_uuid(),
        org.id.as_uuid(),
    )
    .execute(&pool)
    .await
    .unwrap();

    // 2. Find by Email
    let found = repo
        .find_by_email("test@example.com")
        .await
        .expect("Failed to find by email");
    assert_eq!(found.id, user.id);

    // 3. Find by ID
    let by_id = repo
        .find_by_id(user.id)
        .await
        .expect("Failed to find by id");
    assert_eq!(by_id.email, user.email);

    // 4. List by organisation (via org_memberships join)
    let members = repo
        .list_by_organisation(org.id)
        .await
        .expect("Failed to list");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id, user.id);

    // 5. Find Permissions — new schema returns empty vec (RBAC via user_project_roles)
    let permissions = repo
        .find_permissions_for_user(user.id, project.id)
        .await
        .expect("Failed to find permissions");
    assert!(permissions.is_empty());
}
