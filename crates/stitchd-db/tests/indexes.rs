/// Integration tests for Phase 1 PostgreSQL index migrations (db_optim_20260516).
///
/// Each test verifies functional correctness of queries that rely on the new
/// indexes. We do not check EXPLAIN output directly since query plans are
/// environment-specific; instead we verify the correct rows are returned/deleted.
use std::sync::Arc;

use chrono::Utc;
use stitchd_core::{
    context::InferredType,
    flag::{FlagRecord, FlagValueType},
    id::{EnvironmentId, FlagId, FlagKey, OrganisationId, ProjectId, SdkKeyId, SegmentId},
    segment::{Segment, SegmentType},
    tenant::{Environment, Organisation, Project, SdkKey},
};
use stitchd_db::{
    ContextRegistryRepository, EnvironmentRepository, FlagRepository, OrganisationRepository,
    ProjectRepository, SdkKeyRepository, SegmentRepository,
    repository::pg::{
        PgAuditLogger, PgContextRegistryRepository, PgEnvironmentRepository,
        PgOrganisationRepository, PgProjectRepository, PgSdkKeyRepository, PgSegmentRepository,
    },
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn seed_org_project_env(
    pool: &sqlx::PgPool,
) -> (Organisation, Project, Environment) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "Test Org".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
        is_system: false,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "Test Project".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "production".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    (org, project, env)
}

// ---------------------------------------------------------------------------
// Task 1: idx_sdk_keys_key_hash_active — find_active_by_hash correctness
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn sdk_key_hash_index_find_active_returns_matching_key(pool: sqlx::PgPool) {
    let (_, _, env) = seed_org_project_env(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgSdkKeyRepository::new(pool.clone(), audit);

    let key = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env.id,
        key_hash: "unique-hash-abc123".to_string(),
        is_active: true,
        created_at: Utc::now(),
        revoked_at: None,
    };
    repo.create(&key).await.unwrap();

    let found = repo.find_active_by_hash("unique-hash-abc123").await.unwrap();
    assert_eq!(found.id, key.id);
    assert!(found.is_active);
}

#[sqlx::test(migrations = "./migrations")]
async fn sdk_key_hash_index_revoked_key_not_returned(pool: sqlx::PgPool) {
    let (_, _, env) = seed_org_project_env(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgSdkKeyRepository::new(pool.clone(), audit);

    // Need two keys so revocation is allowed
    let key1 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env.id,
        key_hash: "revoked-hash".to_string(),
        is_active: true,
        created_at: Utc::now(),
        revoked_at: None,
    };
    let key2 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env.id,
        key_hash: "active-hash".to_string(),
        is_active: true,
        created_at: Utc::now(),
        revoked_at: None,
    };
    repo.create(&key1).await.unwrap();
    repo.create(&key2).await.unwrap();

    repo.revoke(key1.id).await.unwrap();

    let err = repo.find_active_by_hash("revoked-hash").await.unwrap_err();
    assert!(matches!(err, stitchd_db::RepositoryError::NotFound { .. }));
}

// ---------------------------------------------------------------------------
// Task 2: idx_*_active partial indexes — list queries exclude deleted rows
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn soft_delete_partial_index_flags_excludes_deleted(pool: sqlx::PgPool) {
    let (_, project, _) = seed_org_project_env(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let flag_repo = stitchd_db::repository::pg::PgFlagRepository::new(pool.clone(), audit);

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id: project.id,
        key: FlagKey::new("my-flag").unwrap(),
        value_type: FlagValueType::Bool,
        enabled: true,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
        name: "My Flag".into(),
        description: String::new(),
        default_variant_id: None,
    };
    flag_repo.create(&flag).await.unwrap();

    // Soft-delete the flag
    flag_repo.soft_delete(flag.id).await.unwrap();

    // list_by_project should exclude deleted flags
    let flags = flag_repo.list_by_project(project.id).await.unwrap();
    assert!(
        flags.iter().all(|f| f.id != flag.id),
        "soft-deleted flag must not appear in list_by_project"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn soft_delete_partial_index_segments_excludes_deleted(pool: sqlx::PgPool) {
    let (_, _, env) = seed_org_project_env(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgSegmentRepository::new(pool.clone(), audit);

    let seg = Segment {
        id: SegmentId::new(),
        environment_id: env.id,
        key: "my-segment".to_string(),
        segment_type: SegmentType::Rule,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
        name: "My Segment".into(),
        description: String::new(),
        tags: vec![],
    };
    repo.create(&seg).await.unwrap();
    repo.soft_delete(seg.id).await.unwrap();

    let segs = repo.list_by_environment(env.id).await.unwrap();
    assert!(
        segs.iter().all(|s| s.id != seg.id),
        "soft-deleted segment must not appear in list_by_environment"
    );
}

// ---------------------------------------------------------------------------
// Task 3: idx_segment_list_entries_covering — EXISTS membership correctness
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn segment_list_covering_index_member_found(pool: sqlx::PgPool) {
    let (_, _, env) = seed_org_project_env(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgSegmentRepository::new(pool.clone(), audit);

    let seg = Segment {
        id: SegmentId::new(),
        environment_id: env.id,
        key: "list-seg".to_string(),
        segment_type: SegmentType::List,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
        name: "List Segment".into(),
        description: String::new(),
        tags: vec![],
    };
    repo.create(&seg).await.unwrap();

    repo.set_list_entries(seg.id, "user", &["alice".to_string()], &[])
        .await
        .unwrap();

    let result = repo
        .check_list_membership(env.id, "user", "alice", &["list-seg".to_string()])
        .await
        .unwrap();
    assert_eq!(result.get("list-seg"), Some(&true));
}

#[sqlx::test(migrations = "./migrations")]
async fn segment_list_covering_index_non_member_returns_false(pool: sqlx::PgPool) {
    let (_, _, env) = seed_org_project_env(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgSegmentRepository::new(pool.clone(), audit);

    let seg = Segment {
        id: SegmentId::new(),
        environment_id: env.id,
        key: "excl-seg".to_string(),
        segment_type: SegmentType::List,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
        name: "Exclude Segment".into(),
        description: String::new(),
        tags: vec![],
    };
    repo.create(&seg).await.unwrap();

    // exclude bob
    repo.set_list_entries(
        seg.id,
        "user",
        &["alice".to_string()],
        &["bob".to_string()],
    )
    .await
    .unwrap();

    let result = repo
        .check_list_membership(env.id, "user", "bob", &["excl-seg".to_string()])
        .await
        .unwrap();
    // bob is in include but also in exclude — exclude wins
    assert_eq!(result.get("excl-seg"), Some(&false));
}

// ---------------------------------------------------------------------------
// Task 4: idx_context_*_last_seen — purge deletes correct rows
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn context_registry_last_seen_index_purge_removes_old_types(pool: sqlx::PgPool) {
    let repo = PgContextRegistryRepository::new(pool.clone());
    let env_id = EnvironmentId::new();

    // Recent entry
    repo.upsert_context_type(env_id, "fresh").await.unwrap();

    // Stale entry inserted directly (91 days old)
    sqlx::query(
        "INSERT INTO context_type_registry (env_id, context_type, first_seen_at, last_seen_at)
         VALUES ($1, 'stale', NOW() - INTERVAL '91 days', NOW() - INTERVAL '91 days')",
    )
    .bind(env_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let cutoff = Utc::now() - chrono::Duration::days(90);
    repo.purge_stale(cutoff).await.unwrap();

    let remaining: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM context_type_registry WHERE env_id = $1")
            .bind(env_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining.0, 1, "only the recent 'fresh' entry should survive");
}

#[sqlx::test(migrations = "./migrations")]
async fn context_registry_last_seen_index_purge_removes_old_params(pool: sqlx::PgPool) {
    let repo = PgContextRegistryRepository::new(pool.clone());
    let env_id = EnvironmentId::new();

    // Recent param
    repo.upsert_param(env_id, "user", "plan", InferredType::Str, false)
        .await
        .unwrap();

    // Stale param (91 days old)
    sqlx::query(
        "INSERT INTO context_param_registry
         (env_id, context_type, param_key, inferred_type, is_private, first_seen_at, last_seen_at)
         VALUES ($1, 'user', 'stale_param', 'str', false, NOW() - INTERVAL '91 days', NOW() - INTERVAL '91 days')",
    )
    .bind(env_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let cutoff = Utc::now() - chrono::Duration::days(90);
    repo.purge_stale(cutoff).await.unwrap();

    let remaining: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM context_param_registry WHERE env_id = $1")
            .bind(env_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining.0, 1, "only the recent 'plan' param should survive");
}
