//! Integration tests for `SdkClient` against a live server + DB.
//!
//! Each test spins up a real tonic gRPC server and axum HTTP server bound to
//! random ports, then exercises the SDK end-to-end.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use stitchd_core::{
    context::Context,
    flag::{FlagRecord, FlagRule, FlagValueType},
    id::{EnvironmentId, FlagId, FlagKey, OrganisationId, ProjectId, RuleId, SdkKeyId, SegmentId, VariantId},
    rule_engine::{
        condition::Condition,
        types::{ConditionExpr, Rule, RuleOutput},
    },
    segment::{Segment, SegmentType},
    tenant::{Environment, Organisation, Project, SdkKey},
    variants::{Variant, VariantValue},
};
use stitchd_db::{
    EnvironmentRepository, FlagRepository, OrganisationRepository, ProjectRepository,
    SdkKeyRepository, SegmentRepository, VariantRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgFlagRepository, PgOrganisationRepository,
        PgProjectRepository, PgSdkKeyRepository, PgSegmentRepository, PgVariantRepository,
    },
};
use stitchd_proto::flags::v1::flag_sync_service_server::FlagSyncServiceServer;
use stitchd_sdk::{EvaluationContext, SdkClient, SdkConfig, VariantValue as SdkVariantValue};
use stitchd_server::{AppState, build_router, grpc::flag_sync::FlagSyncServiceImpl};

const SDK_KEY: &str = "sdk-integration-test-key";

/// Build test fixtures and start both gRPC and HTTP servers.
/// Returns (SdkConfig, env_id).
async fn setup(pool: sqlx::PgPool) -> (SdkConfig, EnvironmentId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let flag_repo: Arc<dyn FlagRepository> =
        Arc::new(PgFlagRepository::new(pool.clone(), audit.clone()));
    let variant_repo: Arc<dyn VariantRepository> =
        Arc::new(PgVariantRepository::new(pool.clone(), audit.clone()));
    let segment_repo: Arc<dyn SegmentRepository> =
        Arc::new(PgSegmentRepository::new(pool.clone(), audit.clone()));
    let sdk_key_repo: Arc<dyn SdkKeyRepository> =
        Arc::new(PgSdkKeyRepository::new(pool.clone(), audit.clone()));

    // ── Tenant hierarchy ────────────────────────────────────────────────────
    let org = Organisation {
        id: OrganisationId::new(),
        name: "SDK Test Org".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "SDK Test Project".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "sdk-test".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    // ── SDK key ─────────────────────────────────────────────────────────────
    let key_hash = stitchd_server::api::sdk_auth::hash_sdk_key(SDK_KEY);
    let sdk_key = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env.id,
        key_hash,
        is_active: true,
        created_at: Utc::now(),
        revoked_at: None,
    };
    sdk_key_repo.create(&sdk_key).await.unwrap();

    // ── Bool flag: always returns "on" (true) ────────────────────────────────
    let on_vid = VariantId::new();
    let bool_flag_id = FlagId::new();
    flag_repo
        .create(&FlagRecord {
            id: bool_flag_id,
            project_id: project.id,
            key: FlagKey::new("bool-flag").unwrap(),
            value_type: FlagValueType::Bool,
            enabled: true,
            default_variant_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        })
        .await
        .unwrap();
    variant_repo
        .create(
            bool_flag_id,
            &Variant { id: on_vid, key: "on".into(), value: VariantValue::BoolValue(true) },
        )
        .await
        .unwrap();
    flag_repo
        .upsert_rules(
            bool_flag_id,
            &[FlagRule {
                flag_id: bool_flag_id,
                rule_index: 0,
                rule: Rule {
                    id: RuleId::new(),
                    condition: ConditionExpr::And(vec![]),
                    output: RuleOutput::Variant(on_vid),
                },
            }],
        )
        .await
        .unwrap();

    // ── List segment: user "u1" is in the include list ───────────────────────
    let seg_id = SegmentId::new();
    segment_repo
        .create(&Segment {
            id: seg_id,
            environment_id: env.id,
            key: "beta-users".into(),
            segment_type: SegmentType::List,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        })
        .await
        .unwrap();
    segment_repo
        .set_list_entries(seg_id, "user", &["u1".to_string()], &[])
        .await
        .unwrap();

    // ── Segment flag: "beta" variant when user is in "beta-users" ────────────
    let beta_vid = VariantId::new();
    let seg_flag_id = FlagId::new();
    flag_repo
        .create(&FlagRecord {
            id: seg_flag_id,
            project_id: project.id,
            key: FlagKey::new("segment-flag").unwrap(),
            value_type: FlagValueType::Bool,
            enabled: true,
            default_variant_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        })
        .await
        .unwrap();
    variant_repo
        .create(
            seg_flag_id,
            &Variant {
                id: beta_vid,
                key: "beta".into(),
                value: VariantValue::BoolValue(true),
            },
        )
        .await
        .unwrap();
    flag_repo
        .upsert_rules(
            seg_flag_id,
            &[FlagRule {
                flag_id: seg_flag_id,
                rule_index: 0,
                rule: Rule {
                    id: RuleId::new(),
                    condition: ConditionExpr::Leaf(Condition::InSegment(seg_id)),
                    output: RuleOutput::Variant(beta_vid),
                },
            }],
        )
        .await
        .unwrap();

    // ── Spin up gRPC server ─────────────────────────────────────────────────
    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .build_recorder()
        .handle();

    let state = AppState {
        db: pool.clone(),
        metrics_handle,
        segment_repo: Arc::clone(&segment_repo),
        flag_repo: Arc::clone(&flag_repo),
        variant_repo: Arc::clone(&variant_repo),
        sdk_key_repo: Arc::clone(&sdk_key_repo),
    };

    let grpc_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc_addr = grpc_listener.local_addr().unwrap();
    let grpc_svc = FlagSyncServiceServer::new(FlagSyncServiceImpl::new(state.clone()));
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(grpc_svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(grpc_listener)),
    );

    // ── Spin up HTTP server ─────────────────────────────────────────────────
    let http_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_addr = http_listener.local_addr().unwrap();
    let router = build_router(state);
    tokio::spawn(async move {
        axum::serve(http_listener, router).await.unwrap();
    });

    let config = SdkConfig::new(
        format!("http://{grpc_addr}"),
        format!("http://{http_addr}"),
        SDK_KEY,
    );

    (config, env.id)
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn init_with_valid_key_succeeds(pool: sqlx::PgPool) {
    let (config, _) = setup(pool).await;
    assert!(SdkClient::init(config).await.is_ok());
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn init_with_invalid_key_fails(pool: sqlx::PgPool) {
    let (mut config, _) = setup(pool).await;
    config.sdk_key = "wrong-key".into();
    assert!(SdkClient::init(config).await.is_err());
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn evaluate_rule_based_flag_returns_correct_variant(pool: sqlx::PgPool) {
    let (config, _) = setup(pool).await;
    let client = SdkClient::init(config).await.unwrap();
    let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
    assert_eq!(
        client.evaluate("bool-flag", &ctx).await.unwrap(),
        Some(SdkVariantValue::BoolValue(true))
    );
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn evaluate_unknown_flag_returns_none(pool: sqlx::PgPool) {
    let (config, _) = setup(pool).await;
    let client = SdkClient::init(config).await.unwrap();
    let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
    assert_eq!(client.evaluate("does-not-exist", &ctx).await.unwrap(), None);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn evaluate_list_segment_included_user_matches(pool: sqlx::PgPool) {
    let (config, _) = setup(pool).await;
    let client = SdkClient::init(config).await.unwrap();
    // u1 is in the include list
    let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
    assert_eq!(
        client.evaluate("segment-flag", &ctx).await.unwrap(),
        Some(SdkVariantValue::BoolValue(true))
    );
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn evaluate_list_segment_excluded_user_returns_none(pool: sqlx::PgPool) {
    let (config, _) = setup(pool).await;
    let client = SdkClient::init(config).await.unwrap();
    // u2 is NOT in the include list
    let ctx = EvaluationContext::new().with_context(Context::new("user", "u2"));
    assert_eq!(client.evaluate("segment-flag", &ctx).await.unwrap(), None);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn rule_eval_succeeds_when_http_unreachable(pool: sqlx::PgPool) {
    let (mut config, _) = setup(pool).await;
    // Correct gRPC (for init) but broken HTTP (list-check will fail).
    // Rule-based flags need no HTTP.
    config.http_url = "http://127.0.0.1:1".into();
    let client = SdkClient::init(config).await.unwrap();
    let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
    assert_eq!(
        client.evaluate("bool-flag", &ctx).await.unwrap(),
        Some(SdkVariantValue::BoolValue(true))
    );
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn list_eval_errors_when_http_unreachable(pool: sqlx::PgPool) {
    let (mut config, _) = setup(pool).await;
    config.http_url = "http://127.0.0.1:1".into();
    let client = SdkClient::init(config).await.unwrap();
    let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));
    assert!(
        client.evaluate("segment-flag", &ctx).await.is_err(),
        "list eval should error when HTTP server is unreachable"
    );
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn lfu_warm_context_returns_correct_result(pool: sqlx::PgPool) {
    let (mut config, _) = setup(pool).await;
    config.lfu =
        Some(stitchd_sdk::LfuConfig { capacity: 10, window: Duration::from_secs(60) });
    config.poll_interval = Duration::from_millis(200);

    let client = SdkClient::init(config).await.unwrap();
    let ctx = EvaluationContext::new().with_context(Context::new("user", "u1"));

    // Warm up: call evaluate to make u1 hot
    for _ in 0..5 {
        client.evaluate("segment-flag", &ctx).await.unwrap();
    }

    // Wait for a poll cycle to populate the LFU cache
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Result should still be correct when served from LFU cache
    assert_eq!(
        client.evaluate("segment-flag", &ctx).await.unwrap(),
        Some(SdkVariantValue::BoolValue(true))
    );
}
