//! Phase 6 Task 1 — segment delete/archive referential-integrity block.
//!
//! Verifies the authoritative reference scan (`dependents_of_segment`) AND the
//! service-level `DeleteAdminSegment` guard:
//!   * a segment referenced by a flag rule (`InSegment`) is blocked from delete
//!     with the `dependency_exists:` sentinel listing the blocking flag id;
//!   * a segment referenced by ANOTHER segment's condition is likewise blocked;
//!   * once the reference is removed the delete succeeds.
//!
//! New-table-style access → runtime `sqlx::query` (no compile-time macros);
//! `#[sqlx::test]` provisions a fresh migrated DB per test.

use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;

use stitchd_core::id::SegmentId;
use stitchd_core::rule_engine::condition::Condition;
use stitchd_core::rule_engine::types::ConditionExpr;
use stitchd_db::{PgAuditLogger, PgSegmentRepository};
use stitchd_segmentation_service::dependency_scan::{
    DEPENDENCY_EXISTS_STATUS_PREFIX, dependents_of_segment,
};
use stitchd_segmentation_service::grpc::service::{AppState, SegmentationServiceImpl};

use stitchd_proto::segments::v1::{
    DeleteAdminSegmentRequest, segmentation_service_server::SegmentationService,
};

/// Seed org → project → env, returning `(project_id, env_id)`.
async fn seed_scope(pool: &PgPool) -> (Uuid, Uuid) {
    let org = Uuid::new_v4();
    let project = Uuid::new_v4();
    let env = Uuid::new_v4();
    sqlx::query("INSERT INTO organisations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(format!("org-{org}"))
        .execute(pool)
        .await
        .expect("seed org");
    sqlx::query("INSERT INTO projects (id, organisation_id, name) VALUES ($1, $2, $3)")
        .bind(project)
        .bind(org)
        .bind(format!("proj-{project}"))
        .execute(pool)
        .await
        .expect("seed project");
    sqlx::query("INSERT INTO environments (id, project_id, name) VALUES ($1, $2, $3)")
        .bind(env)
        .bind(project)
        .bind("dev")
        .execute(pool)
        .await
        .expect("seed env");
    (project, env)
}

/// Insert a rule-based segment with the given key + condition expression.
async fn seed_segment(pool: &PgPool, env: Uuid, key: &str, cond: Option<&ConditionExpr>) -> Uuid {
    let id = Uuid::new_v4();
    let cond_json = cond.map(|c| serde_json::to_value(c).unwrap());
    sqlx::query(
        "INSERT INTO segments (id, environment_id, key, segment_type, condition_expr) \
         VALUES ($1, $2, $3, 'rule', $4)",
    )
    .bind(id)
    .bind(env)
    .bind(key)
    .bind(cond_json)
    .execute(pool)
    .await
    .expect("seed segment");
    id
}

/// Insert a flag + a single rule whose condition references `segment`.
async fn seed_flag_referencing_segment(pool: &PgPool, project: Uuid, segment: SegmentId) -> Uuid {
    let flag = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO feature_flags (id, project_id, key, value_type, enabled) \
         VALUES ($1, $2, $3, 'bool', true)",
    )
    .bind(flag)
    .bind(project)
    .bind(format!("flag-{flag}"))
    .execute(pool)
    .await
    .expect("seed flag");

    let rule_def =
        serde_json::to_value(ConditionExpr::Leaf(Condition::InSegment(segment))).unwrap();
    sqlx::query(
        "INSERT INTO feature_flag_rules (id, flag_id, rule_index, rule_def) \
         VALUES ($1, $2, 0, $3)",
    )
    .bind(Uuid::new_v4())
    .bind(flag)
    .bind(rule_def)
    .execute(pool)
    .await
    .expect("seed flag rule");
    flag
}

fn service(pool: PgPool) -> SegmentationServiceImpl {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let repo = Arc::new(PgSegmentRepository::new(pool.clone(), audit));
    SegmentationServiceImpl::new(AppState::new(repo).with_dependency_pool(pool))
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn flag_rule_reference_blocks_segment_delete(pool: PgPool) {
    let (project, env) = seed_scope(&pool).await;
    let seg = seed_segment(&pool, env, "target-seg", None).await;
    let seg_id = SegmentId::from_uuid(seg);
    let flag = seed_flag_referencing_segment(&pool, project, seg_id).await;

    // Authoritative scan finds the flag.
    let deps = dependents_of_segment(&pool, seg_id).await.expect("scan");
    assert_eq!(deps.flag_ids, vec![flag]);
    assert!(deps.segment_ids.is_empty());

    // Service-level delete is blocked with the sentinel listing the flag id.
    let svc = service(pool.clone());
    let err = svc
        .delete_admin_segment(tonic::Request::new(DeleteAdminSegmentRequest {
            segment_id: seg.to_string(),
            org_id: String::new(),
        }))
        .await
        .expect_err("delete must be blocked");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert!(err.message().starts_with(DEPENDENCY_EXISTS_STATUS_PREFIX));
    assert!(err.message().contains(&flag.to_string()));
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn segment_reference_blocks_segment_delete(pool: PgPool) {
    let (_project, env) = seed_scope(&pool).await;
    let target = seed_segment(&pool, env, "target-seg", None).await;
    let target_id = SegmentId::from_uuid(target);
    // Another segment nests the target via NotInSegment.
    let nesting_cond = ConditionExpr::Leaf(Condition::NotInSegment(target_id));
    let nesting = seed_segment(&pool, env, "nesting-seg", Some(&nesting_cond)).await;

    let deps = dependents_of_segment(&pool, target_id).await.expect("scan");
    assert!(deps.flag_ids.is_empty());
    assert_eq!(deps.segment_ids, vec![nesting]);

    let svc = service(pool.clone());
    let err = svc
        .delete_admin_segment(tonic::Request::new(DeleteAdminSegmentRequest {
            segment_id: target.to_string(),
            org_id: String::new(),
        }))
        .await
        .expect_err("delete must be blocked");
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert!(err.message().contains(&nesting.to_string()));
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn delete_succeeds_after_reference_removed(pool: PgPool) {
    let (project, env) = seed_scope(&pool).await;
    let seg = seed_segment(&pool, env, "target-seg", None).await;
    let seg_id = SegmentId::from_uuid(seg);
    let _flag = seed_flag_referencing_segment(&pool, project, seg_id).await;

    let svc = service(pool.clone());
    // Blocked while referenced.
    svc.delete_admin_segment(tonic::Request::new(DeleteAdminSegmentRequest {
        segment_id: seg.to_string(),
        org_id: String::new(),
    }))
    .await
    .expect_err("delete must be blocked while referenced");

    // Remove the referencing rule.
    sqlx::query("DELETE FROM feature_flag_rules WHERE rule_def::text LIKE $1")
        .bind(format!("%{seg}%"))
        .execute(&pool)
        .await
        .expect("remove reference");

    // Scan now empty → delete succeeds.
    let deps = dependents_of_segment(&pool, seg_id).await.expect("scan");
    assert!(deps.is_empty());
    svc.delete_admin_segment(tonic::Request::new(DeleteAdminSegmentRequest {
        segment_id: seg.to_string(),
        org_id: String::new(),
    }))
    .await
    .expect("delete must succeed once unreferenced");
}
