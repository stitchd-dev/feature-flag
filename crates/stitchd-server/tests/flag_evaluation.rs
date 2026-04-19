use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use stitchd_core::{
    flag::Variant,
    id::{EnvironmentId, OrganisationId, ProjectId},
    tenant::{Environment, Organisation, Project},
};
use stitchd_db::{
    EnvironmentRepository, OrganisationRepository, ProjectRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgEventDefinitionRepository,
        PgExperimentRepository, PgFlagRepository, PgOrganisationRepository, PgProjectRepository,
        PgSdkKeyRepository, PgSegmentRepository, PgVariantRepository,
    },
};
use stitchd_server::{AppState, build_router};
use tower::ServiceExt as _;

async fn setup_app(pool: sqlx::PgPool) -> (axum::Router, EnvironmentId, ProjectId) {
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
    let state = AppState {
        db: pool,
        metrics_handle,
        segment_repo,
        flag_repo,
        variant_repo,
        sdk_key_repo,
        event_definition_repo,
        experiment_repo,
        event_writer: None,
    };

    (build_router(state), env.id, project.id)
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn test_flag_evaluation_flow(pool: sqlx::PgPool) {
    let (app, env_id, project_id) = setup_app(pool).await;

    // 1. Create a flag via API
    let create_flag_req = json!({
        "key": "test-flag",
        "value_type": "bool",
        "enabled": true
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/projects/{project_id}/flags"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_flag_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let flag_record: stitchd_core::flag::FlagRecord = serde_json::from_slice(&body).unwrap();
    let flag_id = flag_record.id;

    // 2. Create variants via API
    let create_v1_req = json!({ "key": "on", "value": true });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/projects/{project_id}/flags/{flag_id}/variants"
                ))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_v1_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v1: Variant = serde_json::from_slice(&body).unwrap();

    let create_v2_req = json!({ "key": "off", "value": false });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/projects/{project_id}/flags/{flag_id}/variants"
                ))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_v2_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v2: Variant = serde_json::from_slice(&body).unwrap();

    // 3. Set default variant
    let update_flag_req = json!({
        "default_variant_id": v2.id,
        "version": flag_record.version
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/v1/projects/{project_id}/flags/{flag_id}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&update_flag_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 4. Set rules
    let rules_req = json!([
        {
            "flag_id": flag_id,
            "rule_index": 0,
            "rule": {
                "id": stitchd_core::id::RuleId::new(),
                "condition": {
                    "Leaf": {
                        "Eq": {
                            "context_type": "user",
                            "param": "beta",
                            "value": true
                        }
                    }
                },
                "output": { "Variant": v1.id }
            }
        }
    ]);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/v1/projects/{project_id}/flags/{flag_id}/rules"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&rules_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // 5. Evaluate (Match)
    let eval_req = json!({
        "context": {
            "contexts": [
                {
                    "context_type": "user",
                    "key": "u1",
                    "parameters": { "beta": true },
                    "private_parameters": []
                }
            ]
        }
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/evaluate"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&eval_req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let eval_res: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(eval_res["results"][0]["variant_key"], "on");

    // 6. Evaluate (No Match -> Default)
    let eval_req_no_match = json!({
        "context": {
            "contexts": [
                {
                    "context_type": "user",
                    "key": "u2",
                    "parameters": { "beta": false },
                    "private_parameters": []
                }
            ]
        }
    });
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/evaluate"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&eval_req_no_match).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let eval_res: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(eval_res["results"][0]["variant_key"], "off");
}
