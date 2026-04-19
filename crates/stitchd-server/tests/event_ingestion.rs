//! Integration tests for event ingestion endpoints.
//!
//! Tests cover: valid single event (202), valid batch (202), unknown metric_key (422),
//! type mismatch (422), batch > 500 events (422), missing auth (401).

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use stitchd_core::{
    event::{EventDefinition, EventValueType},
    id::{EnvironmentId, EventDefinitionId, OrganisationId, ProjectId, SdkKeyId},
    tenant::{Environment, Organisation, Project, SdkKey},
};
use stitchd_db::{
    EnvironmentRepository, EventDefinitionRepository, OrganisationRepository, ProjectRepository,
    SdkKeyRepository,
    repository::pg::{
        PgAuditLogger, PgEnvironmentRepository, PgEventDefinitionRepository, PgFlagRepository,
        PgOrganisationRepository, PgProjectRepository, PgSdkKeyRepository, PgSegmentRepository,
        PgVariantRepository,
    },
};
use stitchd_server::{AppState, api::sdk_auth::hash_sdk_key, build_router};
use tower::ServiceExt as _;

const SDK_KEY: &str = "test-ingestion-key";

async fn setup(pool: sqlx::PgPool) -> (axum::Router, EnvironmentId) {
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

    let org = Organisation {
        id: OrganisationId::new(),
        name: "Ingestion Test Org".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    org_repo.create(&org).await.unwrap();

    let project = Project {
        id: ProjectId::new(),
        organisation_id: org.id,
        name: "Ingestion Test Project".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    proj_repo.create(&project).await.unwrap();

    let env = Environment {
        id: EnvironmentId::new(),
        project_id: project.id,
        name: "ingestion-test".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    env_repo.create(&env).await.unwrap();

    let sdk_key = SdkKey {
        id: SdkKeyId::new(),
        environment_id: env.id,
        key_hash: hash_sdk_key(SDK_KEY),
        is_active: true,
        created_at: Utc::now(),
        revoked_at: None,
    };
    sdk_key_repo.create(&sdk_key).await.unwrap();

    // Register event definitions
    let bool_def = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env.id,
        key: "conversion".into(),
        value_type: EventValueType::Bool,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    event_definition_repo.create(&bool_def).await.unwrap();

    let int_def = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env.id,
        key: "click_count".into(),
        value_type: EventValueType::Int,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    event_definition_repo.create(&int_def).await.unwrap();

    let double_def = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env.id,
        key: "revenue".into(),
        value_type: EventValueType::Double,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };
    event_definition_repo.create(&double_def).await.unwrap();

    let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .build_recorder()
        .handle();

    let state = AppState {
        db: pool,
        metrics_handle,
        segment_repo,
        flag_repo,
        variant_repo,
        sdk_key_repo,
        event_definition_repo,
        event_writer: None,
    };

    (build_router(state), env.id)
}

// ── Single event ─────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn single_bool_event_accepted(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    let payload = json!({
        "contexts": [{"_type": "user", "key": "u1"}],
        "metric_key": "conversion",
        "value": { "bool": true },
        "timestamp": Utc::now()
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events"))
                .header("Content-Type", "application/json")
                .header("x-sdk-key", SDK_KEY)
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn single_int_event_accepted(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    let payload = json!({
        "contexts": [{"_type": "user", "key": "u2"}],
        "metric_key": "click_count",
        "value": { "int": 42 },
        "timestamp": Utc::now()
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events"))
                .header("Content-Type", "application/json")
                .header("x-sdk-key", SDK_KEY)
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn single_double_event_accepted(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    let payload = json!({
        "contexts": [{"_type": "user", "key": "u3"}],
        "metric_key": "revenue",
        "value": { "double": 9.99 },
        "timestamp": Utc::now()
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events"))
                .header("Content-Type", "application/json")
                .header("x-sdk-key", SDK_KEY)
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn single_event_unknown_key_rejected_422(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    let payload = json!({
        "contexts": [{"_type": "user", "key": "u1"}],
        "metric_key": "does_not_exist",
        "value": { "bool": true },
        "timestamp": Utc::now()
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events"))
                .header("Content-Type", "application/json")
                .header("x-sdk-key", SDK_KEY)
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn single_event_type_mismatch_rejected_422(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    // "conversion" is registered as Bool but we send Int
    let payload = json!({
        "contexts": [{"_type": "user", "key": "u1"}],
        "metric_key": "conversion",
        "value": { "int": 1 },
        "timestamp": Utc::now()
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events"))
                .header("Content-Type", "application/json")
                .header("x-sdk-key", SDK_KEY)
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn single_event_missing_auth_rejected_401(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    let payload = json!({
        "contexts": [{"_type": "user", "key": "u1"}],
        "metric_key": "conversion",
        "value": { "bool": true },
        "timestamp": Utc::now()
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ── Batch events ──────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn batch_events_accepted(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    let events = vec![
        json!({
            "contexts": [{"_type": "user", "key": "u1"}],
            "metric_key": "conversion",
            "value": { "bool": true },
            "timestamp": Utc::now()
        }),
        json!({
            "contexts": [{"_type": "user", "key": "u2"}],
            "metric_key": "click_count",
            "value": { "int": 5 },
            "timestamp": Utc::now()
        }),
    ];

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events/batch"))
                .header("Content-Type", "application/json")
                .header("x-sdk-key", SDK_KEY)
                .body(Body::from(
                    serde_json::to_vec(&json!({ "events": events })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn batch_over_500_rejected_422(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    let events: Vec<serde_json::Value> = (0..501)
        .map(|i| {
            json!({
                "contexts": [{"_type": "user", "key": format!("u{i}")}],
                "metric_key": "conversion",
                "value": { "bool": true },
                "timestamp": Utc::now()
            })
        })
        .collect();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events/batch"))
                .header("Content-Type", "application/json")
                .header("x-sdk-key", SDK_KEY)
                .body(Body::from(
                    serde_json::to_vec(&json!({ "events": events })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn batch_unknown_key_rejected_422(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    let events = vec![
        json!({
            "contexts": [{"_type": "user", "key": "u1"}],
            "metric_key": "conversion",
            "value": { "bool": true },
            "timestamp": Utc::now()
        }),
        json!({
            "contexts": [{"_type": "user", "key": "u1"}],
            "metric_key": "unknown_key",
            "value": { "int": 1 },
            "timestamp": Utc::now()
        }),
    ];

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events/batch"))
                .header("Content-Type", "application/json")
                .header("x-sdk-key", SDK_KEY)
                .body(Body::from(
                    serde_json::to_vec(&json!({ "events": events })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn batch_type_mismatch_rejected_422(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    // "revenue" is Double but we send Bool
    let events = vec![json!({
        "contexts": [{"_type": "user", "key": "u1"}],
        "metric_key": "revenue",
        "value": { "bool": false },
        "timestamp": Utc::now()
    })];

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events/batch"))
                .header("Content-Type", "application/json")
                .header("x-sdk-key", SDK_KEY)
                .body(Body::from(
                    serde_json::to_vec(&json!({ "events": events })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../stitchd-db/migrations")]
async fn batch_missing_auth_rejected_401(pool: sqlx::PgPool) {
    let (app, env_id) = setup(pool).await;

    let events = vec![json!({
        "contexts": [{"_type": "user", "key": "u1"}],
        "metric_key": "conversion",
        "value": { "bool": true },
        "timestamp": Utc::now()
    })];

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/environments/{env_id}/events/batch"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "events": events })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
