use std::sync::Arc;
use stitchd_core::{id::{OrganisationId, ProjectId, RoleId}, tenant::{Organisation, Project}, user::{Role, Permission, ResourceType, Action}};
use stitchd_db::{
    RoleRepository, ProjectRepository, OrganisationRepository,
    repository::pg::{PgAuditLogger, PgOrganisationRepository, PgProjectRepository, PgRoleRepository},
};

#[sqlx::test(migrations = "./migrations")]
async fn test_role_lifecycle(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let repo = PgRoleRepository::new(pool.clone(), audit);

    // 0. Setup
    let org = Organisation { id: OrganisationId::new(), name: "Org".into(), created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(), deleted_at: None, version: 1 };
    org_repo.create(&org).await.unwrap();
    let project = Project { id: ProjectId::new(), organisation_id: org.id, name: "Proj".into(), created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(), deleted_at: None, version: 1 };
    proj_repo.create(&project).await.unwrap();

    // 1. Create Role
    let role = Role {
        id: RoleId::new(),
        project_id: project.id,
        name: "Editor".to_string(),
        permissions: vec![Permission {
            resource_type: ResourceType::Flag,
            resource_pattern: "payments-*".to_string(),
            action: Action::Write,
        }],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&role).await.expect("Failed to create role");

    // 2. Find
    let found = repo.find_by_id(role.id).await.expect("Failed to find role");
    assert_eq!(found.permissions.len(), 1);
    assert_eq!(found.permissions[0].resource_pattern, "payments-*");

    // 3. Update
    let mut to_update = found.clone();
    to_update.permissions.push(Permission {
        resource_type: ResourceType::Segment,
        resource_pattern: "*".to_string(),
        action: Action::Read,
    });
    let updated = repo.update(&to_update).await.expect("Failed to update role");
    assert_eq!(updated.permissions.len(), 2);

    // 4. Soft Delete
    repo.soft_delete(role.id).await.expect("Failed to soft delete");
    let not_found = repo.find_by_id(role.id).await.unwrap_err();
    assert!(matches!(not_found, stitchd_db::RepositoryError::NotFound { .. }));
}
