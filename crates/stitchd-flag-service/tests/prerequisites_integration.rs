//! Integration tests for the prerequisite RPCs + flag delete-block guard
//! (`flag_lifecycle_20260604` Phase 4 Tasks 2 + 3).
//!
//! These exercise the real `FlagServiceImpl` wired to Postgres-backed
//! repositories (including the `PrerequisiteRepository`) via `#[sqlx::test]`,
//! which provisions a fresh isolated database and runs every migration. Only
//! the flag / variant / prerequisite paths are touched — segments and SDK keys
//! are present but unused by these RPCs.

use std::sync::Arc;

use sqlx::PgPool;
use stitchd_flag_service::prerequisites::DEPENDENCY_EXISTS_STATUS_PREFIX;
use stitchd_flag_service::service::FlagServiceImpl;
use stitchd_proto::flags::v1::flag_service_server::FlagService;
use stitchd_proto::flags::v1::{
    FlagPrerequisite, GetPrerequisitesRequest, MutateFlagRequest, MutationKind,
    SetPrerequisitesRequest,
};
use tonic::Request;
use uuid::Uuid;

// ── Test fixtures ───────────────────────────────────────────────────────────

async fn seed_org_project(pool: &PgPool) -> Uuid {
    let org_id: Uuid =
        sqlx::query_scalar("INSERT INTO organisations (name) VALUES ('org') RETURNING id")
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query_scalar(
        "INSERT INTO projects (organisation_id, name) VALUES ($1, 'proj') RETURNING id",
    )
    .bind(org_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_flag(pool: &PgPool, project_id: Uuid, key: &str) -> Uuid {
    sqlx::query_scalar(
        r"INSERT INTO feature_flags (project_id, key, name, enabled, value_type)
          VALUES ($1, $2, $3, true, 'bool') RETURNING id",
    )
    .bind(project_id)
    .bind(key)
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_variant(pool: &PgPool, flag_id: Uuid, key: &str) -> Uuid {
    sqlx::query_scalar(
        r"INSERT INTO variants (flag_id, key, value) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(flag_id)
    .bind(key)
    .bind(serde_json::json!(true))
    .fetch_one(pool)
    .await
    .unwrap()
}

fn build_service(pool: PgPool) -> FlagServiceImpl {
    let audit = Arc::new(stitchd_db::repository::pg::PgAuditLogger::new(pool.clone()));
    let flag_repo: Arc<dyn stitchd_db::FlagRepository> = Arc::new(
        stitchd_db::repository::pg::PgFlagRepository::new(pool.clone(), audit.clone()),
    );
    let variant_repo: Arc<dyn stitchd_db::VariantRepository> = Arc::new(
        stitchd_db::repository::pg::PgVariantRepository::new(pool.clone(), audit.clone()),
    );
    let sdk_key_repo: Arc<dyn stitchd_db::SdkKeyRepository> = Arc::new(
        stitchd_db::repository::pg::PgSdkKeyRepository::new(pool.clone(), audit.clone()),
    );
    let segment_repo: Arc<dyn stitchd_db::SegmentRepository> = Arc::new(
        stitchd_db::repository::pg::PgSegmentRepository::new(pool.clone(), audit.clone()),
    );
    let prereq_repo = stitchd_db::PrerequisiteRepository::new(pool);
    FlagServiceImpl::new(flag_repo, variant_repo, sdk_key_repo, segment_repo)
        .with_prerequisite_repo(prereq_repo)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn set_then_get_prerequisites_roundtrips(pool: PgPool) {
    let project_id = seed_org_project(&pool).await;
    let dependent = seed_flag(&pool, project_id, "dependent").await;
    let _ = dependent;
    let prereq = seed_flag(&pool, project_id, "prereq").await;
    let variant = seed_variant(&pool, prereq, "on").await;
    let svc = build_service(pool);

    let resp = svc
        .set_prerequisites(Request::new(SetPrerequisitesRequest {
            project_id: project_id.to_string(),
            flag_key: "dependent".to_string(),
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq.to_string(),
                prerequisite_flag_key: String::new(),
                required_variant_id: variant.to_string(),
                required_variant_key: String::new(),
            }],
            fallback_variant_key: String::new(),
            version: 0,
        }))
        .await
        .expect("set_prerequisites");
    let flag = resp.into_inner().flag.expect("flag");
    assert_eq!(flag.prerequisites.len(), 1);
    assert_eq!(
        flag.prerequisites[0].prerequisite_flag_id,
        prereq.to_string()
    );
    // The response carries resolved keys too (snapshot population).
    assert_eq!(flag.prerequisites[0].prerequisite_flag_key, "prereq");
    assert_eq!(flag.prerequisites[0].required_variant_key, "on");

    let got = svc
        .get_prerequisites(Request::new(GetPrerequisitesRequest {
            project_id: project_id.to_string(),
            flag_key: "dependent".to_string(),
        }))
        .await
        .expect("get_prerequisites")
        .into_inner();
    assert_eq!(got.prerequisites.len(), 1);
    assert_eq!(
        got.prerequisites[0].prerequisite_flag_id,
        prereq.to_string()
    );
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn set_prerequisites_with_fallback_variant(pool: PgPool) {
    let project_id = seed_org_project(&pool).await;
    let dependent = seed_flag(&pool, project_id, "dependent").await;
    let fallback = seed_variant(&pool, dependent, "off").await;
    let _ = fallback;
    let prereq = seed_flag(&pool, project_id, "prereq").await;
    let variant = seed_variant(&pool, prereq, "on").await;
    let svc = build_service(pool);

    let resp = svc
        .set_prerequisites(Request::new(SetPrerequisitesRequest {
            project_id: project_id.to_string(),
            flag_key: "dependent".to_string(),
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq.to_string(),
                prerequisite_flag_key: String::new(),
                required_variant_id: variant.to_string(),
                required_variant_key: String::new(),
            }],
            fallback_variant_key: "off".to_string(),
            version: 0,
        }))
        .await
        .expect("set_prerequisites")
        .into_inner();
    assert_eq!(resp.flag.unwrap().fallback_variant_key, "off");
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn set_prerequisites_rejects_cycle(pool: PgPool) {
    let project_id = seed_org_project(&pool).await;
    let a = seed_flag(&pool, project_id, "a").await;
    let b = seed_flag(&pool, project_id, "b").await;
    let va = seed_variant(&pool, a, "on").await;
    let vb = seed_variant(&pool, b, "on").await;
    let svc = build_service(pool);

    // a depends on b — fine.
    svc.set_prerequisites(Request::new(SetPrerequisitesRequest {
        project_id: project_id.to_string(),
        flag_key: "a".to_string(),
        prerequisites: vec![FlagPrerequisite {
            prerequisite_flag_id: b.to_string(),
            prerequisite_flag_key: String::new(),
            required_variant_id: vb.to_string(),
            required_variant_key: String::new(),
        }],
        fallback_variant_key: String::new(),
        version: 0,
    }))
    .await
    .expect("a -> b ok");

    // Now b depends on a — would close the cycle. Must be rejected.
    let err = svc
        .set_prerequisites(Request::new(SetPrerequisitesRequest {
            project_id: project_id.to_string(),
            flag_key: "b".to_string(),
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: a.to_string(),
                prerequisite_flag_key: String::new(),
                required_variant_id: va.to_string(),
                required_variant_key: String::new(),
            }],
            fallback_variant_key: String::new(),
            version: 0,
        }))
        .await
        .expect_err("b -> a must be rejected (cycle)");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert!(
        err.message().contains("cycle"),
        "expected cycle message, got: {}",
        err.message()
    );
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn set_prerequisites_rejects_self_prerequisite(pool: PgPool) {
    let project_id = seed_org_project(&pool).await;
    let a = seed_flag(&pool, project_id, "a").await;
    let va = seed_variant(&pool, a, "on").await;
    let svc = build_service(pool);

    let err = svc
        .set_prerequisites(Request::new(SetPrerequisitesRequest {
            project_id: project_id.to_string(),
            flag_key: "a".to_string(),
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: a.to_string(),
                prerequisite_flag_key: String::new(),
                required_variant_id: va.to_string(),
                required_variant_key: String::new(),
            }],
            fallback_variant_key: String::new(),
            version: 0,
        }))
        .await
        .expect_err("self-prerequisite rejected");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn set_prerequisites_rejects_unknown_variant(pool: PgPool) {
    let project_id = seed_org_project(&pool).await;
    let dependent = seed_flag(&pool, project_id, "dependent").await;
    let _ = dependent;
    let prereq = seed_flag(&pool, project_id, "prereq").await;
    let svc = build_service(pool);

    let err = svc
        .set_prerequisites(Request::new(SetPrerequisitesRequest {
            project_id: project_id.to_string(),
            flag_key: "dependent".to_string(),
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: prereq.to_string(),
                prerequisite_flag_key: String::new(),
                required_variant_id: Uuid::new_v4().to_string(),
                required_variant_key: String::new(),
            }],
            fallback_variant_key: String::new(),
            version: 0,
        }))
        .await
        .expect_err("unknown variant rejected");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn delete_blocked_while_referenced_then_allowed_after_removal(pool: PgPool) {
    let project_id = seed_org_project(&pool).await;
    let dependent = seed_flag(&pool, project_id, "dependent").await;
    let prereq = seed_flag(&pool, project_id, "prereq").await;
    let variant = seed_variant(&pool, prereq, "on").await;
    let svc = build_service(pool);

    // dependent → prereq.
    svc.set_prerequisites(Request::new(SetPrerequisitesRequest {
        project_id: project_id.to_string(),
        flag_key: "dependent".to_string(),
        prerequisites: vec![FlagPrerequisite {
            prerequisite_flag_id: prereq.to_string(),
            prerequisite_flag_key: String::new(),
            required_variant_id: variant.to_string(),
            required_variant_key: String::new(),
        }],
        fallback_variant_key: String::new(),
        version: 0,
    }))
    .await
    .expect("set prereq");

    // Deleting `prereq` must be blocked with the dependency_exists sentinel.
    let delete_req = |key: &str| {
        Request::new(MutateFlagRequest {
            project_id: project_id.to_string(),
            environment_id: String::new(),
            kind: MutationKind::Delete as i32,
            version: 0,
            enabled_override: None,
            flag: Some(stitchd_proto::flags::v1::FeatureFlag {
                key: key.to_string(),
                ..Default::default()
            }),
        })
    };

    let err = svc
        .mutate_flag(delete_req("prereq"))
        .await
        .expect_err("delete of referenced flag must be blocked");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert!(
        err.message().starts_with(DEPENDENCY_EXISTS_STATUS_PREFIX),
        "missing sentinel prefix: {}",
        err.message()
    );
    assert!(
        err.message().contains(&dependent.to_string()),
        "sentinel must name the blocking dependent: {}",
        err.message()
    );

    // Remove the reference, then the delete is allowed.
    svc.set_prerequisites(Request::new(SetPrerequisitesRequest {
        project_id: project_id.to_string(),
        flag_key: "dependent".to_string(),
        prerequisites: vec![],
        fallback_variant_key: String::new(),
        version: 0,
    }))
    .await
    .expect("clear prereq");

    svc.mutate_flag(delete_req("prereq"))
        .await
        .expect("delete now allowed after reference removed");
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn preview_returns_fallback_when_prerequisite_unmet(pool: PgPool) {
    use stitchd_proto::flags::v1::EvaluatePreviewRequest;

    let project_id = seed_org_project(&pool).await;
    let dependent = seed_flag(&pool, project_id, "dependent").await;
    let main_variant = seed_variant(&pool, dependent, "main").await;
    let _fallback = seed_variant(&pool, dependent, "fb").await;
    // Default the dependent flag to "main" so we can prove the gate overrides it.
    sqlx::query("UPDATE feature_flags SET default_variant_id = $2 WHERE id = $1")
        .bind(dependent)
        .bind(main_variant)
        .execute(&pool)
        .await
        .unwrap();

    // The prerequisite flag is DISABLED → it resolves to no variant → the gate
    // is unmet → the dependent flag returns its fallback "fb".
    let prereq = seed_flag(&pool, project_id, "prereq").await;
    sqlx::query("UPDATE feature_flags SET enabled = false WHERE id = $1")
        .bind(prereq)
        .execute(&pool)
        .await
        .unwrap();
    let prereq_on = seed_variant(&pool, prereq, "on").await;

    let svc = build_service(pool);

    svc.set_prerequisites(Request::new(SetPrerequisitesRequest {
        project_id: project_id.to_string(),
        flag_key: "dependent".to_string(),
        prerequisites: vec![FlagPrerequisite {
            prerequisite_flag_id: prereq.to_string(),
            prerequisite_flag_key: String::new(),
            required_variant_id: prereq_on.to_string(),
            required_variant_key: String::new(),
        }],
        fallback_variant_key: "fb".to_string(),
        version: 0,
    }))
    .await
    .expect("set prereq with fallback");

    let contexts_json = serde_json::to_string(&serde_json::json!([
        { "contexts": [ {
            "context_type": "user", "key": "u1",
            "parameters": {}, "private_parameters": []
        } ] }
    ]))
    .unwrap();

    let resp = svc
        .evaluate_preview(Request::new(EvaluatePreviewRequest {
            project_id: project_id.to_string(),
            flag_key: "dependent".to_string(),
            contexts_json,
            environment_id: String::new(),
        }))
        .await
        .expect("evaluate_preview")
        .into_inner();

    let results: Vec<serde_json::Value> = serde_json::from_str(&resp.results_json).unwrap();
    assert_eq!(results.len(), 1);
    // The gate fired → fallback variant "fb", not the default "main".
    assert_eq!(results[0]["variant_key"], "fb");
    // The trace names the failing prerequisite flag.
    let pf = &results[0]["prerequisite_failure"];
    assert!(
        !pf.is_null(),
        "preview must surface the prerequisite_failure trace"
    );
    assert_eq!(pf["prerequisite_flag_id"], prereq.to_string());
}
