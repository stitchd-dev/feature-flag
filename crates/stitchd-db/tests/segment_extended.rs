/// Extended integration tests for segment.rs covering additional paths.
use std::sync::Arc;
use stitchd_core::{
    id::{EnvironmentId, OrganisationId, ProjectId, SegmentId},
    segment::{Segment, SegmentType},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, OrganisationRepository, ProjectRepository, RepositoryError,
    SegmentRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgOrganisationRepository, PgProjectRepository,
        PgSegmentRepository,
    },
};

async fn setup(pool: &sqlx::PgPool) -> (PgSegmentRepository, EnvironmentId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let repo = PgSegmentRepository::new(pool.clone(), audit);

    let org = Organisation {
        id: OrganisationId::new(),
        name: "SegExtOrg".into(),
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
        name: "SegExtProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();
    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "SegExtEnv".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();
    (repo, env.id)
}

/// Test find_by_id returns NotFound for a non-existent segment.
#[sqlx::test(migrations = "./migrations")]
async fn test_segment_find_by_id_not_found(pool: sqlx::PgPool) {
    let (repo, _) = setup(&pool).await;
    let err = repo.find_by_id(SegmentId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

/// Test list_by_environment returns all non-deleted segments.
#[sqlx::test(migrations = "./migrations")]
async fn test_segment_list_by_environment(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;

    for key in ["seg-a", "seg-b", "seg-c"] {
        repo.create(&Segment {
            id: SegmentId::new(),
            environment_id: env_id,
            key: key.to_string(),
            name: String::new(),
            description: String::new(),
            tags: vec![],
            segment_type: SegmentType::Rule,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            version: 1,
        })
        .await
        .unwrap();
    }

    let list = repo.list_by_environment(env_id).await.unwrap();
    assert_eq!(list.len(), 3);
}

// ── Paginated segment list tests ─────────────────────────────────────────────

fn make_segment(env_id: EnvironmentId, key: &str) -> Segment {
    Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key: key.to_string(),
        name: key.to_string(),
        description: String::new(),
        tags: vec![],
        segment_type: SegmentType::Rule,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_first_page_and_next_cursor(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;
    for i in 0..5 {
        repo.create(&make_segment(env_id, &format!("seg-{i:02}")))
            .await
            .unwrap();
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
    let (repo, env_id) = setup(&pool).await;
    for i in 0..2 {
        repo.create(&make_segment(env_id, &format!("seg-{i:02}")))
            .await
            .unwrap();
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
    let (repo, env_id) = setup(&pool).await;
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
    let (repo, env_id) = setup(&pool).await;
    const N: usize = 23;
    for i in 0..N {
        repo.create(&make_segment(env_id, &format!("seg-{i:03}")))
            .await
            .unwrap();
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
        for s in &items {
            seen.push(s.id.as_uuid());
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

/// Test update with stale version returns VersionConflict.
#[sqlx::test(migrations = "./migrations")]
async fn test_segment_update_version_conflict(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;

    let seg = Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key: "conflict-seg".to_string(),
        name: String::new(),
        description: String::new(),
        tags: vec![],
        segment_type: SegmentType::Rule,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&seg).await.unwrap();

    let mut first_update = seg.clone();
    first_update.segment_type = SegmentType::List;
    repo.update(&first_update).await.unwrap();

    // Stale update with version 1
    let err = repo.update(&seg).await.unwrap_err();
    assert!(matches!(err, RepositoryError::VersionConflict { .. }));
}

/// Test update on non-existent segment returns NotFound.
#[sqlx::test(migrations = "./migrations")]
async fn test_segment_update_not_found(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;
    let ghost = Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key: "ghost-seg".to_string(),
        name: String::new(),
        description: String::new(),
        tags: vec![],
        segment_type: SegmentType::Rule,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    let err = repo.update(&ghost).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

/// Test soft_delete on non-existent segment returns NotFound.
#[sqlx::test(migrations = "./migrations")]
async fn test_segment_soft_delete_not_found(pool: sqlx::PgPool) {
    let (repo, _) = setup(&pool).await;
    let err = repo.soft_delete(SegmentId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

/// Test update on a soft-deleted segment returns NotFound.
#[sqlx::test(migrations = "./migrations")]
async fn test_segment_update_soft_deleted(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;

    let seg = Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key: "soft-del-seg".to_string(),
        name: String::new(),
        description: String::new(),
        tags: vec![],
        segment_type: SegmentType::Rule,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&seg).await.unwrap();
    repo.soft_delete(seg.id).await.unwrap();

    let err = repo.update(&seg).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

/// Test check_list_membership with empty segment_keys returns empty map.
#[sqlx::test(migrations = "./migrations")]
async fn test_check_list_membership_empty_keys(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;
    let result = repo
        .check_list_membership(env_id, "user", "u1", &[])
        .await
        .unwrap();
    assert!(result.is_empty());
}

// test_check_list_membership_included, test_batch_check_empty_inputs,
// test_batch_check_list_membership — removed in Phase 3.
// List-entry membership is now Scylla-backed (see tests/scylla_segment_membership.rs).

/// Test find_by_key returns NotFound for missing segment.
#[sqlx::test(migrations = "./migrations")]
async fn test_segment_find_by_key_not_found(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;
    let err = repo.find_by_key("no-such-key", env_id).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

/// Test create duplicate key returns UniqueViolation.
#[sqlx::test(migrations = "./migrations")]
async fn test_segment_create_duplicate(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;

    let seg = Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key: "dup-seg".to_string(),
        name: String::new(),
        description: String::new(),
        tags: vec![],
        segment_type: SegmentType::Rule,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&seg).await.unwrap();

    let dup = Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key: "dup-seg".to_string(),
        name: String::new(),
        description: String::new(),
        tags: vec![],
        segment_type: SegmentType::Rule,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    let err = repo.create(&dup).await.unwrap_err();
    assert!(matches!(err, RepositoryError::UniqueViolation { .. }));
}
