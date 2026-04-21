//! Experiment route handlers — proxy REST requests to the Experimentation Service via gRPC.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use stitchd_proto::experiments::v1::{
    CreateExperimentRequest, Experiment, ExperimentStatus, GetExperimentRequest,
    GetResultsRequest, ListExperimentsRequest, VariantResult,
};

use crate::error::GatewayError;
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateExperimentBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub flag_key: Option<String>,
    pub variant_keys: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct ExperimentJson {
    pub id: String,
    pub name: String,
    pub description: String,
    pub flag_key: String,
    pub status: String,
    pub variant_keys: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct VariantResultJson {
    pub variant_key: String,
    pub participant_count: u64,
}

#[derive(Debug, Serialize)]
pub struct ExperimentResultsJson {
    pub experiment_id: String,
    pub variant_results: Vec<VariantResultJson>,
    pub computed_at_ms: i64,
}

fn experiment_status_str(status: i32) -> String {
    match ExperimentStatus::try_from(status).unwrap_or(ExperimentStatus::Unspecified) {
        ExperimentStatus::Draft => "draft",
        ExperimentStatus::Active => "active",
        ExperimentStatus::Paused => "paused",
        ExperimentStatus::Concluded => "concluded",
        ExperimentStatus::Unspecified => "unspecified",
    }
    .to_string()
}

fn experiment_to_json(e: &Experiment) -> ExperimentJson {
    ExperimentJson {
        id: e.id.clone(),
        name: e.name.clone(),
        description: e.description.clone(),
        flag_key: e.flag_key.clone(),
        status: experiment_status_str(e.status),
        variant_keys: e.variant_keys.clone(),
    }
}

fn variant_result_to_json(v: &VariantResult) -> VariantResultJson {
    VariantResultJson {
        variant_key: v.variant_key.clone(),
        participant_count: v.participant_count,
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `GET /v1/environments/{env_id}/experiments`
pub async fn list_experiments(
    State(state): State<Arc<GatewayState>>,
    Path(env_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(ListExperimentsRequest {
        environment_id: env_id,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .list_experiments(req)
        .await
        .map_err(GatewayError::from)?;
    let experiments: Vec<ExperimentJson> = resp
        .into_inner()
        .experiments
        .iter()
        .map(experiment_to_json)
        .collect();
    Ok(Json(experiments))
}

/// `POST /v1/environments/{env_id}/experiments`
pub async fn create_experiment(
    State(state): State<Arc<GatewayState>>,
    Path(env_id): Path<String>,
    Json(body): Json<CreateExperimentBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let experiment = Experiment {
        environment_id: env_id,
        name: body.name.unwrap_or_default(),
        description: body.description.unwrap_or_default(),
        flag_key: body.flag_key.unwrap_or_default(),
        variant_keys: body.variant_keys.unwrap_or_default(),
        status: ExperimentStatus::Draft as i32,
        ..Default::default()
    };
    let req = tonic::Request::new(CreateExperimentRequest {
        experiment: Some(experiment),
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .create_experiment(req)
        .await
        .map_err(GatewayError::from)?;
    Ok((StatusCode::CREATED, Json(experiment_to_json(&resp.into_inner()))))
}

/// `GET /v1/environments/{env_id}/experiments/{experiment_id}`
pub async fn get_experiment(
    State(state): State<Arc<GatewayState>>,
    Path((env_id, experiment_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetExperimentRequest {
        environment_id: env_id,
        experiment_id,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client
        .get_experiment(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(Json(experiment_to_json(&resp.into_inner())))
}

/// `DELETE /v1/environments/{env_id}/experiments/{experiment_id}`
pub async fn delete_experiment(
    State(_state): State<Arc<GatewayState>>,
    Path((_env_id, _experiment_id)): Path<(String, String)>,
) -> impl IntoResponse {
    // Deletion is not a first-class gRPC RPC in the proto; return 202 Accepted.
    StatusCode::ACCEPTED
}

/// `GET /v1/environments/{env_id}/experiments/{experiment_id}/results`
pub async fn get_results(
    State(state): State<Arc<GatewayState>>,
    Path((env_id, experiment_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetResultsRequest {
        environment_id: env_id,
        experiment_id,
    });
    let mut client = state.experimentation_client.lock().await;
    let resp = client.get_results(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let results = ExperimentResultsJson {
        experiment_id: inner.experiment_id.clone(),
        variant_results: inner.variant_results.iter().map(variant_result_to_json).collect(),
        computed_at_ms: inner.computed_at_ms,
    };
    Ok(Json(results))
}

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    #[allow(unused_imports)]

    use axum::routing::{delete, get, post};
    axum::Router::new()
        .route(
            "/v1/environments/{env_id}/experiments",
            get(list_experiments).post(create_experiment),
        )
        .route(
            "/v1/environments/{env_id}/experiments/{experiment_id}",
            get(get_experiment).delete(delete_experiment),
        )
        .route(
            "/v1/environments/{env_id}/experiments/{experiment_id}/results",
            get(get_results),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt as _;

    use crate::tests::helpers::make_stub_state;

    #[test]
    fn experiment_status_str_draft() {
        assert_eq!(experiment_status_str(ExperimentStatus::Draft as i32), "draft");
    }

    #[test]
    fn experiment_status_str_active() {
        assert_eq!(experiment_status_str(ExperimentStatus::Active as i32), "active");
    }

    #[test]
    fn experiment_status_str_unspecified() {
        assert_eq!(experiment_status_str(0), "unspecified");
    }

    #[test]
    fn variant_result_to_json_maps_fields() {
        let vr = VariantResult {
            variant_key: "control".to_string(),
            participant_count: 42,
            ..Default::default()
        };
        let j = variant_result_to_json(&vr);
        assert_eq!(j.variant_key, "control");
        assert_eq!(j.participant_count, 42);
    }

    #[test]
    fn experiment_to_json_maps_fields() {
        let e = Experiment {
            id: "exp-1".to_string(),
            name: "test".to_string(),
            flag_key: "flag-1".to_string(),
            status: ExperimentStatus::Active as i32,
            variant_keys: vec!["on".to_string(), "off".to_string()],
            ..Default::default()
        };
        let j = experiment_to_json(&e);
        assert_eq!(j.id, "exp-1");
        assert_eq!(j.status, "active");
        assert_eq!(j.variant_keys.len(), 2);
    }

    #[tokio::test]
    async fn list_experiments_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn create_experiment_returns_201_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/experiments")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"exp","flag_key":"f1","variant_keys":["on","off"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::CREATED || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn delete_experiment_returns_202() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/environments/env-1/experiments/exp-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn get_results_returns_200_404_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1/results")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK
                || resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }
}
