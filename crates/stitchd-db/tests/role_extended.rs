/// Extended integration tests for role.rs covering edge cases.
use std::sync::Arc;
use stitchd_core::{
    id::{OrganisationId, ProjectId, RoleId},
    tenant::{Organisation, Project},
    user::{Action, Permission, ResourceType, Role},
};
use stitchd_db::{
    OrganisationRepository, ProjectRepository, RepositoryError, RoleRepository,
    repository::pg::{
        PgAuditLogger, PgOrganisationRepository, PgProjectRepository, PgRoleRepository,
    },
};

async fn setup(pool: &sqlx::PgPool) -> (Arc<PgAuditLogger>, ProjectId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "RoleExtOrg".into(),
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
        name: "RoleExtProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    (audit, project.id)
}

#[sqlx::test(migrations = "./migrations")]
async fn test_role_find_by_id_not_found(pool: sqlx::PgPool) {
    let (audit, _) = setup(&pool).await;
    let repo = PgRoleRepository::new(pool, audit);
    let err = repo.find_by_id(RoleId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_role_list_by_project(pool: sqlx::PgPool) {
    let (audit, project_id) = setup(&pool).await;
    let repo = PgRoleRepository::new(pool, audit);

    for name in ["Viewer", "Editor"] {
        repo.create(&Role {
            id: RoleId::new(),
            project_id,
            name: name.into(),
            permissions: vec![],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        })
        .await
        .unwrap();
    }

    let roles = repo.list_by_project(project_id).await.unwrap();
    assert_eq!(roles.len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_role_update_version_conflict(pool: sqlx::PgPool) {
    let (audit, project_id) = setup(&pool).await;
    let repo = PgRoleRepository::new(pool, audit);

    let role = Role {
        id: RoleId::new(),
        project_id,
        name: "ConflictRole".into(),
        permissions: vec![],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&role).await.unwrap();

    let mut updated = role.clone();
    updated.name = "UpdatedRole".into();
    repo.update(&updated).await.unwrap();

    let err = repo.update(&role).await.unwrap_err();
    assert!(matches!(err, RepositoryError::VersionConflict { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_role_update_not_found(pool: sqlx::PgPool) {
    let (audit, project_id) = setup(&pool).await;
    let repo = PgRoleRepository::new(pool, audit);

    let ghost = Role {
        id: RoleId::new(),
        project_id,
        name: "Ghost".into(),
        permissions: vec![],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    let err = repo.update(&ghost).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_role_soft_delete_not_found(pool: sqlx::PgPool) {
    let (audit, _) = setup(&pool).await;
    let repo = PgRoleRepository::new(pool, audit);
    let err = repo.soft_delete(RoleId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_role_update_soft_deleted(pool: sqlx::PgPool) {
    let (audit, project_id) = setup(&pool).await;
    let repo = PgRoleRepository::new(pool, audit);

    let role = Role {
        id: RoleId::new(),
        project_id,
        name: "SoftDelRole".into(),
        permissions: vec![],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&role).await.unwrap();
    repo.soft_delete(role.id).await.unwrap();

    let err = repo.update(&role).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_role_update_replaces_permissions(pool: sqlx::PgPool) {
    let (audit, project_id) = setup(&pool).await;
    let repo = PgRoleRepository::new(pool, audit);

    let role = Role {
        id: RoleId::new(),
        project_id,
        name: "PermRole".into(),
        permissions: vec![Permission {
            resource_type: ResourceType::Flag,
            resource_pattern: "*".into(),
            action: Action::Read,
        }],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&role).await.unwrap();

    let found = repo.find_by_id(role.id).await.unwrap();
    assert_eq!(found.permissions.len(), 1);

    let mut updated = found.clone();
    updated.permissions = vec![
        Permission {
            resource_type: ResourceType::Environment,
            resource_pattern: "*".into(),
            action: Action::Write,
        },
        Permission {
            resource_type: ResourceType::Segment,
            resource_pattern: "prod-*".into(),
            action: Action::Admin,
        },
    ];
    let result = repo.update(&updated).await.unwrap();
    assert_eq!(result.permissions.len(), 2);
}
