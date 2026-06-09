use std::sync::Arc;
use stitchd_core::{
    flag::{FlagRecord, FlagValueType, Variant, VariantValue},
    id::{FlagId, FlagKey, OrganisationId, ProjectId},
    tenant::{Organisation, Project},
};
use stitchd_db::{
    FlagRepository, OrganisationRepository, ProjectRepository, VariantRepository,
    repository::pg::{
        PgAuditLogger, PgFlagRepository, PgOrganisationRepository, PgProjectRepository,
        PgVariantRepository,
    },
};

fn make_flag(project_id: ProjectId, key: &str) -> FlagRecord {
    FlagRecord {
        id: FlagId::new(),
        project_id,
        key: FlagKey::new(key).unwrap(),
        name: key.to_string(),
        description: String::new(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        default_rule_distribution: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    }
}

async fn setup_org_and_project(
    org_repo: &PgOrganisationRepository,
    proj_repo: &PgProjectRepository,
) -> Project {
    let org = Organisation {
        id: OrganisationId::new(),
        name: "TestOrg".into(),
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
        name: "TestProj".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();
    project
}

#[sqlx::test(migrations = "./migrations")]
async fn test_flag_lifecycle(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let repo = PgFlagRepository::new(pool.clone(), audit.clone());
    let var_repo = PgVariantRepository::new(pool.clone(), audit);

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

    // 1. Create Flag
    let flag = FlagRecord {
        id: FlagId::new(),
        project_id: project.id,
        key: FlagKey::new("test-flag").unwrap(),
        name: String::new(),
        description: String::new(),
        value_type: FlagValueType::Bool,
        enabled: true,
        default_variant_id: None,
        default_rule_distribution: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
    };
    repo.create(&flag).await.expect("Failed to create flag");

    // 2. Create Variant
    let variant = Variant {
        id: stitchd_core::id::VariantId::new(),
        key: "on".to_string(),
        value: VariantValue::BoolValue(true),
    };
    var_repo
        .create(flag.id, &variant)
        .await
        .expect("Failed to create variant");

    // 3. Find Flag by Key
    let found = repo
        .find_by_key(&flag.key, project.id)
        .await
        .expect("Failed to find flag");
    assert_eq!(found.id, flag.id);

    // 4. Find Variants
    let variants = var_repo
        .find_by_flag(flag.id)
        .await
        .expect("Failed to find variants");
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0].key, "on");

    // 5. Update Variant
    let mut to_update = variants[0].clone();
    to_update.key = "true".to_string();
    var_repo
        .update(&to_update)
        .await
        .expect("Failed to update variant");

    // 6. Delete Variant
    var_repo
        .delete(to_update.id)
        .await
        .expect("Failed to delete variant");
    let after_delete = var_repo.find_by_flag(flag.id).await.unwrap();
    assert_eq!(after_delete.len(), 0);
}

// ── Keyset (cursor) flag list tests ────────────────────────────────────────────

#[sqlx::test(migrations = "./migrations")]
async fn list_by_project_keyset_first_page_and_next_cursor(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let project = setup_org_and_project(&org_repo, &proj_repo).await;
    for i in 0..5 {
        repo.create(&make_flag(project.id, &format!("flag-{i:02}")))
            .await
            .unwrap();
    }

    // First page of 3 of 5 → 3 items + a next cursor.
    let (page1, next) = repo
        .list_by_project_keyset(project.id, None, 3)
        .await
        .unwrap();
    assert_eq!(page1.len(), 3, "first page returns limit items");
    assert!(next.is_some(), "more rows remain ⇒ a next cursor");
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_project_keyset_last_page_has_no_cursor(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let project = setup_org_and_project(&org_repo, &proj_repo).await;
    for i in 0..2 {
        repo.create(&make_flag(project.id, &format!("flag-{i:02}")))
            .await
            .unwrap();
    }

    // 2 of 2 fit in one page → no next cursor.
    let (page, next) = repo
        .list_by_project_keyset(project.id, None, 50)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    assert!(next.is_none(), "all rows on one page ⇒ no next cursor");
}

#[sqlx::test(migrations = "./migrations")]
async fn list_by_project_keyset_empty_project(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let project = setup_org_and_project(&org_repo, &proj_repo).await;
    let (items, next) = repo
        .list_by_project_keyset(project.id, None, 50)
        .await
        .unwrap();
    assert!(items.is_empty());
    assert!(next.is_none());
}

/// Rigorous correctness: paging through with the returned cursor visits EVERY
/// row exactly once, in (created_at, id) order, with no duplicates or gaps.
#[sqlx::test(migrations = "./migrations")]
async fn list_by_project_keyset_pages_through_all_rows_exactly_once(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let repo = PgFlagRepository::new(pool.clone(), audit);

    let project = setup_org_and_project(&org_repo, &proj_repo).await;
    const N: usize = 23;
    for i in 0..N {
        repo.create(&make_flag(project.id, &format!("flag-{i:03}")))
            .await
            .unwrap();
    }

    // Walk pages of 7 (so the last page is partial: 23 = 7+7+7+2).
    let mut seen: Vec<uuid::Uuid> = Vec::new();
    let mut cursor: Option<stitchd_db::KeysetCursor> = None;
    let mut pages = 0;
    loop {
        let (items, next) = repo
            .list_by_project_keyset(project.id, cursor, 7)
            .await
            .unwrap();
        pages += 1;
        assert!(items.len() <= 7, "never more than the limit per page");
        for f in &items {
            seen.push(f.id.as_uuid());
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
