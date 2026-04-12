use std::sync::Arc;
use stitchd_core::{
    id::{OrganisationId, ProjectId, RoleId, UserId},
    tenant::{Organisation, Project},
    user::{Action, Permission, ResourceType, Role, User},
};
use stitchd_db::{
    OrganisationRepository, ProjectRepository, RoleRepository, UserRepository,
    repository::pg::{
        PgAuditLogger, PgOrganisationRepository, PgProjectRepository, PgRoleRepository,
        PgUserRepository,
    },
};

#[sqlx::test(migrations = "./migrations")]
async fn test_user_lifecycle(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let role_repo = PgRoleRepository::new(pool.clone(), audit.clone());
    let repo = PgUserRepository::new(pool.clone(), audit);

    // 0. Setup
    let org = Organisation {
        id: OrganisationId::new(),
        name: "Org".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
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
        password_hash: "hash".to_string(),
        organisation_id: org.id,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&user).await.expect("Failed to create user");

    // 2. Find by Email
    let found = repo
        .find_by_email("test@example.com", org.id)
        .await
        .expect("Failed to find by email");
    assert_eq!(found.id, user.id);

    // 3. Setup Role & Permissions
    let role = Role {
        id: RoleId::new(),
        project_id: project.id,
        name: "Admin".to_string(),
        permissions: vec![Permission {
            resource_type: ResourceType::Flag,
            resource_pattern: "*".to_string(),
            action: Action::Admin,
        }],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    role_repo.create(&role).await.unwrap();

    // 4. Assign Role to User
    sqlx::query!(
        "INSERT INTO user_project_roles (user_id, project_id, role_id) VALUES ($1, $2, $3)",
        user.id as UserId,
        project.id as ProjectId,
        role.id as RoleId
    )
    .execute(&pool)
    .await
    .unwrap();

    // 5. Find Permissions
    let permissions = repo
        .find_permissions_for_user(user.id, project.id)
        .await
        .expect("Failed to find permissions");
    assert_eq!(permissions.len(), 1);
    assert_eq!(permissions[0].action, Action::Admin);
}
