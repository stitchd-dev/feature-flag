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
async fn list_by_environment_paginated_page_1_returns_first_slice(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;
    for i in 0..5 {
        repo.create(&make_segment(env_id, &format!("seg-{i:02}"))).await.unwrap();
    }
    let (page, total) = repo.list_by_environment_paginated(env_id, 0, 3).await.unwrap();
    assert_eq!(total, 5);
    assert_eq!(page.len(), 3);
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_paginated_page_2_returns_remainder(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;
    for i in 0..5 {
        repo.create(&make_segment(env_id, &format!("seg-{i:02}"))).await.unwrap();
    }
    let (page, total) = repo.list_by_environment_paginated(env_id, 3, 3).await.unwrap();
    assert_eq!(total, 5);
    assert_eq!(page.len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_paginated_total_count_accurate(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;
    for i in 0..10 {
        repo.create(&make_segment(env_id, &format!("seg-{i:02}"))).await.unwrap();
    }
    let (_items, total) = repo.list_by_environment_paginated(env_id, 0, 1).await.unwrap();
    assert_eq!(total, 10);
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_paginated_empty_returns_zero_total(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;
    let (items, total) = repo.list_by_environment_paginated(env_id, 0, 50).await.unwrap();
    assert_eq!(total, 0);
    assert!(items.is_empty());
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

/// Test check_list_membership with known include/exclude entries.
#[sqlx::test(migrations = "./migrations")]
async fn test_check_list_membership_included(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;

    let seg = Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key: "membership-seg".to_string(),
        name: String::new(),
        description: String::new(),
        tags: vec![],
        segment_type: SegmentType::List,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&seg).await.unwrap();

    repo.set_list_entries(
        seg.id,
        "user",
        &["alice".to_string(), "bob".to_string()],
        &["charlie".to_string()],
    )
    .await
    .unwrap();

    let result = repo
        .check_list_membership(env_id, "user", "alice", &["membership-seg".to_string()])
        .await
        .unwrap();
    assert!(*result.get("membership-seg").unwrap());

    // charlie is excluded
    let result_charlie = repo
        .check_list_membership(env_id, "user", "charlie", &["membership-seg".to_string()])
        .await
        .unwrap();
    assert!(!(*result_charlie.get("membership-seg").unwrap()));

    // dave is not in either list
    let result_dave = repo
        .check_list_membership(env_id, "user", "dave", &["membership-seg".to_string()])
        .await
        .unwrap();
    assert!(!(*result_dave.get("membership-seg").unwrap()));
}

/// Test batch_check_list_membership with empty inputs returns empty.
#[sqlx::test(migrations = "./migrations")]
async fn test_batch_check_empty_inputs(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;

    // Empty contexts
    let r1 = repo
        .batch_check_list_membership(env_id, &[], &["seg".to_string()])
        .await
        .unwrap();
    assert!(r1.is_empty());

    // Empty segment_keys
    let r2 = repo
        .batch_check_list_membership(env_id, &[("user".to_string(), "alice".to_string())], &[])
        .await
        .unwrap();
    assert!(r2.is_empty());
}

/// Test batch_check_list_membership returns results for multiple contexts.
#[sqlx::test(migrations = "./migrations")]
async fn test_batch_check_list_membership(pool: sqlx::PgPool) {
    let (repo, env_id) = setup(&pool).await;

    let seg = Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key: "batch-seg".to_string(),
        name: String::new(),
        description: String::new(),
        tags: vec![],
        segment_type: SegmentType::List,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&seg).await.unwrap();

    repo.set_list_entries(seg.id, "user", &["alice".to_string()], &[])
        .await
        .unwrap();

    let contexts = vec![
        ("user".to_string(), "alice".to_string()),
        ("user".to_string(), "bob".to_string()),
    ];

    let results = repo
        .batch_check_list_membership(env_id, &contexts, &["batch-seg".to_string()])
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    let alice = results.iter().find(|r| r.context_key == "alice").unwrap();
    assert!(*alice.memberships.get("batch-seg").unwrap());
    let bob = results.iter().find(|r| r.context_key == "bob").unwrap();
    assert!(!(*bob.memberships.get("batch-seg").unwrap()));
}

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
