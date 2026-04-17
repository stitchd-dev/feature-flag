/// Extended integration tests for flag.rs covering code paths not exercised
/// by the basic lifecycle test.
use std::sync::Arc;
use stitchd_core::{
    flag::{FlagRecord, FlagValueType},
    id::{EnvironmentId, FlagId, FlagKey, OrganisationId, ProjectId},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, FlagRepository, OrganisationRepository, ProjectRepository,
    RepositoryError,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgFlagRepository, PgOrganisationRepository,
        PgProjectRepository,
    },
};

async fn setup_org_project(
    pool: &sqlx::PgPool,
    audit: &Arc<PgAuditLogger>,
) -> (OrganisationId, ProjectId) {
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "FlagExtOrg".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "FlagExtProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    (org.id, project.id)
}

/// Test find_by_id for an existing flag.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_find_by_id(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let (_, project_id) = setup_org_project(&pool, &audit).await;
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("find-by-id-flag").unwrap(),
        value_type: FlagValueType::Str,
        enabled: false,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&flag).await.unwrap();

    let found = repo.find_by_id(flag.id).await.unwrap();
    assert_eq!(found.id, flag.id);
    assert_eq!(found.value_type, FlagValueType::Str);
    assert!(!found.enabled);
}

/// Test find_by_id returns NotFound for missing flag.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_find_by_id_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgFlagRepository::new(pool.clone(), audit);
    let err = repo.find_by_id(FlagId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

/// Test list_by_project returns all non-deleted flags.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_list_by_project(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let (_, project_id) = setup_org_project(&pool, &audit).await;
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let flag1 = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("list-flag-1").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    let flag2 = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("list-flag-2").unwrap(),
        value_type: FlagValueType::Int,
        enabled: false,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&flag1).await.unwrap();
    repo.create(&flag2).await.unwrap();

    let all = repo.list_by_project(project_id).await.unwrap();
    assert_eq!(all.len(), 2);
}

/// Test list_by_environment returns flags tied to that environment's project.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_list_by_environment(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let (_, project_id) = setup_org_project(&pool, &audit).await;
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let env = Environment {
        id: EnvironmentId::new(),
        project_id,
        name: "Staging".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("env-list-flag").unwrap(),
        value_type: FlagValueType::Json,
        enabled: true,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&flag).await.unwrap();

    let flags = repo.list_by_environment(env.id).await.unwrap();
    assert_eq!(flags.len(), 1);
    assert_eq!(flags[0].id, flag.id);
}

/// Test update succeeds and increments version.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_update(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let (_, project_id) = setup_org_project(&pool, &audit).await;
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("update-flag").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&flag).await.unwrap();

    let mut to_update = flag.clone();
    to_update.enabled = false;
    let updated = repo.update(&to_update).await.unwrap();
    assert!(!updated.enabled);
    assert_eq!(updated.version, 2);
}

/// Test update with stale version returns VersionConflict.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_update_version_conflict(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let (_, project_id) = setup_org_project(&pool, &audit).await;
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("conflict-flag").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&flag).await.unwrap();

    // Update once to move to version 2
    let mut first = flag.clone();
    first.enabled = false;
    repo.update(&first).await.unwrap();

    // Try to update with old version 1 — should fail with VersionConflict
    let err = repo.update(&flag).await.unwrap_err();
    assert!(matches!(err, RepositoryError::VersionConflict { .. }));
}

/// Test update on a non-existent flag returns NotFound.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_update_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let (_, project_id) = setup_org_project(&pool, &audit).await;
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let ghost = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("ghost-flag").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    let err = repo.update(&ghost).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

/// Test soft_delete removes flag from find_by_id.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_soft_delete(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let (_, project_id) = setup_org_project(&pool, &audit).await;
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("delete-flag").unwrap(),
        value_type: FlagValueType::Double,
        enabled: true,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&flag).await.unwrap();
    repo.soft_delete(flag.id).await.unwrap();

    let err = repo.find_by_id(flag.id).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

/// Test soft_delete on non-existent flag returns NotFound.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_soft_delete_not_found(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgFlagRepository::new(pool.clone(), audit);
    let err = repo.soft_delete(FlagId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

/// Test update on a soft-deleted flag returns NotFound.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_update_soft_deleted(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let (_, project_id) = setup_org_project(&pool, &audit).await;
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("soft-del-update").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&flag).await.unwrap();
    repo.soft_delete(flag.id).await.unwrap();

    let err = repo.update(&flag).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

/// Test create with duplicate key returns UniqueViolation.
#[sqlx::test(migrations = "./migrations")]
async fn test_flag_create_duplicate_key(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let (_, project_id) = setup_org_project(&pool, &audit).await;
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("dup-key").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&flag).await.unwrap();

    // Try to create same key again
    let dup = FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new("dup-key").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    let err = repo.create(&dup).await.unwrap_err();
    assert!(matches!(err, RepositoryError::UniqueViolation { .. }));
}
