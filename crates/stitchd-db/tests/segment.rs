use std::sync::Arc;
use stitchd_core::{
    id::{EnvironmentId, OrganisationId, ProjectId, SegmentId},
    segment::{Segment, SegmentType},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, OrganisationRepository, ProjectRepository, SegmentRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgOrganisationRepository, PgProjectRepository,
        PgSegmentRepository,
    },
};

#[sqlx::test(migrations = "./migrations")]
async fn test_segment_lifecycle(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let repo = PgSegmentRepository::new(pool.clone(), audit);

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
    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "Env".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    // 1. Create Segment
    let segment = Segment {
        id: SegmentId::new(),
        environment_id: env.id,
        key: "beta-testers".to_string(),
        segment_type: SegmentType::Rule,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&segment)
        .await
        .expect("Failed to create segment");

    // 2. Find by Key
    let found = repo
        .find_by_key("beta-testers", env.id)
        .await
        .expect("Failed to find segment");
    assert_eq!(found.id, segment.id);

    // 3. Update
    let mut to_update = found.clone();
    to_update.segment_type = SegmentType::List;
    let updated = repo
        .update(&to_update)
        .await
        .expect("Failed to update segment");
    assert_eq!(updated.segment_type, SegmentType::List);

    // 4. Soft Delete
    repo.soft_delete(segment.id)
        .await
        .expect("Failed to soft delete");
    let not_found = repo.find_by_id(segment.id).await.unwrap_err();
    assert!(matches!(
        not_found,
        stitchd_db::RepositoryError::NotFound { .. }
    ));
}
