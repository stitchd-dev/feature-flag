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
