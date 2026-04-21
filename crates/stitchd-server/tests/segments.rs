use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use stitchd_core::{
    id::{EnvironmentId, OrganisationId, ProjectId, SegmentId},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, OrganisationRepository, ProjectRepository,
    PgAuthUserRepository, PgOrgMembershipRepository, PgRefreshTokenRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgEventDefinitionRepository,
        PgExperimentRepository, PgFlagRepository, PgOrganisationRepository, PgProjectRepository,
        PgSdkKeyRepository, PgSegmentRepository, PgUserRepository, PgVariantRepository,
    },
};
use stitchd_server::{AppState, build_router};
use tower::ServiceExt as _;

async fn setup_app(pool: sqlx::PgPool) -> (axum::Router, EnvironmentId) {
    let audit = Arc::new(PgAuditLogger::new(pool.clone()));
    let org_repo = PgOrganisationRepository::new(pool.clone(), audit.clone());
    let proj_repo = PgProjectRepository::new(pool.clone(), audit.clone());
    let env_repo = PgEnvironmentRepository::new(pool.clone(), audit.clone());
    let segment_repo = Arc::new(PgSegmentRepository::new(pool.clone(), audit.clone()));
    let flag_repo = Arc::new(PgFlagRepository::new(pool.clone(), audit.clone()));
    let variant_repo = Arc::new(PgVariantRepository::new(pool.clone(), audit.clone()));
    let sdk_key_repo = Arc::new(PgSdkKeyRepository::new(pool.clone(), audit.clone()));
    let event_definition_repo = Arc::new(PgEventDefinitionRepository::new(
        pool.clone(),
        audit.clone(),
    ));

    // Setup hierarchy
    let org = Organisation {
        id: OrganisationId::new(),
        name: "Test Org".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "Test Project".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "Test Env".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .build_recorder()
        .handle();

    let experiment_repo = Arc::new(PgExperimentRepository::new(pool.clone(), audit.clone()));
    let results_repo =
        Arc::new(stitchd_db::experiment_results::PgExperimentResultsRepository::new(pool.clone()));
    let state = AppState {
        db: pool.clone(),
        metrics_handle,
        user_repo: Arc::new(PgUserRepository::new(pool.clone(), audit.clone())),
        auth_user_repo: Arc::new(PgAuthUserRepository::new(pool.clone())),
        membership_repo: Arc::new(PgOrgMembershipRepository::new(pool.clone())),
        refresh_token_repo: Arc::new(PgRefreshTokenRepository::new(pool.clone())),
        mfa_repo: Arc::new(stitchd_db::PgMfaRepository::new(pool.clone())),
        segment_repo,
        flag_repo,
        variant_repo,
        sdk_key_repo,
        event_definition_repo,
        experiment_repo,
        results_repo,
        ch_client: None,
        event_writer: None,
    };

    (build_router(state), env.id)
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_create_list_segment(pool: sqlx::PgPool) {
    let (app, env_id) = setup_app(pool).await;

    let req_body = json!({
        "key": "beta-testers",
        "segment_type": "list",
        "lists": {
            "user": {
                "include": ["u1"],
                "exclude": ["u2"]
            }
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/segments"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_create_rule_segment_invalid_condition(pool: sqlx::PgPool) {
    let (app, env_id) = setup_app(pool).await;

    let req_body = json!({
        "key": "invalid-segment",
        "segment_type": "rule",
        "rules": [
            {
                "id": SegmentId::new(), // just using a UUID
                "condition": {
                    "type": "leaf",
                    "leaf": {
                        "in_segment": "00000000-0000-0000-0000-000000000000"
                    }
                },
                "output": { "variant": "00000000-0000-0000-0000-000000000000" }
            }
        ]
    });

    // Note: Condition serialization might be different depending on how
    // it was implemented in rule-engine track.
    // Assuming the above JSON shape for now.

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/segments"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_get_unknown_segment(pool: sqlx::PgPool) {
    let (app, env_id) = setup_app(pool).await;
    let seg_id = SegmentId::new();

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/environments/{env_id}/segments/{seg_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
