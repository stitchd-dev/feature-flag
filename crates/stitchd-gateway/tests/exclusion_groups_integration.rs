//! Integration tests for the Phase 7 exclusion-group REST surface.
//!
//! Spins up an in-process mock `ExperimentationService` and routes REST
//! requests through the gateway handlers, asserting:
//! - `GET /exclusion-groups` returns the group list with allocated/free capacity
//! - `POST /exclusion-groups` creates a group (201)
//! - assign returns 200 with the experiment + updated group on a capacity fit
//! - assign surfaces the `flag_locked_by_experiment` sentinel as a structured 409
//! - `GET /interactions` proxies and maps the interaction rows

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
    AssignExperimentToGroupRequest, AssignExperimentToGroupResponse, CreateExperimentRequest,
    DeleteExperimentRequest, ExclusionGroup, Experiment, ExperimentInteraction,
    ExperimentIteration, ExperimentResults, GetExperimentInteractionsRequest,
    GetExperimentInteractionsResponse, GetExperimentIterationRequest, GetExperimentRequest,
    GetResultsRequest, ListExclusionGroupsRequest, ListExclusionGroupsResponse,
    ListExperimentsRequest, ListExperimentsResponse, ListIterationsRequest, ListIterationsResponse,
    ListRunningExperimentsRequest, RunningExperiment, TransitionExperimentRequest,
    UnassignExperimentRequest, UnassignExperimentResponse, UpdateExperimentRequest,
    UpdateIterationLastComputedRequest, UpdateIterationLastComputedResponse,
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
    groups: Vec<ExclusionGroup>,
    group_total: u64,
    created_group: Option<ExclusionGroup>,
    /// When set, `get_exclusion_group` returns it; when `None`, returns NOT_FOUND.
    single_group: Option<ExclusionGroup>,
    /// When set, assign returns the `flag_locked_by_experiment:<id>` sentinel.
    locked_experiment_id: Option<String>,
    assign_response: Option<AssignExperimentToGroupResponse>,
    interactions: Vec<ExperimentInteraction>,
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
        Err(Status::unimplemented("not used"))
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
        Err(Status::unimplemented("not used"))
    }

    async fn create_exclusion_group(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::CreateExclusionGroupRequest>,
    ) -> Result<Response<ExclusionGroup>, Status> {
        Ok(Response::new(
            self.created_group.clone().unwrap_or_default(),
        ))
    }

    async fn get_exclusion_group(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::GetExclusionGroupRequest>,
    ) -> Result<Response<ExclusionGroup>, Status> {
        match &self.single_group {
            Some(g) => Ok(Response::new(g.clone())),
            None => Err(Status::not_found("exclusion group not found")),
        }
    }

    async fn list_exclusion_groups(
        &self,
        _req: tonic::Request<ListExclusionGroupsRequest>,
    ) -> Result<Response<ListExclusionGroupsResponse>, Status> {
        Ok(Response::new(ListExclusionGroupsResponse {
            groups: self.groups.clone(),
            total: self.group_total,
        }))
    }

    async fn update_exclusion_group(
        &self,
        _req: tonic::Request<stitchd_proto::experiments::v1::UpdateExclusionGroupRequest>,
    ) -> Result<Response<ExclusionGroup>, Status> {
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
        _req: tonic::Request<AssignExperimentToGroupRequest>,
    ) -> Result<Response<AssignExperimentToGroupResponse>, Status> {
        if let Some(exp_id) = &self.locked_experiment_id {
            return Err(Status::failed_precondition(format!(
                "flag_locked_by_experiment:{exp_id}"
            )));
        }
        Ok(Response::new(
            self.assign_response.clone().unwrap_or_default(),
        ))
    }

    async fn unassign_experiment(
        &self,
        _req: tonic::Request<UnassignExperimentRequest>,
    ) -> Result<Response<UnassignExperimentResponse>, Status> {
        Err(Status::unimplemented("not used"))
    }

    async fn get_experiment_interactions(
        &self,
        _req: tonic::Request<GetExperimentInteractionsRequest>,
    ) -> Result<Response<GetExperimentInteractionsResponse>, Status> {
        Ok(Response::new(GetExperimentInteractionsResponse {
            interactions: self.interactions.clone(),
        }))
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
    use axum::routing::{get, post};
    use stitchd_gateway::routes::{exclusion_groups as eg, experiments as exp_routes};
    axum::Router::new()
        .route(
            "/v1/environments/{environment_id}/exclusion-groups",
            get(eg::list_exclusion_groups).post(eg::create_exclusion_group),
        )
        .route(
            "/v1/environments/{environment_id}/exclusion-groups/{group_id}",
            get(eg::get_exclusion_group)
                .patch(eg::update_exclusion_group)
                .delete(eg::delete_exclusion_group),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/exclusion-group",
            post(eg::assign_experiment_to_group).delete(eg::unassign_experiment),
        )
        .route(
            "/v1/environments/{environment_id}/experiments/{experiment_id}/interactions",
            get(exp_routes::get_interactions),
        )
        .with_state(state)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_exclusion_groups_returns_capacity_fields() {
    let svc = MockExpService {
        groups: vec![ExclusionGroup {
            id: "grp-1".to_string(),
            env_id: "env-1".to_string(),
            name: "Checkout".to_string(),
            description: "checkout funnel".to_string(),
            allocated_bp: 3000,
            free_bp: 7000,
            version: 2,
            unit_context_type: "user".to_string(),
        }],
        group_total: 1,
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/environments/env-1/exclusion-groups")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let item = &body["items"][0];
    assert_eq!(item["id"], "grp-1");
    assert_eq!(item["allocated_bp"], 3000);
    assert_eq!(item["free_bp"], 7000);
    assert_eq!(item["version"], 2);
}

#[tokio::test]
async fn create_exclusion_group_returns_201() {
    let svc = MockExpService {
        created_group: Some(ExclusionGroup {
            id: "grp-new".to_string(),
            env_id: "env-1".to_string(),
            name: "Checkout".to_string(),
            description: "funnel".to_string(),
            allocated_bp: 0,
            free_bp: 10000,
            version: 1,
            unit_context_type: "user".to_string(),
        }),
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/environments/env-1/exclusion-groups")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Checkout","description":"funnel","unit_context_type":"account"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["id"], "grp-new");
    assert_eq!(body["free_bp"], 10000);
    // The created group's diversion unit is surfaced in the response body.
    assert_eq!(body["unit_context_type"], "user");
}

#[tokio::test]
async fn get_exclusion_group_returns_200_via_rpc() {
    let svc = MockExpService {
        single_group: Some(ExclusionGroup {
            id: "grp-1".to_string(),
            env_id: "env-1".to_string(),
            name: "Checkout".to_string(),
            description: "funnel".to_string(),
            allocated_bp: 2500,
            free_bp: 7500,
            version: 5,
            unit_context_type: "account".to_string(),
        }),
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/environments/env-1/exclusion-groups/grp-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["id"], "grp-1");
    assert_eq!(body["unit_context_type"], "account");
}

#[tokio::test]
async fn get_exclusion_group_returns_404_when_absent() {
    // single_group None → mock returns NOT_FOUND → gateway 404.
    let svc = MockExpService::default();
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/environments/env-1/exclusion-groups/missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn assign_experiment_returns_200_with_experiment_and_group() {
    let svc = MockExpService {
        assign_response: Some(AssignExperimentToGroupResponse {
            experiment: Some(Experiment {
                id: "exp-1".to_string(),
                environment_id: "env-1".to_string(),
                name: "Pricing".to_string(),
                exclusion_group_id: Some("grp-1".to_string()),
                ..Default::default()
            }),
            group: Some(ExclusionGroup {
                id: "grp-1".to_string(),
                env_id: "env-1".to_string(),
                name: "Checkout".to_string(),
                allocated_bp: 2500,
                free_bp: 7500,
                version: 3,
                ..Default::default()
            }),
        }),
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/environments/env-1/experiments/exp-1/exclusion-group")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"group_id":"grp-1","requested_bp":2500}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["experiment"]["id"], "exp-1");
    assert_eq!(body["group"]["allocated_bp"], 2500);
    assert_eq!(body["group"]["free_bp"], 7500);
}

#[tokio::test]
async fn assign_experiment_returns_409_when_flag_locked() {
    let exp_id = "55555555-5555-5555-5555-555555555555";
    let svc = MockExpService {
        locked_experiment_id: Some(exp_id.to_string()),
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/environments/env-1/experiments/exp-1/exclusion-group")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"group_id":"grp-1","requested_bp":2500}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "expected 409 when flag is locked; got {}",
        resp.status()
    );
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["error"], "flag_locked_by_experiment");
    assert_eq!(body["experiment_id"], exp_id);
}

#[tokio::test]
async fn get_interactions_maps_rows() {
    let svc = MockExpService {
        interactions: vec![
            ExperimentInteraction {
                experiment_id_a: "exp-1".to_string(),
                experiment_id_b: "exp-2".to_string(),
                other_experiment_name: "Banner test".to_string(),
                context_type: "user".to_string(),
                metric_key: "conversion".to_string(),
                shared_count: 4200,
                interaction_estimate: 0.034,
                p_value: 0.012,
                significant: true,
                insufficient_data: false,
            },
            // A pair with too few shared exposures to test: insufficient_data.
            ExperimentInteraction {
                experiment_id_a: "exp-1".to_string(),
                experiment_id_b: "exp-3".to_string(),
                other_experiment_name: "Sparse test".to_string(),
                context_type: "user".to_string(),
                metric_key: "conversion".to_string(),
                shared_count: 12,
                interaction_estimate: 0.0,
                p_value: 0.0,
                significant: false,
                insufficient_data: true,
            },
        ],
        ..Default::default()
    };
    let exp_client = spawn_mock_exp_service(svc).await;
    let app = build_router(make_state(exp_client));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/v1/environments/env-1/experiments/exp-1/interactions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let row = &body["interactions"][0];
    assert_eq!(row["experiment_id_b"], "exp-2");
    assert_eq!(row["other_experiment_name"], "Banner test");
    assert_eq!(row["metric_key"], "conversion");
    assert_eq!(row["shared_count"], 4200);
    assert_eq!(row["significant"], true);
    assert_eq!(row["insufficient_data"], false);
    // The sparse pair surfaces insufficient_data=true.
    let sparse = &body["interactions"][1];
    assert_eq!(sparse["experiment_id_b"], "exp-3");
    assert_eq!(sparse["insufficient_data"], true);
    assert_eq!(sparse["significant"], false);
}
