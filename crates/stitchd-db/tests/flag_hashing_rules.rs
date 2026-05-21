use std::sync::Arc;
use stitchd_core::{
    flag::{FlagHashingConfig, FlagRecord, FlagRule, FlagValueType},
    id::{FlagId, FlagKey, OrganisationId, ProjectId, RuleId},
    rule_engine::types::{ConditionExpr, Rule, RuleOutput},
    tenant::{Organisation, Project},
};
use stitchd_db::{
    FlagRepository, OrganisationRepository, ProjectRepository,
    repository::pg::{
        PgAuditLogger, PgFlagRepository, PgOrganisationRepository, PgProjectRepository,
    },
};

#[sqlx::test(migrations = "./migrations")]
async fn test_flag_hashing_and_rules(pool: sqlx::PgPool) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let repo = PgFlagRepository::new(pool, audit);

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

    let flag = FlagRecord {
        id: FlagId::new(),
        project_id: project.id,
        key: FlagKey::new("rule-flag").unwrap(),
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
    repo.create(&flag).await.unwrap();

    // 1. Test Hashing Config
    let config = vec![
        FlagHashingConfig {
            flag_id: flag.id,
            parameter_key: "user_id".to_string(),
            parameter_type: "user".to_string(),
            order: 0,
        },
        FlagHashingConfig {
            flag_id: flag.id,
            parameter_key: "session_id".to_string(),
            parameter_type: "session".to_string(),
            order: 1,
        },
    ];

    repo.upsert_hashing_config(flag.id, &config).await.unwrap();
    let found_config = repo.find_hashing_config(flag.id).await.unwrap();
    assert_eq!(found_config.len(), 2);
    assert_eq!(found_config[0].parameter_key, "user_id");
    assert_eq!(found_config[1].parameter_key, "session_id");

    // 2. Test Rules
    let rules = vec![FlagRule {
        flag_id: flag.id,
        rule_index: 0,
        rule: Rule {
            id: RuleId::new(),
            name: None,
            condition: ConditionExpr::And(vec![]),
            output: RuleOutput::Variant(stitchd_core::id::VariantId::new()),
        },
    }];

    repo.upsert_rules(flag.id, &rules).await.unwrap();
    let found_rules = repo.find_rules(flag.id).await.unwrap();
    assert_eq!(found_rules.len(), 1);
    assert_eq!(found_rules[0].rule_index, 0);
    assert_eq!(found_rules[0].rule.id, rules[0].rule.id);
}
