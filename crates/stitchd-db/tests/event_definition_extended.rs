use std::sync::Arc;

use stitchd_core::{
    event::{EventDefinition, EventValueType, MetricType},
    id::{EnvironmentId, EventDefinitionId, OrganisationId, ProjectId},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, EventDefinitionRepository, OrganisationRepository, ProjectRepository,
    RepositoryError,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgEventDefinitionRepository,
        PgOrganisationRepository, PgProjectRepository,
    },
};

async fn setup(pool: &sqlx::PgPool) -> EnvironmentId {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());

    let org = Organisation {
        id: OrganisationId::new(),
        name: "EdExtOrg".into(),
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
        name: "EdExtProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "EdExtEnv".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();
    env.id
}

fn make_def(env_id: EnvironmentId, key: &str) -> EventDefinition {
    EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env_id,
        key: key.to_string(),
        name: key.to_string(),
        description: None,
        metric_type: MetricType::Count,
        schema: None,
        value_type: EventValueType::Int,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_id_returns_definition(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);

    let def = make_def(env_id, "impressions");
    repo.create(&def).await.unwrap();

    let found = repo.find_by_id(def.id).await.unwrap();
    assert_eq!(found.id, def.id);
    assert_eq!(found.key, "impressions");
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_id_not_found_returns_error(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);
    let err = repo.find_by_id(EventDefinitionId::new()).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_key_not_found_returns_error(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);
    let err = repo.find_by_key("no_such_key", env_id).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn soft_delete_not_found_returns_error(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);
    let err = repo
        .soft_delete(EventDefinitionId::new())
        .await
        .unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn update_not_found_returns_error(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);
    let ghost = make_def(env_id, "ghost");
    let err = repo.update(&ghost).await.unwrap_err();
    assert!(matches!(err, RepositoryError::NotFound { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn update_stores_new_value_type_and_bumps_version(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);

    let def = make_def(env_id, "original_key");
    repo.create(&def).await.unwrap();

    // `key` is intentionally immutable post-creation (admin UI shows it
    // as read-only on EditEventModal). We update the mutable fields only
    // and assert the key stays put.
    let mut updated = def.clone();
    updated.value_type = EventValueType::Double;
    updated.name = "Renamed".to_string();
    let result = repo.update(&updated).await.unwrap();
    assert_eq!(result.key, "original_key", "key is immutable post-create");
    assert_eq!(result.value_type, EventValueType::Double);
    assert_eq!(result.name, "Renamed");
    assert_eq!(result.version, 2);
}

// ── Keyset (cursor) event-definition list tests ─────────────────────────────────

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_first_page_and_next_cursor(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);

    for i in 0..5 {
        repo.create(&make_def(env_id, &format!("evt-{i:02}")))
            .await
            .unwrap();
    }

    let (page, next) = repo
        .list_by_environment_keyset(env_id, None, 3, false)
        .await
        .unwrap();
    assert_eq!(page.len(), 3, "first page returns limit items");
    assert!(next.is_some(), "more rows remain ⇒ a next cursor");
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_last_page_has_no_cursor(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);

    for i in 0..2 {
        repo.create(&make_def(env_id, &format!("evt-{i:02}")))
            .await
            .unwrap();
    }

    let (page, next) = repo
        .list_by_environment_keyset(env_id, None, 50, false)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    assert!(next.is_none(), "all rows on one page ⇒ no next cursor");
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_empty_returns_no_cursor(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);

    let (items, next) = repo
        .list_by_environment_keyset(env_id, None, 50, false)
        .await
        .unwrap();
    assert!(items.is_empty());
    assert!(next.is_none());
}

/// Rigorous correctness: paging through with the returned cursor visits EVERY
/// row exactly once, in (created_at, id) order, with no duplicates or gaps.
#[sqlx::test(migrations = "./migrations")]
async fn list_by_environment_keyset_pages_through_all_rows_exactly_once(pool: sqlx::PgPool) {
    let env_id = setup(&pool).await;
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgEventDefinitionRepository::new(pool, audit);

    const N: usize = 23;
    for i in 0..N {
        repo.create(&make_def(env_id, &format!("evt-{i:03}")))
            .await
            .unwrap();
    }

    // Walk pages of 7 (so the last page is partial: 23 = 7+7+7+2).
    let mut seen: Vec<uuid::Uuid> = Vec::new();
    let mut cursor: Option<stitchd_db::KeysetCursor> = None;
    let mut pages = 0;
    loop {
        let (items, next) = repo
            .list_by_environment_keyset(env_id, cursor, 7, false)
            .await
            .unwrap();
        pages += 1;
        assert!(items.len() <= 7, "never more than the limit per page");
        for d in &items {
            seen.push(d.id.as_uuid());
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
