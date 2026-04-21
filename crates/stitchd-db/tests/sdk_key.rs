use std::sync::Arc;
use stitchd_core::{
    id::{EnvironmentId, OrganisationId, ProjectId, SdkKeyId},
    tenant::{Environment, Organisation, Project, SdkKey},
};
use stitchd_db::{
    EnvironmentRepository, OrganisationRepository, ProjectRepository, SdkKeyRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgOrganisationRepository, PgProjectRepository,
        PgSdkKeyRepository,
    },
};

#[sqlx::test(migrations = "./migrations")]
async fn test_sdk_key_lifecycle(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let repo = PgSdkKeyRepository::new(pool.clone(), audit);

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

    // 1. Create first key
    let key1 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env.id,
        key_hash: "hash1".to_string(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    };
    repo.create(&key1).await.expect("Failed to create key1");

    // 2. Create second key
    let key2 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env.id,
        key_hash: "hash2".to_string(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    };
    repo.create(&key2).await.expect("Failed to create key2");

    // 3. List
    let all = repo
        .list_by_environment(env.id)
        .await
        .expect("Failed to list keys");
    assert_eq!(all.len(), 2);

    // 4. Revoke key1 (should work because key2 is active)
    repo.revoke(key1.id).await.expect("Failed to revoke key1");
    let found1 = repo.find_by_id(key1.id).await.unwrap();
    assert!(!found1.is_active);
    assert!(found1.revoked_at.is_some());

    // 5. Revoke key2 (should FAIL because it's the last active key)
    let err = repo.revoke(key2.id).await.unwrap_err();
    assert!(matches!(
        err,
        stitchd_db::RepositoryError::UniqueViolation { .. }
    ));
}
