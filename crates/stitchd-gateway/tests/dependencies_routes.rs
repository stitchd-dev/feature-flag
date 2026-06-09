//! Integration tests for the gateway dependency-graph read API
//! (`flag_lifecycle_20260604` Phase 6 Task 2).
//!
//! Spins up an in-process mock `FlagService` and routes
//! `GET /v1/projects/{project_id}/{entity_kind}/{entity_id}/dependencies`
//! through the real gateway handler via `tower::ServiceExt::oneshot`, asserting
//! the aggregated upstream/downstream edges for flags + segments and the
//! experiment note branch.

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
    GetExperimentStartPrerequisitesRequest, GetExperimentStartPrerequisitesResponse,
    StartPrerequisite,
    experimentation_service_client::ExperimentationServiceClient,
    experimentation_service_server::{ExperimentationService, ExperimentationServiceServer},
};
use stitchd_proto::flags::v1::{
    EvaluatePreviewRequest, EvaluatePreviewResponse, FeatureFlag, FlagPrerequisite, FlagRule,
    GetFlagDefinitionsRequest, GetFlagRequest, GetPrerequisitesRequest, GetPrerequisitesResponse,
    ListFlagsRequest, ListFlagsResponse, MutateFlagRequest, MutateFlagResponse,
    SetPrerequisitesRequest, SetPrerequisitesResponse, UpdateFlagHashingRequest,
    UpdateFlagHashingResponse,
    flag_service_client::FlagServiceClient,
    flag_service_server::{FlagService, FlagServiceServer},
};
use stitchd_proto::management::v1::management_service_client::ManagementServiceClient;
use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;
use stitchd_proto::stats::v1::stats_service_client::StatsServiceClient;

use stitchd_gateway::state::GatewayState;

const SEG_A: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

fn build_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::get;
    use stitchd_gateway::routes::dependencies;
    axum::Router::new()
        .route(
            "/v1/projects/{project_id}/{entity_kind}/{entity_id}/dependencies",
            get(dependencies::get_dependencies),
        )
        .with_state(state)
}

/// Serialize a single-leaf `InSegment(seg)` ConditionExpr to the rule_payload
/// bytes the gateway parses.
fn in_segment_payload(seg: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "Leaf": { "InSegment": seg } })).unwrap()
}

/// The subject flag `dependent` has: a prerequisite flag `prereq`, a rule that
/// references segment SEG_A. The flag `downstream` lists `dependent` as a
/// prerequisite.
#[derive(Default, Clone)]
struct MockFlagService;

impl MockFlagService {
    fn subject_flag() -> FeatureFlag {
        FeatureFlag {
            key: "dependent".to_string(),
            flag_id: "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: "11111111-1111-1111-1111-111111111111".to_string(),
                prerequisite_flag_key: "prereq".to_string(),
                required_variant_id: "22222222-2222-2222-2222-222222222222".to_string(),
                required_variant_key: "on".to_string(),
            }],
            rules: vec![FlagRule {
                rule_payload: in_segment_payload(SEG_A),
                output: None,
                name: String::new(),
                rule_id: String::new(),
            }],
            ..Default::default()
        }
    }

    fn downstream_flag() -> FeatureFlag {
        FeatureFlag {
            key: "downstream".to_string(),
            flag_id: "dddddddd-dddd-dddd-dddd-dddddddddddd".to_string(),
            prerequisites: vec![FlagPrerequisite {
                prerequisite_flag_id: String::new(),
                prerequisite_flag_key: "dependent".to_string(),
                required_variant_id: String::new(),
                required_variant_key: "on".to_string(),
            }],
            ..Default::default()
        }
    }
}

#[tonic::async_trait]
impl FlagService for MockFlagService {
    async fn get_flag(
        &self,
        _req: tonic::Request<GetFlagRequest>,
    ) -> Result<Response<FeatureFlag>, Status> {
        Ok(Response::new(Self::subject_flag()))
    }
    async fn list_flags(
        &self,
        _req: tonic::Request<ListFlagsRequest>,
    ) -> Result<Response<ListFlagsResponse>, Status> {
        Ok(Response::new(ListFlagsResponse {
            flags: vec![Self::subject_flag(), Self::downstream_flag()],
            next_cursor: String::new(),
        }))
    }
    async fn mutate_flag(
        &self,
        _req: tonic::Request<MutateFlagRequest>,
    ) -> Result<Response<MutateFlagResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn update_flag_hashing(
        &self,
        _req: tonic::Request<UpdateFlagHashingRequest>,
    ) -> Result<Response<UpdateFlagHashingResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn evaluate_preview(
        &self,
        _req: tonic::Request<EvaluatePreviewRequest>,
    ) -> Result<Response<EvaluatePreviewResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    type GetFlagDefinitionsStream =
        tokio_stream::wrappers::ReceiverStream<Result<FeatureFlag, Status>>;
    async fn get_flag_definitions(
        &self,
        _req: tonic::Request<GetFlagDefinitionsRequest>,
    ) -> Result<Response<Self::GetFlagDefinitionsStream>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn set_default_rule_distribution(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::SetDefaultRuleDistributionRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::SetDefaultRuleDistributionResponse>, Status>
    {
        Err(Status::unimplemented("not used"))
    }
    async fn set_prerequisites(
        &self,
        _req: tonic::Request<SetPrerequisitesRequest>,
    ) -> Result<Response<SetPrerequisitesResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn get_prerequisites(
        &self,
        _req: tonic::Request<GetPrerequisitesRequest>,
    ) -> Result<Response<GetPrerequisitesResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn bandit_update_allocation(
        &self,
        _req: tonic::Request<stitchd_proto::flags::v1::BanditUpdateAllocationRequest>,
    ) -> Result<Response<stitchd_proto::flags::v1::BanditUpdateAllocationResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
}

/// Mock ExperimentationService whose `GetExperimentStartPrerequisites` returns a
/// flag_variant + experiment_done prerequisite, and whose every other RPC is
/// unimplemented (not exercised by the dependency-graph route).
#[derive(Clone)]
struct MockExpService {
    prereqs: Vec<StartPrerequisite>,
}

#[tonic::async_trait]
impl ExperimentationService for MockExpService {
    async fn get_experiment_start_prerequisites(
        &self,
        _req: tonic::Request<GetExperimentStartPrerequisitesRequest>,
    ) -> Result<Response<GetExperimentStartPrerequisitesResponse>, Status> {
        Ok(Response::new(GetExperimentStartPrerequisitesResponse {
            prerequisites: self.prereqs.clone(),
        }))
    }

    async fn create_experiment(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::CreateExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn get_experiment(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::GetExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn list_experiments(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::ListExperimentsRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ListExperimentsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn update_experiment(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::UpdateExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn delete_experiment(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::DeleteExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn transition_experiment(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::TransitionExperimentRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::Experiment>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn list_iterations(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::ListIterationsRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ListIterationsResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn get_results(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::GetResultsRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ExperimentResults>, Status> {
        Err(Status::unimplemented("not used"))
    }
    type ListRunningExperimentsStream = tokio_stream::wrappers::ReceiverStream<
        Result<stitchd_proto::experiments::v1::RunningExperiment, Status>,
    >;
    async fn list_running_experiments(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::ListRunningExperimentsRequest>,
    ) -> Result<Response<Self::ListRunningExperimentsStream>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn get_experiment_iteration(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::GetExperimentIterationRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ExperimentIteration>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn update_iteration_last_computed(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::UpdateIterationLastComputedRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::UpdateIterationLastComputedResponse>, Status>
    {
        Err(Status::unimplemented("not used"))
    }
    async fn list_exposures(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::ListExposuresRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ListExposuresResponse>, Status> {
        Err(Status::unimplemented("not used"))
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
        _req: tonic::Request<stitchd_proto::experiments::v1::GetBanditCampaignRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::BanditCampaign>, Status> {
        Err(Status::unimplemented("not used"))
    }
    async fn list_bandit_campaigns(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::ListBanditCampaignsRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::ListBanditCampaignsResponse>, Status> {
        Err(Status::unimplemented("not used"))
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
        Err(Status::unimplemented("not used"))
    }

    async fn get_bandit_allocation_history(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::GetBanditAllocationHistoryRequest>,
    ) -> Result<Response<stitchd_proto::experiments::v1::GetBanditAllocationHistoryResponse>, Status>
    {
        Err(Status::unimplemented("not used"))
    }
}

async fn spawn_mock_exp(prereqs: Vec<StartPrerequisite>) -> ExperimentationServiceClient<Channel> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(ExperimentationServiceServer::new(MockExpService {
                prereqs,
            }))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    ExperimentationServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap()
}

async fn spawn_mock() -> FlagServiceClient<Channel> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(FlagServiceServer::new(MockFlagService))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    FlagServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap()
}

fn make_state(flag_client: FlagServiceClient<Channel>) -> Arc<GatewayState> {
    make_state_with_exp(
        flag_client,
        ExperimentationServiceClient::new(
            Channel::from_static("http://127.0.0.1:5").connect_lazy(),
        ),
    )
}

fn make_state_with_exp(
    flag_client: FlagServiceClient<Channel>,
    exp_client: ExperimentationServiceClient<Channel>,
) -> Arc<GatewayState> {
    let flag_channel = Channel::from_static("http://127.0.0.1:2").connect_lazy();
    let seg_channel = Channel::from_static("http://127.0.0.1:3").connect_lazy();
    Arc::new(GatewayState::from_channels(
        AuthServiceClient::new(Channel::from_static("http://127.0.0.1:1").connect_lazy()),
        flag_client,
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
    ))
}

async fn get(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    (status, json)
}

#[tokio::test]
async fn flag_dependencies_aggregate_upstream_and_downstream() {
    let router = build_router(make_state(spawn_mock().await));
    let (status, json) = get(router, "/v1/projects/p1/flags/dependent/dependencies").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["entity_kind"], "flag");
    assert_eq!(json["entity_id"], "dependent");

    // Upstream: prerequisite flag `prereq` + segment SEG_A from the rule.
    let upstream = json["upstream"].as_array().unwrap();
    assert!(upstream.iter().any(|e| e["kind"] == "prerequisite_flag"
        && e["key"] == "prereq"
        && e["entity_kind"] == "flag"));
    assert!(
        upstream.iter().any(|e| e["kind"] == "segment_ref"
            && e["entity_kind"] == "segment"
            && e["id"] == SEG_A)
    );

    // Downstream: flag `downstream` lists `dependent` as a prerequisite.
    let downstream = json["downstream"].as_array().unwrap();
    assert_eq!(downstream.len(), 1);
    assert_eq!(downstream[0]["key"], "downstream");
    assert_eq!(downstream[0]["kind"], "dependent_flag");
}

#[tokio::test]
async fn segment_dependencies_lists_referencing_flags() {
    let router = build_router(make_state(spawn_mock().await));
    let (status, json) = get(
        router,
        &format!("/v1/projects/p1/segments/{SEG_A}/dependencies"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["entity_kind"], "segment");
    // The subject flag references SEG_A via its rule → it is a downstream dependent.
    let downstream = json["downstream"].as_array().unwrap();
    assert!(downstream.iter().any(|e| e["key"] == "dependent"));
    assert!(json["note"].is_string());
}

#[tokio::test]
async fn experiment_dependencies_populates_upstream_from_start_prereqs() {
    let flag_id = "33333333-3333-3333-3333-333333333333";
    let variant_id = "44444444-4444-4444-4444-444444444444";
    let prereq_exp = "55555555-5555-5555-5555-555555555555";
    let prereqs = vec![
        StartPrerequisite {
            kind: "flag_variant".to_string(),
            flag_id: flag_id.to_string(),
            required_variant_id: variant_id.to_string(),
            prerequisite_experiment_id: String::new(),
        },
        StartPrerequisite {
            kind: "experiment_done".to_string(),
            flag_id: String::new(),
            required_variant_id: String::new(),
            prerequisite_experiment_id: prereq_exp.to_string(),
        },
    ];
    let state = make_state_with_exp(spawn_mock().await, spawn_mock_exp(prereqs).await);
    let router = build_router(state);
    let (status, json) = get(router, "/v1/projects/p1/experiments/exp-1/dependencies").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["entity_kind"], "experiment");

    let upstream = json["upstream"].as_array().unwrap();
    assert_eq!(upstream.len(), 2);
    assert!(
        upstream
            .iter()
            .any(|e| e["kind"] == "prerequisite_flag_variant"
                && e["entity_kind"] == "flag"
                && e["id"] == flag_id)
    );
    assert!(
        upstream
            .iter()
            .any(|e| e["kind"] == "prerequisite_experiment_done"
                && e["entity_kind"] == "experiment"
                && e["id"] == prereq_exp)
    );
    assert!(json["downstream"].as_array().unwrap().is_empty());
    assert!(json["note"].is_string());
}

#[tokio::test]
async fn unknown_entity_kind_is_400() {
    let router = build_router(make_state(spawn_mock().await));
    let (status, _json) = get(router, "/v1/projects/p1/widgets/x/dependencies").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
