use std::sync::Arc;
use stitchd_core::{
    id::{OrganisationId, ProjectId},
    tenant::{Organisation, Project},
};
use stitchd_db::{
    OrganisationRepository, ProjectRepository,
    repository::pg::{PgAuditLogger, PgOrganisationRepository, PgProjectRepository},
};

#[sqlx::test(migrations = "./migrations")]
async fn test_project_lifecycle(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let repo = PgProjectRepository::new(pool.clone(), audit);

    // 0. Setup Org
    let org = Organisation {
        id: OrganisationId::new(),
        name: "Test Org".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    // 1. Create
    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "Test Project".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };

    repo.create(&project)
        .await
        .expect("Failed to create project");

    // 2. Find
    let found = repo
        .find_by_id(project.id)
        .await
        .expect("Failed to find project");
    assert_eq!(found.name, "Test Project");

    // 3. Update
    let mut to_update = found.clone();
    to_update.name = "Updated Project".to_string();
    let updated = repo
        .update(&to_update)
        .await
        .expect("Failed to update project");
    assert_eq!(updated.name, "Updated Project");
    assert_eq!(updated.version, 2);

    // 4. List by Org
    let all = repo
        .list_by_organisation(org.id)
        .await
        .expect("Failed to list projects");
    assert!(all.iter().any(|p| p.id == project.id));

    // 5. Soft Delete
    repo.soft_delete(project.id)
        .await
        .expect("Failed to soft delete");

    // 6. Find (Should be NotFound)
    let not_found = repo.find_by_id(project.id).await.unwrap_err();
    assert!(matches!(
        not_found,
        stitchd_db::RepositoryError::NotFound { .. }
    ));
}
