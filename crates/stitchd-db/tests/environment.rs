use std::sync::Arc;
use stitchd_core::{
    id::{EnvironmentId, OrganisationId, ProjectId},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, OrganisationRepository, ProjectRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgOrganisationRepository, PgProjectRepository,
    },
};

#[sqlx::test(migrations = "./migrations")]
async fn test_environment_lifecycle(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let repo = PgEnvironmentRepository::new(pool.clone(), audit);

    // 0. Setup Org & Project
    let org = Organisation {
        id: OrganisationId::new(),
        name: "Test Org".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "Test Project".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    // 1. Create
    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "Production".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };

    repo.create(&env)
        .await
        .expect("Failed to create environment");

    // 2. Find
    let found = repo
        .find_by_id(env.id)
        .await
        .expect("Failed to find environment");
    assert_eq!(found.name, "Production");

    // 3. Update
    let mut to_update = found.clone();
    to_update.name = "Staging".to_string();
    let updated = repo
        .update(&to_update)
        .await
        .expect("Failed to update environment");
    assert_eq!(updated.name, "Staging");

    // 4. List by Project
    let all = repo
        .list_by_project(project.id)
        .await
        .expect("Failed to list environments");
    assert!(all.iter().any(|e| e.id == env.id));

    // 5. Soft Delete
    repo.soft_delete(env.id)
        .await
        .expect("Failed to soft delete");

    // 6. Find (Should be NotFound)
    let not_found = repo.find_by_id(env.id).await.unwrap_err();
    assert!(matches!(
        not_found,
        stitchd_db::RepositoryError::NotFound { .. }
    ));
}
