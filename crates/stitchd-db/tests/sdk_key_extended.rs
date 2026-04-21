/// Extended integration tests for sdk_key.rs covering additional paths.
use std::sync::Arc;
use stitchd_core::{
    id::{EnvironmentId, OrganisationId, ProjectId, SdkKeyId},
    tenant::{Environment, Organisation, Project, SdkKey},
};
use stitchd_db::{
    EnvironmentRepository, OrganisationRepository, ProjectRepository, RepositoryError,
    SdkKeyRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgOrganisationRepository, PgProjectRepository,
        PgSdkKeyRepository,
    },
};

async fn setup(pool: &sqlx::PgPool) -> (Arc<PgAuditLogger>, EnvironmentId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "SdkExtOrg".into(),
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
        name: "SdkExtProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "SdkExtEnv".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    (audit, env.id)
}

#[sqlx::test(migrations = "./migrations")]
async fn test_sdk_key_find_by_id_not_found(pool: sqlx::PgPool) {
    let (audit, _) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);
    let err = repo.find_by_id(SdkKeyId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_sdk_key_find_active_by_environment(pool: sqlx::PgPool) {
    let (audit, env_id) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);

    // Create two active keys and one revoked
    let key1 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: "active-hash-1".into(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    };
    let key2 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: "active-hash-2".into(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    };
    let key3 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: "revoked-hash".into(),
        is_active: false,
        created_at: chrono::Utc::now(),
        revoked_at: Some(chrono::Utc::now()),
    };
    repo.create(&key1).await.unwrap();
    repo.create(&key2).await.unwrap();
    repo.create(&key3).await.unwrap();

    let active = repo.find_active_by_environment(env_id).await.unwrap();
    assert_eq!(active.len(), 2);
    assert!(active.iter().all(|k| k.is_active));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_sdk_key_find_active_by_hash(pool: sqlx::PgPool) {
    let (audit, env_id) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);

    let key = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: "find-by-hash".into(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    };
    repo.create(&key).await.unwrap();

    let found = repo.find_active_by_hash("find-by-hash").await.unwrap();
    assert_eq!(found.id, key.id);
}

#[sqlx::test(migrations = "./migrations")]
async fn test_sdk_key_find_active_by_hash_not_found(pool: sqlx::PgPool) {
    let (audit, _) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);
    let err = repo
        .find_active_by_hash("nonexistent-hash")
        .await
        .unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_sdk_key_find_active_by_hash_revoked_not_found(pool: sqlx::PgPool) {
    let (audit, env_id) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);

    let key = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: "revoked-active".into(),
        is_active: false,
        created_at: chrono::Utc::now(),
        revoked_at: Some(chrono::Utc::now()),
    };
    repo.create(&key).await.unwrap();

    let err = repo
        .find_active_by_hash("revoked-active")
        .await
        .unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_sdk_key_revoke_not_found(pool: sqlx::PgPool) {
    let (audit, _) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);
    let err = repo.revoke(SdkKeyId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn test_sdk_key_revoke_already_revoked_is_noop(pool: sqlx::PgPool) {
    let (audit, env_id) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);

    // Create two keys so we can revoke one
    let key1 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: "already-revoked-1".into(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    };
    let key2 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: "already-revoked-2".into(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    };
    repo.create(&key1).await.unwrap();
    repo.create(&key2).await.unwrap();

    // Revoke key1 successfully
    repo.revoke(key1.id).await.unwrap();

    // Revoking key1 again should be a no-op (already inactive)
    repo.revoke(key1.id).await.unwrap();
    let found = repo.find_by_id(key1.id).await.unwrap();
    assert!(!found.is_active);
}
