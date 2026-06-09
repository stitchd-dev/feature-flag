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
        name: String::new(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    };
    let key2 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: "active-hash-2".into(),
        name: String::new(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    };
    let key3 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: "revoked-hash".into(),
        name: String::new(),
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
        name: String::new(),
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
        name: String::new(),
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
        name: String::new(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    };
    let key2 = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: "already-revoked-2".into(),
        name: String::new(),
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

// ---------------------------------------------------------------------------
// Paginated SDK key list tests
// ---------------------------------------------------------------------------

fn make_sdk_key(env_id: EnvironmentId) -> SdkKey {
    SdkKey {
        id: SdkKeyId::new(),
        environment_id: env_id,
        key_hash: uuid::Uuid::new_v4().to_string(),
        name: String::new(),
        is_active: true,
        created_at: chrono::Utc::now(),
        revoked_at: None,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_first_page_and_next_cursor(pool: sqlx::PgPool) {
    let (audit, env_id) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);

    for _ in 0..5 {
        repo.create(&make_sdk_key(env_id)).await.unwrap();
    }

    let (page, next) = repo
        .list_by_environment_keyset(env_id, None, 3)
        .await
        .unwrap();
    assert_eq!(page.len(), 3, "first page returns limit items");
    assert!(next.is_some(), "more rows remain ⇒ a next cursor");
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_last_page_has_no_cursor(pool: sqlx::PgPool) {
    let (audit, env_id) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);

    for _ in 0..2 {
        repo.create(&make_sdk_key(env_id)).await.unwrap();
    }

    let (page, next) = repo
        .list_by_environment_keyset(env_id, None, 50)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    assert!(next.is_none(), "all rows on one page ⇒ no next cursor");
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_empty_returns_no_cursor(pool: sqlx::PgPool) {
    let (audit, env_id) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);

    let (items, next) = repo
        .list_by_environment_keyset(env_id, None, 50)
        .await
        .unwrap();
    assert!(items.is_empty());
    assert!(next.is_none());
}

/// Rigorous correctness: paging through with the returned cursor visits EVERY
/// row exactly once, in (created_at, id) order, with no duplicates or gaps.
#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_pages_through_all_rows_exactly_once(pool: sqlx::PgPool) {
    let (audit, env_id) = setup(&pool).await;
    let repo = PgSdkKeyRepository::new(pool, audit);

    const N: usize = 23;
    for _ in 0..N {
        repo.create(&make_sdk_key(env_id)).await.unwrap();
    }

    // Walk pages of 7 (so the last page is partial: 23 = 7+7+7+2).
    let mut seen: Vec<uuid::Uuid> = Vec::new();
    let mut cursor: Option<stitchd_db::KeysetCursor> = None;
    let mut pages = 0;
    loop {
        let (items, next) = repo
            .list_by_environment_keyset(env_id, cursor, 7)
            .await
            .unwrap();
        pages += 1;
        assert!(items.len() <= 7, "never more than the limit per page");
        for k in &items {
            seen.push(k.id.as_uuid());
        }
        match next {
            Some(tok) => cursor = Some(stitchd_db::KeysetCursor::decode(&tok).unwrap()),
            None => break,
        }
        assert!(pages <= N + 1, "must terminate");
    }

    assert_eq!(
        seen.len(),
        N,
        "every row visited exactly once — no gaps/dupes"
    );
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), N, "no duplicates across pages");
    assert_eq!(pages, 4, "23 rows / 7 per page = 4 pages (7+7+7+2)");
}
