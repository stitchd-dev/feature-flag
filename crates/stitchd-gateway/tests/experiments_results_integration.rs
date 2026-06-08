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
    bandit_state: stitchd_proto::experiments::v1::GetBanditStateResponse,
    bandit_history: Vec<stitchd_proto::experiments::v1::BanditAllocationRun>,
    campaigns: Vec<stitchd_proto::experiments::v1::BanditCampaign>,
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

    async fn get_exclusion_group(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::GetExclusionGroupRequest>,
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
    ) -> Result<Response<stitchd_proto::experiments::v1::DeleteExclusionGroupResponse>, Status>
    {
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

    async fn get_experiment_start_prerequisites(
        &self,
        _req: tonic::Request<
            stitchd_proto::experiments::v1::GetExperimentStartPrerequisitesRequest,
        >,
    ) -> Result<
        Response<stitchd_proto::experiments::v1::GetExperimentStartPrerequisitesResponse>,
        Status,
    > {
        Err(Status::unimplemented("not used"))
    }

    async fn apply_bandit_allocation(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::ApplyBanditAllocationRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ApplyBanditAllocationResponse>, Status>
    {
        Err(Status::unimplemented("not used"))
    }

    async fn create_bandit_campaign(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::CreateBanditCampaignRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::BanditCampaign>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn get_bandit_campaign(
        &self,
        req: tonic::Request<stitchd_proto::experiments::v1::GetBanditCampaignRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::BanditCampaign>, Status> {
        let id = req.into_inner().campaign_id;
        self.campaigns
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .map(Response::new)
            .ok_or_else(|| Status::not_found("campaign not found"))
    }
    async fn stop_bandit_campaign(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::StopBanditCampaignRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::BanditCampaign>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_bandit_state(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::GetBanditStateRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::GetBanditStateResponse>, Status> {
        Ok(Response::new(self.bandit_state.clone()))
    }

    async fn get_bandit_allocation_history(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::GetBanditAllocationHistoryRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::GetBanditAllocationHistoryResponse>, Status>
    {
        Ok(Response::new(
            stitchd_proto::experiments::v1::GetBanditAllocationHistoryResponse {
                runs: self.bandit_history.clone(),
            },
        ))
    }

    async fn list_bandit_campaigns(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::ListBanditCampaignsRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ListBanditCampaignsResponse>, Status> {
        Ok(Response::new(
            stitchd_proto::experiments::v1::ListBanditCampaignsResponse {
                campaigns: self.campaigns.clone(),
            },
        ))
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
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/bandit",
            get(exp_routes::get_bandit_state),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/bandit/history",
            get(exp_routes::get_bandit_history),
        )
        .route(
            "/v1/environments/{environment_id}/bandit-campaigns",
            get(exp_routes::list_bandit_campaigns),
        )
        .route(
            "/v1/environments/{environment_id}/bandit-campaigns/{campaign_id}",
            get(exp_routes::get_bandit_campaign),
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
        ..Default::default()
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

// ─── Bandit surfacing (FR7) integration tests ─────────────────────────────────

#[tokio::test]
async fn get_bandit_state_returns_allocation_posteriors_convergence_and_campaign() {
    use stitchd_proto::experiments::v1::{BanditAllocationBucket, GetBanditStateResponse};
    let exp_id = "77777777-7777-7777-7777-777777777777";

    let svc = MockExpService {
        bandit_state: GetBanditStateResponse {
            experiment_id: exp_id.to_string(),
            is_bandit: true,
            current_allocation: vec![
                BanditAllocationBucket {
                    variant_key: "control".to_string(),
                    weight_bp: 3000,
                },
                BanditAllocationBucket {
                    variant_key: "treatment".to_string(),
                    weight_bp: 7000,
                },
            ],
            objectives_json: r#"{"objectives":[{"metric_id":"m1","role":"scalar","goal":"increase","variants":[{"variant_key":"treatment","mean":0.31,"ci_lower":0.28,"ci_upper":0.34,"n":1200,"guardrail_violated":false}]}]}"#.to_string(),
            bandit_config_json: r#"{"algorithm":{"type":"thompson_sampling"},"propagation_mode":"static","min_exploration_bp":500,"lifecycle_policy":"advisory"}"#.to_string(),
            converged_variant: "treatment".to_string(),
            converged_prob: 0.97,
            has_converged: true,
            committed: false,
            campaign_id: "cmp-1".to_string(),
            campaign_status: "active".to_string(),
        },
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/environments/env-1/experiments/{exp_id}/bandit"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();

    assert_eq!(body["is_bandit"], true);
    let alloc = body["current_allocation"].as_array().unwrap();
    assert_eq!(alloc.len(), 2);
    let treatment = alloc
        .iter()
        .find(|a| a["variant_key"] == "treatment")
        .unwrap();
    assert_eq!(treatment["weight_bp"], 7000);

    // Posteriors re-hydrated into nested JSON (not a string).
    assert!(body["objectives"]["objectives"].is_array());
    assert_eq!(
        body["objectives"]["objectives"][0]["variants"][0]["mean"],
        0.31
    );

    // Config summary re-hydrated.
    assert_eq!(
        body["bandit_config"]["algorithm"]["type"],
        "thompson_sampling"
    );
    assert_eq!(body["bandit_config"]["min_exploration_bp"], 500);

    // Convergence + campaign.
    assert_eq!(body["converged_variant"], "treatment");
    assert!((body["converged_prob"].as_f64().unwrap() - 0.97).abs() < 1e-9);
    assert_eq!(body["has_converged"], true);
    assert_eq!(body["committed"], false);
    assert_eq!(body["campaign_id"], "cmp-1");
    assert_eq!(body["campaign_status"], "active");
}

#[tokio::test]
async fn get_bandit_state_non_bandit_omits_optional_fields() {
    use stitchd_proto::experiments::v1::GetBanditStateResponse;
    let exp_id = "88888888-8888-8888-8888-888888888888";
    let svc = MockExpService {
        bandit_state: GetBanditStateResponse {
            experiment_id: exp_id.to_string(),
            is_bandit: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/environments/env-1/experiments/{exp_id}/bandit"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["is_bandit"], false);
    assert!(body["current_allocation"].as_array().unwrap().is_empty());
    assert!(body.get("bandit_config").is_none());
    assert!(body.get("converged_variant").is_none());
    assert!(body.get("campaign_id").is_none());
    assert_eq!(body["has_converged"], false);
}

#[tokio::test]
async fn get_bandit_history_returns_timeline_newest_first() {
    use stitchd_proto::experiments::v1::BanditAllocationRun;
    let exp_id = "99999999-9999-9999-9999-999999999999";
    let svc = MockExpService {
        bandit_history: vec![
            BanditAllocationRun {
                fired_at_ms: 1_700_000_600_000,
                action: "commit".to_string(),
                outcome: "applied".to_string(),
                old_allocation_json: String::new(),
                new_allocation_json: r#"{"treatment":10000}"#.to_string(),
                detail: String::new(),
            },
            BanditAllocationRun {
                fired_at_ms: 1_700_000_000_000,
                action: "reallocate".to_string(),
                outcome: "applied".to_string(),
                old_allocation_json: r#"{"control":5000,"treatment":5000}"#.to_string(),
                new_allocation_json: r#"{"control":3000,"treatment":7000}"#.to_string(),
                detail: String::new(),
            },
        ],
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/v1/environments/env-1/experiments/{exp_id}/bandit/history?limit=50"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let runs = body["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0]["action"], "commit");
    assert_eq!(runs[0]["new_allocation"]["treatment"], 10000);
    // Empty old_allocation omitted on the commit row.
    assert!(runs[0].get("old_allocation").is_none());
    assert_eq!(runs[1]["action"], "reallocate");
    assert_eq!(runs[1]["old_allocation"]["control"], 5000);
    assert!(runs[0]["fired_at"].is_string());
}

#[tokio::test]
async fn list_bandit_campaigns_returns_campaigns() {
    use stitchd_proto::experiments::v1::BanditCampaign;
    let svc = MockExpService {
        campaigns: vec![BanditCampaign {
            id: "cmp-1".to_string(),
            environment_id: "env-1".to_string(),
            flag_id: "flag-1".to_string(),
            name: "homepage opt".to_string(),
            config: r#"{"max_iterations":5,"drift_threshold":0.2}"#.to_string(),
            status: "active".to_string(),
            iterations_spawned: 2,
            version: 3,
        }],
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/environments/env-1/bandit-campaigns")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let camps = body["campaigns"].as_array().unwrap();
    assert_eq!(camps.len(), 1);
    assert_eq!(camps[0]["name"], "homepage opt");
    assert_eq!(camps[0]["status"], "active");
    assert_eq!(camps[0]["iterations_spawned"], 2);
    assert_eq!(camps[0]["config"]["max_iterations"], 5);
}

#[tokio::test]
async fn get_bandit_campaign_returns_404_for_unknown() {
    let svc = MockExpService::default();
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/environments/env-1/bandit-campaigns/missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
