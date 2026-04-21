use std::sync::Arc;
use stitchd_core::{id::OrganisationId, tenant::Organisation};
use stitchd_db::{
    OrganisationRepository,
    repository::pg::{PgAuditLogger, PgOrganisationRepository},
};

#[sqlx::test(migrations = "./migrations")]
async fn test_organisation_lifecycle(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = PgOrganisationRepository::new(pool.clone(), audit);

    // 1. Create
    let org = Organisation {
        id: OrganisationId::new(),
        name: "Test Org".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: 1,
        is_system: false,
};

    repo.create(&org)
        .await
        .expect("Failed to create organisation");

    // 2. Find
    let found = repo
        .find_by_id(org.id)
        .await
        .expect("Failed to find organisation");
    assert_eq!(found.name, "Test Org");
    assert_eq!(found.version, 1);

    // 3. Update (Correct Version)
    let mut to_update = found.clone();
    to_update.name = "Updated Org".to_string();
    let updated = repo
        .update(&to_update)
        .await
        .expect("Failed to update organisation");
    assert_eq!(updated.name, "Updated Org");
    assert_eq!(updated.version, 2);

    // 4. Update (Stale Version -> Conflict)
    let stale_err = repo.update(&found).await.unwrap_err();
    assert!(matches!(
        stale_err,
        stitchd_db::RepositoryError::VersionConflict { .. }
    ));

    // 5. List
    let all = repo.list_all().await.expect("Failed to list organisations");
    assert!(
        all.iter()
            .any(|o| o.id == org.id && o.name == "Updated Org")
    );

    // 6. Soft Delete
    repo.soft_delete(org.id)
        .await
        .expect("Failed to soft delete");

    // 7. Find (Should be NotFound because it's soft-deleted)
    let not_found = repo.find_by_id(org.id).await.unwrap_err();
    assert!(matches!(
        not_found,
        stitchd_db::RepositoryError::NotFound { .. }
    ));

    // 8. List (Should be absent from active list)
    let all_after = repo.list_all().await.expect("Failed to list organisations");
    assert!(!all_after.iter().any(|o| o.id == org.id));
}
