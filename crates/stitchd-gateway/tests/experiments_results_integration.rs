//! Integration test for the Phase 7 Task 1 extended `GET /results` response shape.
//!
//! Spins up an in-process mock `ExperimentationService` that returns an
//! `ExperimentResults` proto populated with per-context-type result bundles
//! (Phase 7 fields `results_by_context_type`, `bound_target`,
//! `pre_period_days`), then asserts the gateway's JSON response carries the
//! per-context-type breakdown along with SRM and guardrail buckets per
//! context type.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use tonic::transport::{Channel, Server};
use tonic::{Response, Status};
use tower::ServiceExt as _;

use stitchd_proto::analytics::v1::analytics_service_client::AnalyticsServiceClient;
use stitchd_proto::auth::v1::{
    auth_provider_service_client::AuthProviderServiceClient,
    auth_service_client::AuthServiceClient, oidc_login_service_client::OidcLoginServiceClient,
    saml_login_service_client::SamlLoginServiceClient,
};
use stitchd_proto::experiments::v1::{
    BoundTarget, ContextTypeResults, CreateExperimentRequest, DeleteExperimentRequest, Experiment,
    ExperimentIteration, ExperimentResults, GetExperimentIterationRequest, GetExperimentRequest,
    GetResultsRequest, ListExperimentsRequest, ListExperimentsResponse, ListIterationsRequest,
    ListIterationsResponse, ListRunningExperimentsRequest, RunningExperiment, SrmPerVariant,
    SrmResult as ProtoSrmResult, TransitionExperimentRequest, UpdateExperimentRequest,
    UpdateIterationLastComputedRequest, UpdateIterationLastComputedResponse, VariantResult,
    experimentation_service_client::ExperimentationServiceClient,
    experimentation_service_server::{ExperimentationService, ExperimentationServiceServer},
};
use stitchd_proto::flags::v1::flag_service_client::FlagServiceClient;
use stitchd_proto::management::v1::management_service_client::ManagementServiceClient;
use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;
use stitchd_proto::stats::v1::stats_service_client::StatsServiceClient;

use stitchd_gateway::state::GatewayState;

// ─── Mock ExperimentationService ──────────────────────────────────────────────

#[derive(Default, Clone)]
struct MockExpService {
    results: ExperimentResults,
    exposures: Vec<stitchd_proto::experiments::v1::ExposureRow>,
    exposure_total: u64,
}

#[tonic::async_trait]
impl ExperimentationService for MockExpService {
    async fn create_experiment(
        &self,
        _req: tonic::Request<CreateExperimentRequest>,
    ) -> Result<Response<Experiment>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_experiment(
        &self,
        _req: tonic::Request<GetExperimentRequest>,
    ) -> Result<Response<Experiment>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn list_experiments(
        &self,
        _req: tonic::Request<ListExperimentsRequest>,
    ) -> Result<Response<ListExperimentsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn update_experiment(
        &self,
        _req: tonic::Request<UpdateExperimentRequest>,
    ) -> Result<Response<Experiment>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn delete_experiment(
        &self,
        _req: tonic::Request<DeleteExperimentRequest>,
    ) -> Result<Response<Experiment>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn transition_experiment(
        &self,
        _req: tonic::Request<TransitionExperimentRequest>,
    ) -> Result<Response<Experiment>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn list_iterations(
        &self,
        _req: tonic::Request<ListIterationsRequest>,
    ) -> Result<Response<ListIterationsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_results(
        &self,
        _req: tonic::Request<GetResultsRequest>,
    ) -> Result<Response<ExperimentResults>, Status> {
        Ok(Response::new(self.results.clone()))
    }

    type ListRunningExperimentsStream =
        tokio_stream::wrappers::ReceiverStream<Result<RunningExperiment, Status>>;

    async fn list_running_experiments(
        &self,
        _req: tonic::Request<ListRunningExperimentsRequest>,
    ) -> Result<Response<Self::ListRunningExperimentsStream>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_experiment_iteration(
        &self,
        _req: tonic::Request<GetExperimentIterationRequest>,
    ) -> Result<Response<ExperimentIteration>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn update_iteration_last_computed(
        &self,
        _req: tonic::Request<UpdateIterationLastComputedRequest>,
    ) -> Result<Response<UpdateIterationLastComputedResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn list_exposures(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::ListExposuresRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ListExposuresResponse>, Status> {
        Ok(Response::new(
            stitchd_proto::experiments::v1::ListExposuresResponse {
                exposures: self.exposures.clone(),
                total: self.exposure_total,
            },
        ))
    }

    async fn create_exclusion_group(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::CreateExclusionGroupRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ExclusionGroup>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn list_exclusion_groups(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::ListExclusionGroupsRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ListExclusionGroupsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn update_exclusion_group(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::UpdateExclusionGroupRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ExclusionGroup>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn delete_exclusion_group(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::DeleteExclusionGroupRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::DeleteExclusionGroupResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn assign_experiment_to_group(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::AssignExperimentToGroupRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::AssignExperimentToGroupResponse>, Status>
    {
        Err(Status::unimplemented("not used"))
    }

    async fn unassign_experiment(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::UnassignExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::UnassignExperimentResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_experiment_interactions(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::GetExperimentInteractionsRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::GetExperimentInteractionsResponse>, Status>
    {
        Err(Status::unimplemented("not used"))
    }
}

async fn spawn_mock_exp_service(svc: MockExpService) -> ExperimentationServiceClient<Channel> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(ExperimentationServiceServer::new(svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    ExperimentationServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap()
}

fn make_state(exp_client: ExperimentationServiceClient<Channel>) -> Arc<GatewayState> {
    let flag_channel = Channel::from_static("http://127.0.0.1:2").connect_lazy();
    let seg_channel = Channel::from_static("http://127.0.0.1:3").connect_lazy();
    let state = GatewayState::from_channels(
        AuthServiceClient::new(Channel::from_static("http://127.0.0.1:1").connect_lazy()),
        FlagServiceClient::new(flag_channel.clone()),
        flag_channel,
        SegmentationServiceClient::new(seg_channel.clone()),
        seg_channel,
        AnalyticsServiceClient::new(Channel::from_static("http://127.0.0.1:4").connect_lazy()),
        exp_client,
        ManagementServiceClient::new(Channel::from_static("http://127.0.0.1:6").connect_lazy()),
        AuthProviderServiceClient::new(Channel::from_static("http://127.0.0.1:7").connect_lazy()),
        OidcLoginServiceClient::new(Channel::from_static("http://127.0.0.1:8").connect_lazy()),
        SamlLoginServiceClient::new(Channel::from_static("http://127.0.0.1:9").connect_lazy()),
        StatsServiceClient::new(Channel::from_static("http://127.0.0.1:10").connect_lazy()),
    );
    Arc::new(state)
}

fn build_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::get;
    use stitchd_gateway::routes::experiments as exp_routes;
    axum::Router::new()
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/results",
            get(exp_routes::get_results),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/exposures",
            get(exp_routes::list_exposures),
        )
        .with_state(state)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_results_returns_per_context_type_shape_with_srm_and_guardrails() {
    let exp_id = "11111111-1111-1111-1111-111111111111";
    let rule_id = "22222222-2222-2222-2222-222222222222";

    // Build a mock ExperimentResults proto with two context types
    // (user, account), each with two variants + a guardrail row, and an
    // SRM block under "user".
    let user_bundle = ContextTypeResults {
        context_type: "user".to_string(),
        variants: vec![
            VariantResult {
                variant_key: "control".to_string(),
                participant_count: 1000,
                p_value: 0.0,
                p_value_present: false,
                context_type: "user".to_string(),
                lift: 0.0,
                ..Default::default()
            },
            VariantResult {
                variant_key: "treatment".to_string(),
                participant_count: 1020,
                p_value: 0.03,
                p_value_present: true,
                p_value_corrected: Some(0.06),
                context_type: "user".to_string(),
                lift: 0.05,
                ..Default::default()
            },
        ],
        srm: Some(ProtoSrmResult {
            per_variant: vec![
                SrmPerVariant {
                    variant_key: "control".to_string(),
                    observed: 1000,
                    expected: 1010.0,
                    chi_sq_contribution: 0.099,
                },
                SrmPerVariant {
                    variant_key: "treatment".to_string(),
                    observed: 1020,
                    expected: 1010.0,
                    chi_sq_contribution: 0.099,
                },
            ],
            overall_chi_sq: 0.198,
            overall_chi_sq_p: 0.65,
            health: "green".to_string(),
        }),
        guardrails: vec![VariantResult {
            variant_key: "treatment".to_string(),
            participant_count: 1020,
            context_type: "user".to_string(),
            direction_violation: true,
            lift: -0.4,
            ..Default::default()
        }],
    };

    let account_bundle = ContextTypeResults {
        context_type: "account".to_string(),
        variants: vec![VariantResult {
            variant_key: "control".to_string(),
            participant_count: 50,
            context_type: "account".to_string(),
            ..Default::default()
        }],
        srm: None,
        guardrails: vec![],
    };

    let svc = MockExpService {
        results: ExperimentResults {
            experiment_id: exp_id.to_string(),
            variant_results: vec![],
            computed_at_ms: 1_700_000_000_000,
            is_stale: false,
            next_run_at_ms: 1_700_000_600_000,
            computation_status: "ready".to_string(),
            results_by_context_type: vec![user_bundle, account_bundle],
            bound_target: Some(BoundTarget {
                kind: "rule".to_string(),
                rule_id: rule_id.to_string(),
                label: rule_id.to_string(),
            }),
            pre_period_days: 14,
        },
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let state = make_state(exp_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/environments/env-1/experiments/{exp_id}/results"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    // ── results_by_context_type shape ───────────────────────────────────────
    let by_ct = &body["results_by_context_type"];
    assert!(
        by_ct.is_object(),
        "results_by_context_type must be an object"
    );
    let user_obj = &by_ct["user"];
    assert!(user_obj.is_object(), "user bucket must be an object");
    assert_eq!(user_obj["variants"].as_array().unwrap().len(), 2);
    assert_eq!(user_obj["guardrails"].as_array().unwrap().len(), 1);

    // Treatment row carries p_value + p_value_corrected + lift.
    let treatment = user_obj["variants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["variant_key"] == "treatment")
        .expect("treatment variant present");
    assert!((treatment["p_value"].as_f64().unwrap() - 0.03).abs() < 1e-9);
    assert!((treatment["p_value_corrected"].as_f64().unwrap() - 0.06).abs() < 1e-9);
    assert!((treatment["lift"].as_f64().unwrap() - 0.05).abs() < 1e-9);
    assert_eq!(treatment["context_type"], "user");

    // Control row: no p_value field at all (skip_serializing_if = Option::is_none).
    let control = user_obj["variants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["variant_key"] == "control")
        .expect("control variant present");
    assert!(
        control.get("p_value").is_none(),
        "control row should omit p_value"
    );

    // SRM is present for user; absent for account.
    let srm = &user_obj["srm"];
    assert_eq!(srm["health"], "green");
    assert_eq!(srm["per_variant"].as_array().unwrap().len(), 2);

    let account_obj = &by_ct["account"];
    assert!(
        account_obj.get("srm").is_none() || account_obj["srm"].is_null(),
        "account bucket has no SRM"
    );

    // Guardrail row carries direction_violation = true.
    let gr = &user_obj["guardrails"].as_array().unwrap()[0];
    assert_eq!(gr["direction_violation"], true);

    // ── bound_target ─────────────────────────────────────────────────────────
    let bt = &body["bound_target"];
    assert_eq!(bt["kind"], "rule");
    assert_eq!(bt["rule_id"], rule_id);

    // ── pre_period_days ──────────────────────────────────────────────────────
    assert_eq!(body["pre_period_days"], 14);
}

#[tokio::test]
async fn list_exposures_proxies_to_grpc_and_returns_paginated_rows() {
    let exp_id = "44444444-4444-4444-4444-444444444444";
    let rule_id = "55555555-5555-5555-5555-555555555555";

    let svc = MockExpService {
        results: ExperimentResults::default(),
        exposures: vec![
            stitchd_proto::experiments::v1::ExposureRow {
                context_type: "user".to_string(),
                context_key: "alice".to_string(),
                variant_key: "treatment".to_string(),
                assigned_at: "2026-05-21T10:00:00Z".to_string(),
                matched_rule_id: rule_id.to_string(),
            },
            stitchd_proto::experiments::v1::ExposureRow {
                context_type: "user".to_string(),
                context_key: "bob".to_string(),
                variant_key: "control".to_string(),
                assigned_at: "2026-05-21T09:30:00Z".to_string(),
                matched_rule_id: String::new(),
            },
        ],
        exposure_total: 1234,
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let state = make_state(exp_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/environments/env-1/experiments/{exp_id}/exposures?context_type=user&page=1&per_page=50"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    let items = body["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["context_key"], "alice");
    assert_eq!(items[0]["matched_rule_id"], rule_id);
    // default-rule exposure omits matched_rule_id.
    assert!(items[1].get("matched_rule_id").is_none());
    assert_eq!(body["total"], 1234);
}

#[tokio::test]
async fn list_exposures_rejects_missing_context_type_with_400() {
    let exp_id = "66666666-6666-6666-6666-666666666666";
    let svc = MockExpService::default();
    let exp_client = spawn_mock_exp_service(svc).await;
    let state = make_state(exp_client);
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/environments/env-1/experiments/{exp_id}/exposures"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["error"], "missing_context_type");
}
